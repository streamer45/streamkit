// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! GPU compositing backend using wgpu.
//!
//! When the `gpu` feature is enabled and a GPU adapter is available at
//! runtime, the compositing thread uses this backend instead of the CPU
//! `composite_frame()` path.  Falls back to CPU transparently on init
//! failure.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use streamkit_core::frame_pool::PooledVideoData;
use streamkit_core::types::PixelFormat;

use super::config::{CropShape, Rect};
use super::kernel::LayerSnapshot;
use super::overlay::DecodedOverlay;

// ── Uniform types ───────────────────────────────────────────────────────────

/// Per-layer uniform data uploaded to the GPU each frame.
///
/// Layout matches `LayerUniforms` in `composite.wgsl` (std140).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct LayerUniforms {
    /// 4×4 transform matrix (column-major).
    /// Encodes: scale to dest rect, rotation, mirror, translation to NDC.
    transform: [[f32; 4]; 4],
    /// Source sub-region for crop/zoom (normalised UV coords).
    /// `[u_min, v_min, u_max, v_max]`
    src_region: [f32; 4],
    /// Layer opacity (0.0–1.0).
    opacity: f32,
    /// 1.0 for circle crop, 0.0 for rect.
    circle_crop: f32,
    _pad: [f32; 2],
}

/// Uniform params for the YUV→RGBA compute shader.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct YuvToRgbaParams {
    width: u32,
    height: u32,
    /// 0 = NV12, 1 = I420.
    format: u32,
    _pad: u32,
}

/// Uniform params for the RGBA→YUV compute shader.
///
/// Uses storage buffers (not storage textures) for output because
/// `R8Unorm`/`Rg8Unorm` don't universally support `STORAGE_BINDING`.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct RgbaToYuvParams {
    width: u32,
    height: u32,
    /// 0 = NV12, 1 = I420.
    format: u32,
    /// Row stride (bytes) for the Y output buffer.
    y_stride: u32,
    /// Row stride (bytes) for the UV output buffer.
    uv_stride: u32,
    /// Chroma width  (width.div_ceil(2)).
    chroma_w: u32,
    /// Chroma height (height.div_ceil(2)).
    chroma_h: u32,
    _pad: u32,
}

// ── Canvas texture cache ────────────────────────────────────────────────────

/// Output canvas texture + view, cached across frames.
struct CanvasTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

// ── GPU context ─────────────────────────────────────────────────────────────

/// GPU compositing context — owns the wgpu device, queue, and
/// pre-compiled pipelines.  Created once when the compositing thread
/// starts; lives for the node's lifetime.
pub struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,

    // ── YUV → RGBA pipeline ──
    yuv_to_rgba_pipeline: wgpu::ComputePipeline,
    yuv_to_rgba_bgl: wgpu::BindGroupLayout,

    // ── Layer compositing pipeline ──
    composite_pipeline: wgpu::RenderPipeline,
    layer_uniforms_bgl: wgpu::BindGroupLayout,
    layer_texture_bgl: wgpu::BindGroupLayout,

    // ── RGBA → YUV pipeline ──
    rgba_to_yuv_pipeline: wgpu::ComputePipeline,
    rgba_to_yuv_bgl: wgpu::BindGroupLayout,

    /// Linear-filtering sampler shared across all layer draws.
    sampler: wgpu::Sampler,

    /// Cached output canvas texture — recreated when canvas size changes.
    canvas: Option<CanvasTexture>,

    /// Staging buffer for GPU→CPU readback — sized to canvas.
    readback_buffer: Option<wgpu::Buffer>,
}

