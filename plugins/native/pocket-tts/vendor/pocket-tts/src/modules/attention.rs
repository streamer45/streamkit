use crate::ModelState;
use crate::modules::rope::RotaryEmbedding;
use candle_core::{DType, Result, Tensor};
use candle_nn::{Linear, Module, VarBuilder};
use std::collections::HashMap;

#[derive(Clone)]
pub struct StreamingMultiheadAttention {
    embed_dim: usize,
    num_heads: usize,
    rope: RotaryEmbedding,
    in_proj: Linear,
    out_proj: Linear,
    context: Option<usize>,
    name: String,
}

impl StreamingMultiheadAttention {
    pub fn new(
        embed_dim: usize,
        num_heads: usize,
        rope: RotaryEmbedding,
        context: Option<usize>,
        name: &str,
        vb: VarBuilder,
    ) -> Result<Self> {
        // out_dim = embed_dim + 2 * kv_dim (GQA/MHA logic in original)
        // Original code:
        // out_dim = embed_dim
        // num_kv = num_heads
        // kv_dim = (embed_dim // num_heads) * num_kv -> so embed_dim
        // out_dim += 2 * kv_dim -> so 3 * embed_dim
        let in_proj = candle_nn::linear_no_bias(embed_dim, 3 * embed_dim, vb.pp("in_proj"))?;
        let out_proj = candle_nn::linear_no_bias(embed_dim, embed_dim, vb.pp("out_proj"))?;

        Ok(Self {
            embed_dim,
            num_heads,
            rope,
            in_proj,
            out_proj,
            context,
            name: name.to_string(),
        })
    }

    pub fn init_state(
        &self,
        batch_size: usize,
        _sequence_length: usize,
        device: &candle_core::Device,
    ) -> Result<HashMap<String, Tensor>> {
        let dim_per_head = self.embed_dim / self.num_heads;
        let mut state = HashMap::new();
        state.insert("pos".to_string(), Tensor::zeros((), DType::U32, device)?);

        // Initial capacity: match context if windowed, otherwise reasonable default
        let cap = self.context.unwrap_or(64);
        state.insert(
            "k_buf".to_string(),
            Tensor::zeros(
                (batch_size, self.num_heads, cap, dim_per_head),
                DType::F32,
                device,
            )?,
        );
        state.insert(
            "v_buf".to_string(),
            Tensor::zeros(
                (batch_size, self.num_heads, cap, dim_per_head),
                DType::F32,
                device,
            )?,
        );
        state.insert("l".to_string(), Tensor::zeros((), DType::I64, device)?);
        Ok(state)
    }

