// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Compute shader: YUV (I420 or NV12) → RGBA8 conversion.
//
// BT.601 coefficients matching the CPU scalar path in convert.rs:
//   R = 1.164*(Y-16) + 1.596*(V-128)
//   G = 1.164*(Y-16) - 0.813*(V-128) - 0.391*(U-128)
//   B = 1.164*(Y-16) + 2.018*(U-128)
//
// Workgroup: 16×16 threads, one thread per output pixel.

struct Params {
    width: u32,
    height: u32,
    // 0 = NV12 (UV interleaved), 1 = I420 (planar U then V)
    format: u32,
    _pad: u32,
}

@group(0) @binding(0) var y_tex: texture_2d<f32>;
@group(0) @binding(1) var uv_tex: texture_2d<f32>;
@group(0) @binding(2) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if x >= params.width || y >= params.height {
        return;
    }

    let coord = vec2<i32>(i32(x), i32(y));
    let chroma_coord = vec2<i32>(i32(x / 2u), i32(y / 2u));

    // Sample luma (stored in the R channel of an r8unorm texture).
    let y_val = textureLoad(y_tex, coord, 0).r;

    var u_val: f32;
    var v_val: f32;

    if params.format == 0u {
        // NV12: UV interleaved in an rg8unorm texture.
        let uv = textureLoad(uv_tex, chroma_coord, 0);
        u_val = uv.r;
        v_val = uv.g;
    } else {
        // I420: U and V are packed vertically in a single r8unorm texture.
        // U plane occupies rows [0, height/2), V plane occupies rows [height/2, height).
        let chroma_h = i32(params.height / 2u);
        u_val = textureLoad(uv_tex, vec2<i32>(chroma_coord.x, chroma_coord.y), 0).r;
        v_val = textureLoad(uv_tex, vec2<i32>(chroma_coord.x, chroma_coord.y + chroma_h), 0).r;
    }

    // BT.601 YUV → RGB conversion.
    // Input textures are r8unorm so values are already in [0.0, 1.0].
    // Convert to [0, 255] range for the standard integer formula, then
    // back to [0.0, 1.0] for the rgba8unorm output.
    let y_scaled = (y_val * 255.0 - 16.0) * (1.164 / 255.0);
    let u_shifted = u_val * 255.0 - 128.0;
    let v_shifted = v_val * 255.0 - 128.0;

    let r = y_scaled + v_shifted * (1.596 / 255.0);
    let g = y_scaled - v_shifted * (0.813 / 255.0) - u_shifted * (0.391 / 255.0);
    let b = y_scaled + u_shifted * (2.018 / 255.0);

    textureStore(output, coord, vec4<f32>(
        clamp(r, 0.0, 1.0),
        clamp(g, 0.0, 1.0),
        clamp(b, 0.0, 1.0),
        1.0,
    ));
}