impl GpuContext {
    /// Attempt to create a GPU context.  Returns `None` if no suitable
    /// adapter is found or device creation fails.
    ///
    /// Uses `pollster::block_on` since this runs on a blocking thread,
    /// not inside a tokio runtime.
    pub fn try_init() -> Option<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::METAL | wgpu::Backends::DX12,
            ..Default::default()
        });

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .ok()?;

        tracing::info!(
            "wgpu adapter: {} ({:?})",
            adapter.get_info().name,
            adapter.get_info().backend,
        );

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("streamkit-compositor"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .ok()?;

        // ── YUV → RGBA compute pipeline ─────────────────────────────
        let yuv_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv_to_rgba"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/yuv_to_rgba.wgsl").into()),
        });

        let yuv_to_rgba_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv_to_rgba_bgl"),
            entries: &[
                // Y texture
                bgl_texture_entry(
                    0,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::ShaderStages::COMPUTE,
                ),
                // UV texture
                bgl_texture_entry(
                    1,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::ShaderStages::COMPUTE,
                ),
                // Output storage texture
                bgl_storage_texture_entry(2, wgpu::TextureFormat::Rgba8Unorm),
                // Params uniform
                bgl_uniform_entry(3, wgpu::ShaderStages::COMPUTE),
            ],
        });

        let yuv_to_rgba_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("yuv_to_rgba_pipeline"),
                layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("yuv_to_rgba_layout"),
                    bind_group_layouts: &[&yuv_to_rgba_bgl],
                    push_constant_ranges: &[],
                })),
                module: &yuv_shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });

        // ── Layer compositing render pipeline ────────────────────────
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/composite.wgsl").into()),
        });

        let layer_uniforms_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("layer_uniforms_bgl"),
                entries: &[bgl_uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
            });

        let layer_texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer_texture_bgl"),
            entries: &[
                bgl_texture_entry(
                    0,
                    wgpu::TextureSampleType::Float { filterable: true },
                    wgpu::ShaderStages::FRAGMENT,
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("composite_layout"),
                bind_group_layouts: &[&layer_uniforms_bgl, &layer_texture_bgl],
                push_constant_ranges: &[],
            });

        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite_pipeline"),
            layout: Some(&composite_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // ── RGBA → YUV compute pipeline ─────────────────────────────
        let rgba_to_yuv_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rgba_to_yuv"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/rgba_to_yuv.wgsl").into()),
        });

        let rgba_to_yuv_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rgba_to_yuv_bgl"),
            entries: &[
                // Input RGBA texture
                bgl_texture_entry(
                    0,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::ShaderStages::COMPUTE,
                ),
                // Y output storage buffer
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // UV output storage buffer
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Params uniform
                bgl_uniform_entry(3, wgpu::ShaderStages::COMPUTE),
            ],
        });

        let rgba_to_yuv_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("rgba_to_yuv_pipeline"),
                layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("rgba_to_yuv_layout"),
                    bind_group_layouts: &[&rgba_to_yuv_bgl],
                    push_constant_ranges: &[],
                })),
                module: &rgba_to_yuv_shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });

        // ── Sampler ─────────────────────────────────────────────────
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("layer_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        Some(Self {
            device,
            queue,
            yuv_to_rgba_pipeline,
            yuv_to_rgba_bgl,
            composite_pipeline,
            layer_uniforms_bgl,
            layer_texture_bgl,
            rgba_to_yuv_pipeline,
            rgba_to_yuv_bgl,
            sampler,
            canvas: None,
            readback_buffer: None,
        })
    }

    /// Ensure the canvas texture and readback buffer match the requested
    /// dimensions.  Recreates them if the canvas size changed.
    fn ensure_canvas(&mut self, width: u32, height: u32) {
        let needs_recreate =
            self.canvas.as_ref().is_none_or(|c| c.width != width || c.height != height);

        if needs_recreate {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("compositor_canvas"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.canvas = Some(CanvasTexture { texture, view, width, height });

            // Readback buffer: RGBA8 = 4 bytes per pixel, rows padded to
            // COPY_BYTES_PER_ROW_ALIGNMENT (256).
            let padded_row = padded_bytes_per_row(width, 4);
            let buf_size = (padded_row as u64) * u64::from(height);
            self.readback_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("compositor_readback"),
                size: buf_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }));
        }
    }

    /// Upload an RGBA8 buffer to a GPU texture suitable for sampling.
    fn upload_rgba_texture(&self, data: &[u8], width: u32, height: u32) -> wgpu::Texture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer_rgba"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        texture
    }

    /// Upload a YUV frame (I420 or NV12) and convert to RGBA8 on the GPU.
    ///
    /// Returns an RGBA8 texture ready for sampling in the compositing pass.
    ///
    /// TODO(phase-3): accept an external `CommandEncoder` so multiple YUV
    /// layer conversions can be batched into a single `queue.submit()`.
    fn upload_yuv_layer(
        &self,
        data: &[u8],
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
    ) -> wgpu::Texture {
        let (y_tex, uv_tex, format_id) = match pixel_format {
            PixelFormat::Nv12 => {
                let y_size = (width as usize) * (height as usize);
                let y_tex = self.create_and_write_r8_texture(
                    "y_plane_nv12",
                    width,
                    height,
                    &data[..y_size],
                );
                let chroma_w = width.div_ceil(2);
                let chroma_h = height.div_ceil(2);
                let uv_tex = self.create_and_write_rg8_texture(
                    "uv_plane_nv12",
                    chroma_w,
                    chroma_h,
                    &data[y_size..y_size + (chroma_w as usize) * (chroma_h as usize) * 2],
                );
                (y_tex, uv_tex, 0u32)
            },
            PixelFormat::I420 => {
                let y_size = (width as usize) * (height as usize);
                let y_tex = self.create_and_write_r8_texture(
                    "y_plane_i420",
                    width,
                    height,
                    &data[..y_size],
                );
                // Pack U and V planes vertically into a single R8 texture:
                // rows [0, chroma_h) = U, rows [chroma_h, 2*chroma_h) = V.
                let chroma_w = width.div_ceil(2);
                let chroma_h = height.div_ceil(2);
                let u_size = (chroma_w as usize) * (chroma_h as usize);
                let u_data = &data[y_size..y_size + u_size];
                let v_data = &data[y_size + u_size..y_size + 2 * u_size];
                let mut packed = Vec::with_capacity(u_size + u_size);
                packed.extend_from_slice(u_data);
                packed.extend_from_slice(v_data);
                let uv_tex = self.create_and_write_r8_texture(
                    "uv_plane_i420",
                    chroma_w,
                    chroma_h * 2,
                    &packed,
                );
                (y_tex, uv_tex, 1u32)
            },
            _ => unreachable!("upload_yuv_layer called with non-YUV format"),
        };

        // Output RGBA8 texture (written by compute shader).
        let output_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("yuv_to_rgba_output"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let params_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("yuv_params"),
            size: std::mem::size_of::<YuvToRgbaParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(
            &params_buf,
            0,
            bytemuck::bytes_of(&YuvToRgbaParams { width, height, format: format_id, _pad: 0 }),
        );

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuv_to_rgba_bg"),
            layout: &self.yuv_to_rgba_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &y_tex.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &uv_tex.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &output_tex.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry { binding: 3, resource: params_buf.as_entire_binding() },
            ],
        });

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("yuv_to_rgba_encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("yuv_to_rgba_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.yuv_to_rgba_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));

        output_tex
    }

    /// Upload a single layer to a GPU texture.
    /// Returns the RGBA8 texture (after YUV conversion if needed).
    fn upload_layer_texture(
        &self,
        data: &[u8],
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
    ) -> wgpu::Texture {
        match pixel_format {
            PixelFormat::Rgba8 => self.upload_rgba_texture(data, width, height),
            PixelFormat::I420 | PixelFormat::Nv12 => {
                self.upload_yuv_layer(data, width, height, pixel_format)
            },
            _ => {
                tracing::warn!("Unsupported pixel format for GPU upload: {pixel_format:?}");
                // Return a 1×1 transparent texture as fallback.
                self.upload_rgba_texture(&[0, 0, 0, 0], 1, 1)
            },
        }
    }

    /// GPU-accelerated equivalent of `kernel::composite_frame()`.
    ///
    /// 1. Upload each visible layer/overlay to a GPU texture.
    /// 2. Clear the canvas.
    /// 3. For each item in z-sorted order, draw a textured quad with
    ///    the layer's transform + opacity.
    /// 4. Optionally convert RGBA→YUV on GPU.
    /// 5. Copy output texture → staging buffer → CPU.
    ///
    /// TODO(phase-3): pool per-layer textures and uniform buffers instead
    /// of creating new ones each frame.  At 30fps with 2 layers + 2
    /// overlays this is ~120 texture + ~120 buffer allocations/sec.
    // Allow: GPU compositing coordinates upload → render pass → readback in a
    // single function; splitting would add complexity without improving clarity.
    // Will shrink naturally when per-layer texture pooling is added (phase-3).
    #[allow(clippy::too_many_lines)]
    pub fn composite_frame_gpu(
        &mut self,
        canvas_w: u32,
        canvas_h: u32,
        layers: &[Option<LayerSnapshot>],
        image_overlays: &[Arc<DecodedOverlay>],
        text_overlays: &[Arc<DecodedOverlay>],
        video_pool: Option<&streamkit_core::VideoFramePool>,
        output_format: Option<PixelFormat>,
    ) -> (PooledVideoData, PixelFormat) {
        self.ensure_canvas(canvas_w, canvas_h);

        // ── Build z-sorted draw list ────────────────────────────────
        struct DrawItem {
            texture: wgpu::Texture,
            uniforms: LayerUniforms,
            sort_key: (i32, usize),
        }

        let mut items: Vec<DrawItem> =
            Vec::with_capacity(layers.len() + image_overlays.len() + text_overlays.len());
        let mut insertion_order: usize = 0;

        // Video layers.
        for layer_opt in layers {
            if let Some(layer) = layer_opt {
                let texture = self.upload_layer_texture(
                    layer.data.as_slice(),
                    layer.width,
                    layer.height,
                    layer.pixel_format,
                );
                let dst =
                    layer.rect.unwrap_or(Rect { x: 0, y: 0, width: canvas_w, height: canvas_h });
                let uniforms = build_layer_uniforms(
                    canvas_w,
                    canvas_h,
                    &dst,
                    layer.opacity,
                    layer.rotation_degrees,
                    layer.mirror_horizontal,
                    layer.mirror_vertical,
                    layer.crop_zoom,
                    layer.crop_x,
                    layer.crop_y,
                    layer.crop_shape,
                );
                items.push(DrawItem {
                    texture,
                    uniforms,
                    sort_key: (layer.z_index, insertion_order),
                });
                insertion_order += 1;
            }
        }

        // Image overlays.
        for ov in image_overlays {
            let texture = self.upload_rgba_texture(&ov.rgba_data, ov.width, ov.height);
            let uniforms = build_layer_uniforms(
                canvas_w,
                canvas_h,
                &ov.rect,
                ov.opacity,
                ov.rotation_degrees,
                ov.mirror_horizontal,
                ov.mirror_vertical,
                1.0,
                0.5,
                0.5,
                CropShape::Rect,
            );
            items.push(DrawItem { texture, uniforms, sort_key: (ov.z_index, insertion_order) });
            insertion_order += 1;
        }

        // Text overlays.
        for ov in text_overlays {
            let texture = self.upload_rgba_texture(&ov.rgba_data, ov.width, ov.height);
            let uniforms = build_layer_uniforms(
                canvas_w,
                canvas_h,
                &ov.rect,
                ov.opacity,
                ov.rotation_degrees,
                ov.mirror_horizontal,
                ov.mirror_vertical,
                1.0,
                0.5,
                0.5,
                CropShape::Rect,
            );
            items.push(DrawItem { texture, uniforms, sort_key: (ov.z_index, insertion_order) });
            insertion_order += 1;
        }

        // Stable sort: lower z_index drawn first (bottom).
        items.sort_by_key(|item| item.sort_key);

        // ── Render pass: composite all layers onto the canvas ────────
        let canvas = self.canvas.as_ref().expect("canvas was just ensured");
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("composite_encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &canvas.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.composite_pipeline);

            for item in &items {
                // Per-layer uniform buffer.
                let uniform_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("layer_uniform_buf"),
                    size: std::mem::size_of::<LayerUniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&item.uniforms));

                let uniform_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("layer_uniform_bg"),
                    layout: &self.layer_uniforms_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform_bg_resource(&uniform_buf),
                    }],
                });

                let tex_view = item.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let texture_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("layer_texture_bg"),
                    layout: &self.layer_texture_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&tex_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });

                pass.set_bind_group(0, &uniform_bg, &[]);
                pass.set_bind_group(1, &texture_bg, &[]);
                pass.draw(0..6, 0..1); // 6 vertices = fullscreen quad
            }
        }

        // ── Output conversion or readback ───────────────────────────
        let (output_data, pix_fmt) = match output_format {
            Some(fmt @ (PixelFormat::Nv12 | PixelFormat::I420)) => {
                // Convert RGBA→YUV on GPU, then read back the YUV planes.
                self.queue.submit(std::iter::once(encoder.finish()));
                let yuv_data = self.convert_and_readback_yuv(canvas_w, canvas_h, fmt, video_pool);
                (yuv_data, fmt)
            },
            _ => {
                // Read back RGBA8 directly.
                let readback_buf =
                    self.readback_buffer.as_ref().expect("readback buffer was just ensured");
                let padded_row = padded_bytes_per_row(canvas_w, 4);
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &canvas.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: readback_buf,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(padded_row as u32),
                            rows_per_image: Some(canvas_h),
                        },
                    },
                    wgpu::Extent3d { width: canvas_w, height: canvas_h, depth_or_array_layers: 1 },
                );
                self.queue.submit(std::iter::once(encoder.finish()));

                let data = self.readback_rgba(canvas_w, canvas_h, video_pool);
                (data, PixelFormat::Rgba8)
            },
        };

        (output_data, pix_fmt)
    }

    /// Read back the RGBA8 canvas from the staging buffer into CPU memory.
    fn readback_rgba(
        &self,
        width: u32,
        height: u32,
        video_pool: Option<&streamkit_core::VideoFramePool>,
    ) -> PooledVideoData {
        let readback_buf = self.readback_buffer.as_ref().expect("readback buffer exists");
        let buf_slice = readback_buf.slice(..);

        let unpadded_row = (width as usize) * 4;
        let total_bytes = unpadded_row * (height as usize);

        let mut pooled = video_pool.map_or_else(
            || PooledVideoData::from_vec(vec![0u8; total_bytes]),
            |pool| pool.get(total_bytes),
        );

        // Block until the GPU finishes and the buffer is mapped.
        let (tx, rx) = std::sync::mpsc::channel();
        buf_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = self.device.poll(wgpu::PollType::Wait);
        match rx.recv() {
            Ok(Ok(())) => {},
            Ok(Err(e)) => {
                tracing::error!("GPU buffer mapping failed: {e}");
                return pooled;
            },
            Err(e) => {
                tracing::error!("GPU readback channel error: {e}");
                return pooled;
            },
        }

        let mapped = buf_slice.get_mapped_range();
        let padded_row = padded_bytes_per_row(width, 4);

        // Copy rows, stripping padding.
        let out = pooled.as_mut_slice();
        if padded_row == unpadded_row {
            out[..total_bytes].copy_from_slice(&mapped[..total_bytes]);
        } else {
            for row in 0..(height as usize) {
                let src_off = row * padded_row;
                let dst_off = row * unpadded_row;
                out[dst_off..dst_off + unpadded_row]
                    .copy_from_slice(&mapped[src_off..src_off + unpadded_row]);
            }
        }
        drop(mapped);
        readback_buf.unmap();

        pooled
    }

    /// Convert the canvas RGBA8 texture to NV12/I420 on the GPU and read
    /// back the resulting YUV planes.
    fn convert_and_readback_yuv(
        &self,
        width: u32,
        height: u32,
        format: PixelFormat,
        video_pool: Option<&streamkit_core::VideoFramePool>,
    ) -> PooledVideoData {
        let canvas = self.canvas.as_ref().expect("canvas exists");

        let chroma_w = width.div_ceil(2);
        let chroma_h = height.div_ceil(2);

        // Y buffer: one byte per pixel, rows padded to 4 bytes for u32 packing.
        let y_stride = align_up(width as usize, 4) as u32;
        let y_buf_size = (y_stride as u64) * u64::from(height);
        // Round up to 4 bytes for u32 array.
        let y_buf_size_aligned = align_up(y_buf_size as usize, 4) as u64;

        let y_gpu_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("y_output_buf"),
            size: y_buf_size_aligned,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // UV buffer: for NV12, 2 bytes per chroma sample; for I420, 1 byte per
        // sample but U and V planes stacked vertically (2× chroma_h rows).
        let uv_row_bytes: u32 = if format == PixelFormat::I420 { chroma_w } else { chroma_w * 2 };
        let uv_stride = align_up(uv_row_bytes as usize, 4) as u32;
        let uv_rows: u32 = if format == PixelFormat::I420 { chroma_h * 2 } else { chroma_h };
        let uv_buf_size = (uv_stride as u64) * u64::from(uv_rows);
        let uv_buf_size_aligned = align_up(uv_buf_size as usize, 4) as u64;

        let uv_gpu_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uv_output_buf"),
            size: uv_buf_size_aligned,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let format_id: u32 = if format == PixelFormat::I420 { 1 } else { 0 };
        let params = RgbaToYuvParams {
            width,
            height,
            format: format_id,
            y_stride,
            uv_stride,
            chroma_w,
            chroma_h,
            _pad: 0,
        };
        let params_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rgba_to_yuv_params"),
            size: std::mem::size_of::<RgbaToYuvParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&params_buf, 0, bytemuck::bytes_of(&params));

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rgba_to_yuv_bg"),
            layout: &self.rgba_to_yuv_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &canvas.texture.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry { binding: 1, resource: y_gpu_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: uv_gpu_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: params_buf.as_entire_binding() },
            ],
        });

        // INVARIANT: output buffers MUST be zero-filled before dispatch.
        // The shader uses atomicOr to pack individual bytes into u32 words;
        // if the buffers contain stale data the OR will silently corrupt
        // the output.  Do not remove or reorder this clear.
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("rgba_to_yuv_encoder"),
        });
        encoder.clear_buffer(&y_gpu_buf, 0, None);
        encoder.clear_buffer(&uv_gpu_buf, 0, None);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("rgba_to_yuv_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.rgba_to_yuv_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1);
        }

        // Copy GPU storage buffers to staging buffers for CPU readback.
        // TODO(phase-3): cache these staging buffers on GpuContext (keyed on
        // canvas size + format) instead of creating them every frame — the
        // output format rarely changes between frames.
        let y_staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("y_staging"),
            size: y_buf_size_aligned,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let uv_staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uv_staging"),
            size: uv_buf_size_aligned,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        encoder.copy_buffer_to_buffer(&y_gpu_buf, 0, &y_staging, 0, y_buf_size_aligned);
        encoder.copy_buffer_to_buffer(&uv_gpu_buf, 0, &uv_staging, 0, uv_buf_size_aligned);

        self.queue.submit(std::iter::once(encoder.finish()));

        // Read back Y plane, stripping row-stride padding.
        let y_data =
            self.map_and_read_buffer(&y_staging, width as usize, height as usize, y_stride);
        // Read back UV plane(s), stripping row-stride padding.
        let uv_data = self.map_and_read_buffer(
            &uv_staging,
            uv_row_bytes as usize,
            uv_rows as usize,
            uv_stride,
        );

        // Assemble the final YUV buffer.
        let total = y_data.len() + uv_data.len();
        let mut pooled = video_pool
            .map_or_else(|| PooledVideoData::from_vec(vec![0u8; total]), |pool| pool.get(total));
        let out = pooled.as_mut_slice();
        out[..y_data.len()].copy_from_slice(&y_data);
        out[y_data.len()..y_data.len() + uv_data.len()].copy_from_slice(&uv_data);

        pooled
    }

    /// Map a staging buffer, strip row padding, and return the unpadded data.
    ///
    /// `unpadded_row_bytes` is the number of useful bytes per row.
    /// `padded_row_stride` is the GPU-side stride (>= unpadded_row_bytes).
    fn map_and_read_buffer(
        &self,
        buffer: &wgpu::Buffer,
        unpadded_row_bytes: usize,
        rows: usize,
        padded_row_stride: u32,
    ) -> Vec<u8> {
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = self.device.poll(wgpu::PollType::Wait);
        match rx.recv() {
            Ok(Ok(())) => {},
            Ok(Err(e)) => {
                tracing::error!("GPU buffer mapping failed: {e}");
                return vec![0u8; unpadded_row_bytes * rows];
            },
            Err(e) => {
                tracing::error!("GPU readback channel error: {e}");
                return vec![0u8; unpadded_row_bytes * rows];
            },
        }

        let mapped = slice.get_mapped_range();
        let padded_row = padded_row_stride as usize;
        let total = unpadded_row_bytes * rows;
        let mut data = vec![0u8; total];

        if padded_row == unpadded_row_bytes {
            data.copy_from_slice(&mapped[..total]);
        } else {
            for row in 0..rows {
                let src_off = row * padded_row;
                let dst_off = row * unpadded_row_bytes;
                data[dst_off..dst_off + unpadded_row_bytes]
                    .copy_from_slice(&mapped[src_off..src_off + unpadded_row_bytes]);
            }
        }
        drop(mapped);
        buffer.unmap();

        data
    }

    // ── Texture creation helpers ────────────────────────────────────

    fn create_and_write_r8_texture(
        &self,
        label: &str,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> wgpu::Texture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        texture
    }

    fn create_and_write_rg8_texture(
        &self,
        label: &str,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> wgpu::Texture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 2),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        texture
    }
}

