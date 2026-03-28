// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Compute shader: RGBA8 → NV12 or I420 conversion.
//
// BT.601 coefficients matching the CPU path in convert.rs:
//   Y  =  16 + 0.257*R + 0.504*G + 0.098*B
//   Cb = 128 - 0.148*R - 0.291*G + 0.439*B
//   Cr = 128 + 0.439*R - 0.368*G - 0.071*B
//
// Y plane: one thread per pixel.
// Chroma: computed for every 2×2 block by the thread at the top-left corner.
//
// Outputs are storage buffers (not storage textures) because R8Unorm and
// Rg8Unorm don't universally support STORAGE_BINDING across GPU vendors.
//
// Byte packing: each atomic<u32> in the buffer holds 4 packed bytes
// (little-endian).  Multiple threads may contribute different bytes to
// the same u32, so we use atomicOr to merge them without data races.
// The buffer must be zero-filled before dispatch.

struct Params {
    width: u32,
    height: u32,
    // 0 = NV12, 1 = I420
    format: u32,
    // Padded row stride for the Y buffer (in bytes), aligned to 4 for
    // u32 packing.
    y_stride: u32,
    // Padded row stride for the UV buffer (in bytes).
    uv_stride: u32,
    // Chroma width (width / 2).
    chroma_w: u32,
    // Chroma height (height / 2).
    chroma_h: u32,
    _pad: u32,
}

@group(0) @binding(0) var input: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> y_buf: array<atomic<u32>>;
@group(0) @binding(2) var<storage, read_write> uv_buf: array<atomic<u32>>;
@group(0) @binding(3) var<uniform> params: Params;

// Convert a single RGBA pixel to YUV.
fn rgb_to_yuv(rgb: vec3<f32>) -> vec3<f32> {
    // rgb is in [0.0, 1.0], convert to [0, 255] for BT.601 formula.
    let r = rgb.r * 255.0;
    let g = rgb.g * 255.0;
    let b = rgb.b * 255.0;

    let y  = (16.0  + 0.257 * r + 0.504 * g + 0.098 * b) / 255.0;
    let cb = (128.0 - 0.148 * r - 0.291 * g + 0.439 * b) / 255.0;
    let cr = (128.0 + 0.439 * r - 0.368 * g - 0.071 * b) / 255.0;

    return vec3<f32>(
        clamp(y, 0.0, 1.0),
        clamp(cb, 0.0, 1.0),
        clamp(cr, 0.0, 1.0),
    );
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if x >= params.width || y >= params.height {
        return;
    }

    let coord = vec2<i32>(i32(x), i32(y));
    let rgba = textureLoad(input, coord, 0);
    let yuv = rgb_to_yuv(rgba.rgb);

    // Write Y for every pixel using atomicOr to avoid data races.
    // Multiple threads contribute different byte lanes to the same u32.
    {
        let y_val = u32(yuv.x * 255.0 + 0.5);
        let byte_idx = y * params.y_stride + x;
        let word_idx = byte_idx / 4u;
        let lane = byte_idx % 4u;
        atomicOr(&y_buf[word_idx], y_val << (lane * 8u));
    }

    // Write chroma for top-left pixel of each 2×2 block only.
    if (x % 2u) == 0u && (y % 2u) == 0u {
        // Average the 2×2 block for chroma subsampling.
        let coord01 = vec2<i32>(i32(min(x + 1u, params.width - 1u)), i32(y));
        let coord10 = vec2<i32>(i32(x), i32(min(y + 1u, params.height - 1u)));
        let coord11 = vec2<i32>(i32(min(x + 1u, params.width - 1u)), i32(min(y + 1u, params.height - 1u)));

        let yuv01 = rgb_to_yuv(textureLoad(input, coord01, 0).rgb);
        let yuv10 = rgb_to_yuv(textureLoad(input, coord10, 0).rgb);
        let yuv11 = rgb_to_yuv(textureLoad(input, coord11, 0).rgb);

        let avg_cb = (yuv.y + yuv01.y + yuv10.y + yuv11.y) * 0.25;
        let avg_cr = (yuv.z + yuv01.z + yuv10.z + yuv11.z) * 0.25;

        let cb_val = u32(clamp(avg_cb, 0.0, 1.0) * 255.0 + 0.5);
        let cr_val = u32(clamp(avg_cr, 0.0, 1.0) * 255.0 + 0.5);

        let cx = x / 2u;
        let cy = y / 2u;

        if params.format == 0u {
            // NV12: interleaved UV.  Two bytes per chroma sample.
            let uv_byte_0 = cy * params.uv_stride + cx * 2u;
            let uv_byte_1 = uv_byte_0 + 1u;

            let w0 = uv_byte_0 / 4u;
            let l0 = uv_byte_0 % 4u;
            atomicOr(&uv_buf[w0], cb_val << (l0 * 8u));

            let w1 = uv_byte_1 / 4u;
            let l1 = uv_byte_1 % 4u;
            atomicOr(&uv_buf[w1], cr_val << (l1 * 8u));
        } else {
            // I420: separate U and V planes, packed sequentially.
            // U plane: rows [0, chroma_h)
            // V plane: rows [chroma_h, 2*chroma_h)
            let u_byte = cy * params.uv_stride + cx;
            let v_byte = (cy + params.chroma_h) * params.uv_stride + cx;

            let uw = u_byte / 4u;
            let ul = u_byte % 4u;
            atomicOr(&uv_buf[uw], cb_val << (ul * 8u));

            let vw = v_byte / 4u;
            let vl = v_byte % 4u;
            atomicOr(&uv_buf[vw], cr_val << (vl * 8u));
        }
    }
}