    pub fn forward(
        &self,
        query: &Tensor,
        model_state: &mut ModelState,
        current_pos: usize,
        current_len: usize,
    ) -> Result<Tensor> {
        let (b, t, _) = query.dims3()?;
        let d = self.embed_dim / self.num_heads;
        let window_size = self.context;

        // Auto-initialize state if missing
        if !model_state.contains_key(&self.name) {
            model_state.insert(self.name.clone(), self.init_state(b, 0, query.device())?);
        }

        let module_state = model_state.get_mut(&self.name).unwrap();

        let projected = self.in_proj.forward(query)?;

        // Reshape to (b, t, 3, h, d)
        let packed = projected.reshape((b, t, 3, self.num_heads, d))?;
        let mut q = packed.narrow(2, 0, 1)?.squeeze(2)?; // (b, t, h, d)
        let mut k = packed.narrow(2, 1, 1)?.squeeze(2)?; // (b, t, h, d)
        let mut v = packed.narrow(2, 2, 1)?.squeeze(2)?; // (b, t, h, d)

        // current_pos passed as argument

        // Apply RoPE
        // RoPE expects (B, T, H, D)
        (q, k) = self.rope.forward(&q, &k, current_pos)?;

        // Transpose q, k, v to (B, H, T, D) for SDPA and KV cache
        q = q.transpose(1, 2)?;
        k = k.transpose(1, 2)?;
        v = v.transpose(1, 2)?;

        // KV Cache Management with Doubling Buffer
        // We take ownership from the state to avoid clones and ensure uniqueness for slice_set
        let (mut k_buf, mut v_buf, current_len) =
            match (module_state.remove("k_buf"), module_state.remove("v_buf")) {
                (Some(kb), Some(vb)) => (kb, vb, current_len),
                _ => {
                    let initial_cap = window_size.unwrap_or(64);
                    let kb =
                        Tensor::zeros((b, self.num_heads, initial_cap, d), q.dtype(), q.device())?;
                    let vb =
                        Tensor::zeros((b, self.num_heads, initial_cap, d), q.dtype(), q.device())?;
                    (kb, vb, 0)
                }
            };

        let cap = k_buf.dim(2)?; // Current capacity of the buffer

        let (kc, vc, k_buf, v_buf, current_len) = if let Some(window_size) = self.context {
            // Windowed Attention (Mimi)
            // If the current batch is larger than or equal to the window size,
            // we can't just slice_set. We needs to handle the overflow by
            // producing a concatenated KV for the current call and a truncated
            // buffer for the next call.
            if t >= window_size {
                let kc = if current_len > 0 {
                    Tensor::cat(&[&k_buf.narrow(2, 0, current_len)?, &k], 2)?
                } else {
                    k.clone()
                };
                let vc = if current_len > 0 {
                    Tensor::cat(&[&v_buf.narrow(2, 0, current_len)?, &v], 2)?
                } else {
                    v.clone()
                };

                // For the next state, we only keep the last window_size
                let next_kb = kc
                    .narrow(2, kc.dim(2)? - window_size, window_size)?
                    .contiguous()?;
                let next_vb = vc
                    .narrow(2, vc.dim(2)? - window_size, window_size)?
                    .contiguous()?;
                (kc, vc, next_kb, next_vb, window_size)
            } else {
                // Standard streaming / small batch path
                let mut current_len = current_len;
                if current_len + t > window_size {
                    let shift = (current_len + t).saturating_sub(window_size);
                    let to_move = current_len.saturating_sub(shift);
                    if to_move > 0 {
                        let k_to_move = k_buf.narrow(2, shift, to_move)?;
                        let v_to_move = v_buf.narrow(2, shift, to_move)?;
                        k_buf.slice_set(&k_to_move.contiguous()?, 2, 0)?;
                        v_buf.slice_set(&v_to_move.contiguous()?, 2, 0)?;
                        current_len = to_move;
                    } else {
                        current_len = 0;
                    }
                }
                k_buf.slice_set(&k.contiguous()?, 2, current_len)?;
                v_buf.slice_set(&v.contiguous()?, 2, current_len)?;
                let next_len = current_len + t;

                // Get current KV for attention
                let kc = k_buf.narrow(2, 0, next_len)?;
                let vc = v_buf.narrow(2, 0, next_len)?;
                (kc, vc, k_buf, v_buf, next_len)
            }
        } else {
            // Linear Attention (FlowLM) with Doubling Buffer
            if current_len + t > cap {
                let new_cap = (current_len + t).next_power_of_two();
                let zeros_shape = (b, self.num_heads, new_cap - cap, d);
                let k_zeros = Tensor::zeros(zeros_shape, q.dtype(), q.device())?;
                let v_zeros = Tensor::zeros(zeros_shape, q.dtype(), q.device())?;
                k_buf = Tensor::cat(&[k_buf, k_zeros], 2)?;
                v_buf = Tensor::cat(&[v_buf, v_zeros], 2)?;
            }
            k_buf.slice_set(&k.contiguous()?, 2, current_len)?;
            v_buf.slice_set(&v.contiguous()?, 2, current_len)?;
            let next_len = current_len + t;

            // Get current KV for attention
            let kc = k_buf.narrow(2, 0, next_len)?;
            let vc = v_buf.narrow(2, 0, next_len)?;
            (kc, vc, k_buf, v_buf, next_len)
        };

        // Update state
        module_state.insert("k_buf".to_string(), k_buf);
        module_state.insert("v_buf".to_string(), v_buf);
        module_state.insert(
            "l".to_string(),
            Tensor::new(current_len as i64, q.device())?,
        );
        module_state.insert(
            "pos".to_string(),
            Tensor::new((current_pos + t) as u32, q.device())?,
        );

        // Scaled dot-product attention
        let scale = 1.0 / (d as f64).sqrt();
        let x = crate::modules::sdpa::sdpa(
            &q,
            &kc,
            &vc,
            scale,
            true, // is_causal
            self.context,
        )?;

        // Transpose back to [B, T, H, D] and project out
        let x = x.transpose(1, 2)?.reshape((b, t, self.embed_dim))?;
        let x = self.out_proj.forward(&x)?;

        Ok(x)
    }
}