// ── Transform builder ───────────────────────────────────────────────────────

/// Build the 4×4 transform matrix and UV region for a layer.
///
/// The matrix maps a `[-1, 1]` unit quad to the layer's destination
/// position on the canvas in NDC space, applying scale, rotation, mirror,
/// and translation.
// Allow: arguments mirror LayerSnapshot fields 1:1 — wrapping in a struct
// would just add indirection.  Precision loss is inherent in u32 → f32
// canvas/rect conversions (lossless up to 2^24 pixels).
#[allow(clippy::too_many_arguments, clippy::cast_precision_loss)]
fn build_layer_uniforms(
    canvas_w: u32,
    canvas_h: u32,
    dst_rect: &Rect,
    opacity: f32,
    rotation_degrees: f32,
    mirror_h: bool,
    mirror_v: bool,
    crop_zoom: f32,
    crop_x: f32,
    crop_y: f32,
    crop_shape: CropShape,
) -> LayerUniforms {
    let cw = canvas_w as f32;
    let ch = canvas_h as f32;

    // Destination rect in NDC:
    //   x_ndc = (2 * rect.x / canvas_w) - 1
    //   y_ndc = 1 - (2 * rect.y / canvas_h)       (Y flipped for GPU)
    let sx = dst_rect.width as f32 / cw;
    let sy = dst_rect.height as f32 / ch;
    let tx = (2.0 * dst_rect.x as f32 + dst_rect.width as f32) / cw - 1.0;
    let ty = 1.0 - (2.0 * dst_rect.y as f32 + dst_rect.height as f32) / ch;

    // Mirror: flip scale signs.
    let mx: f32 = if mirror_h { -1.0 } else { 1.0 };
    let my: f32 = if mirror_v { -1.0 } else { 1.0 };

    // Rotation (around the quad centre, which is at (tx, ty) in NDC).
    let theta = rotation_degrees.to_radians();
    let cos_t = theta.cos();
    let sin_t = theta.sin();

    // Combined 2D affine in 4×4 (column-major):
    //   Scale(sx*mx, sy*my) → Rotate(theta) → Translate(tx, ty)
    //
    //   | sx*mx*cos  -sy*my*sin  0  tx |
    //   | sx*mx*sin   sy*my*cos  0  ty |
    //   |     0           0      1   0 |
    //   |     0           0      0   1 |
    let transform: [[f32; 4]; 4] = [
        [sx * mx * cos_t, sx * mx * sin_t, 0.0, 0.0], // column 0
        [-sy * my * sin_t, sy * my * cos_t, 0.0, 0.0], // column 1
        [0.0, 0.0, 1.0, 0.0],                         // column 2
        [tx, ty, 0.0, 1.0],                           // column 3
    ];

    // Source UV region for crop/zoom.
    let src_region = if crop_zoom > 1.0 {
        let crop_w = 1.0 / crop_zoom;
        let crop_h = 1.0 / crop_zoom;
        let max_u = 1.0 - crop_w;
        let max_v = 1.0 - crop_h;
        let u_min = (crop_x * max_u).clamp(0.0, max_u);
        let v_min = (crop_y * max_v).clamp(0.0, max_v);
        [u_min, v_min, u_min + crop_w, v_min + crop_h]
    } else {
        [0.0, 0.0, 1.0, 1.0]
    };

    LayerUniforms {
        transform,
        src_region,
        opacity,
        circle_crop: if crop_shape == CropShape::Circle { 1.0 } else { 0.0 },
        _pad: [0.0; 2],
    }
}

