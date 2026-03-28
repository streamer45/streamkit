// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Vertex + Fragment shader: textured-quad compositing with alpha blending.
//
// Each layer is drawn as a fullscreen quad with a per-layer transform matrix
// that maps the quad to its destination position on the canvas.  The fragment
// shader samples the layer texture with UV remapping for crop/zoom, applies
// circle-crop clipping, and modulates alpha for opacity.
//
// GPU blend state (set on the pipeline, not in the shader):
//   src_factor: SrcAlpha
//   dst_factor: OneMinusSrcAlpha
//   operation: Add
// This implements standard Porter-Duff "over" compositing.

// Per-layer uniforms.
struct LayerUniforms {
    // 4×4 transform matrix (only 2D affine, but 4×4 for std140 compat).
    // Maps quad vertex positions [-1,1] to canvas NDC positions.
    // Encodes: scale to dest rect, rotation, mirror, translation.
    transform: mat4x4<f32>,
    // Source sub-region for crop/zoom (normalised UV coords).
    // [u_min, v_min, u_max, v_max]
    src_region: vec4<f32>,
    // Layer opacity (0.0–1.0).
    opacity: f32,
    // 1.0 for circle crop, 0.0 for rect.
    circle_crop: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(0) var<uniform> layer: LayerUniforms;
@group(1) @binding(0) var layer_texture: texture_2d<f32>;
@group(1) @binding(1) var layer_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Fullscreen quad: two triangles from 6 vertices (no index buffer needed).
// Vertices are generated procedurally from vertex_index.
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    // Quad vertices: 0=TL, 1=BL, 2=TR, 3=TR, 4=BL, 5=BR
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
    );

    var out: VertexOutput;
    let pos = positions[vi];
    out.position = layer.transform * vec4<f32>(pos, 0.0, 1.0);

    // Remap UVs to the source sub-region (crop/zoom).
    let uv = uvs[vi];
    out.uv = mix(layer.src_region.xy, layer.src_region.zw, uv);

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Discard fragments outside [0,1] UV range (prevents texture wrapping
    // for partially off-canvas layers).
    if in.uv.x < 0.0 || in.uv.x > 1.0 || in.uv.y < 0.0 || in.uv.y > 1.0 {
        discard;
    }

    // Circle-crop: discard fragments outside the unit circle inscribed
    // in the quad.  Uses smoothstep for 1px anti-aliased edges.
    if layer.circle_crop > 0.5 {
        let centre = vec2<f32>(0.5, 0.5);
        let dist = length(in.uv - centre) * 2.0;
        if dist > 1.0 {
            discard;
        }
        // Optional: smooth edge (anti-aliasing)
        // let alpha_mask = 1.0 - smoothstep(0.98, 1.0, dist);
    }

    var color = textureSample(layer_texture, layer_sampler, in.uv);
    color.a *= layer.opacity;

    return color;
}
