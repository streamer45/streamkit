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

struct Params {
    width: u32,
    height: u32,
    // 0 = NV12, 1 = I420
    format: u32,
    _pad: u32,
}

@group(0) @binding(0) var input: texture_2d<f32>;
@group(0) @binding(1) var y_output: texture_storage_2d<r8unorm, write>;
@group(0) @binding(2) var uv_output: texture_storage_2d<rg8unorm, write>;
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

    // Write Y for every pixel.
    textureStore(y_output, coord, vec4<f32>(yuv.x, 0.0, 0.0, 1.0));

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

        let chroma_coord = vec2<i32>(i32(x / 2u), i32(y / 2u));

        if params.format == 0u {
            // NV12: interleaved UV in rg8unorm.
            textureStore(uv_output, chroma_coord, vec4<f32>(avg_cb, avg_cr, 0.0, 1.0));
        } else {
            // I420: U and V planes packed vertically.
            // U in rows [0, height/2), V in rows [height/2, height).
            let chroma_h = i32(params.height / 2u);
            textureStore(uv_output, chroma_coord, vec4<f32>(avg_cb, 0.0, 0.0, 1.0));
            textureStore(uv_output, vec2<i32>(chroma_coord.x, chroma_coord.y + chroma_h), vec4<f32>(avg_cr, 0.0, 0.0, 1.0));
        }
    }
}
