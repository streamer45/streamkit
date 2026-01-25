use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use pocket_tts::modules::{attention::StreamingMultiheadAttention, rope::RotaryEmbedding};
use std::collections::HashMap;

fn bench_attention_scaling(c: &mut Criterion) {
    let device = Device::Cpu;
    let dim = 512;
    let heads = 8;
    let dim_head = dim / heads;
    let max_period = 10000.0;

    // Setup model
    let vb = VarBuilder::zeros(DType::F32, &device);
    let rope = RotaryEmbedding::new(max_period as f32, dim_head, &device).unwrap();
    let attention =
        StreamingMultiheadAttention::new(dim, heads, rope, None, "bench_attn", vb).unwrap();

    let mut group = c.benchmark_group("Attention_Forward_Step_Time");

    // Measure time for a SINGLE step at various context lengths
    for start_len in [0, 500, 1000, 1500, 2000, 3000].iter() {
        group.bench_with_input(
            BenchmarkId::new("ctx_len", start_len),
            start_len,
            |b, &len| {
                let q = Tensor::randn(0f32, 1.0, (1, 1, dim), &device).unwrap();

                b.iter_custom(|iters| {
                    let mut total_time = std::time::Duration::ZERO;

                    for _ in 0..iters {
                        let mut state = HashMap::new();
                        let mut module_state = HashMap::new();

                        if len > 0 {
                            let k = Tensor::zeros(
                                (1, heads, len.max(64), dim_head),
                                DType::F32,
                                &device,
                            )
                            .unwrap();
                            let v = Tensor::zeros(
                                (1, heads, len.max(64), dim_head),
                                DType::F32,
                                &device,
                            )
                            .unwrap();

                            module_state.insert("k_buf".to_string(), k);
                            module_state.insert("v_buf".to_string(), v);
                        }

                        state.insert("bench_attn".to_string(), module_state);

                        let start = std::time::Instant::now();
                        let _ = attention.forward(&q, &mut state, len, len).unwrap();
                        total_time += start.elapsed();
                    }
                    total_time
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_attention_scaling);
criterion_main!(benches);