// ── Bind group layout helpers ───────────────────────────────────────────────

fn bgl_texture_entry(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn bgl_storage_texture_entry(
    binding: u32,
    format: wgpu::TextureFormat,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn bgl_uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_bg_resource(buffer: &wgpu::Buffer) -> wgpu::BindingResource<'_> {
    buffer.as_entire_binding()
}

// ── Row-padding helpers ─────────────────────────────────────────────────────

/// Compute the padded bytes-per-row for a texture with `bytes_per_pixel` bytes per texel.
///
/// wgpu requires rows to be aligned to [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`].
fn padded_bytes_per_row(width: u32, bytes_per_pixel: u32) -> usize {
    align_up(
        (width as usize) * (bytes_per_pixel as usize),
        wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize,
    )
}

/// Align `value` up to the next multiple of `alignment`.
const fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

// ── GPU mode configuration ──────────────────────────────────────────────────

/// GPU compositing preference parsed from config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuMode {
    /// Use GPU when available and beneficial (default).
    Auto,
    /// Force GPU compositing (log warning and fall back to CPU if unavailable).
    ForceGpu,
    /// Force CPU compositing (ignore GPU even if available).
    ForceCpu,
}

impl GpuMode {
    /// Parse a `gpu_mode` config string.
    pub fn from_config(s: Option<&str>) -> Self {
        match s.map(str::to_lowercase).as_deref() {
            Some("gpu") => Self::ForceGpu,
            Some("cpu") => Self::ForceCpu,
            _ => Self::Auto,
        }
    }
}

/// Decide whether to use GPU compositing for this frame based on scene
/// complexity.  Used when `GpuMode::Auto` is selected.
///
/// GPU wins for: multi-layer, high-resolution, effects (rotation/crop).
/// CPU wins for: single opaque layer at identity scale (memcpy fast path).
///
/// TODO(phase-3): add hysteresis — prefer the same path as last frame
/// unless the scene complexity delta is large, to avoid thrashing between
/// GPU/CPU when the scene oscillates around the threshold.
pub fn should_use_gpu(
    canvas_w: u32,
    canvas_h: u32,
    layers: &[Option<LayerSnapshot>],
    image_overlays: &[Arc<DecodedOverlay>],
    text_overlays: &[Arc<DecodedOverlay>],
) -> bool {
    let visible_layers = layers.iter().filter(|l| l.is_some()).count();
    let total_items = visible_layers + image_overlays.len() + text_overlays.len();
    let total_pixels = u64::from(canvas_w) * u64::from(canvas_h);
    let has_effects = layers.iter().flatten().any(|l| {
        l.rotation_degrees.abs() > 0.01 || l.crop_zoom > 1.01 || l.crop_shape != CropShape::Rect
    });

    // GPU is worthwhile when there's enough work to amortise
    // the upload + readback overhead (~0.5ms for 1080p).
    total_items >= 2 || total_pixels >= 1920 * 1080 || has_effects
}
