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
    /// Destination rect aspect ratio (width / height).
    /// Used by circle crop to inscribe a true circle in the shorter dimension.
    aspect_ratio: f32,
    _pad: f32,
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

/// Cached staging buffers for RGBA→YUV GPU readback.
/// Recreated only when the canvas size or output format changes.
struct YuvStagingCache {
    y_staging: wgpu::Buffer,
    uv_staging: wgpu::Buffer,
    width: u32,
    height: u32,
    format: PixelFormat,
}

// ── Resource pools ──────────────────────────────────────────────────────────

/// Key for texture pool lookup — textures with identical keys are
/// interchangeable and can be reused across frames.
#[derive(Clone, PartialEq, Eq, Hash)]
struct TextureKey {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
}

/// Opaque index into [`TexturePool::in_use`].
///
/// Valid only within the frame it was issued (i.e. between two
/// consecutive `reclaim()` calls).  Using it after `reclaim()` is a
/// logic error — the newtype makes accidental misuse visible at the
/// type level.
#[derive(Clone, Copy)]
struct TextureIdx(usize);

/// Number of `reclaim()` cycles an unused pool entry survives before
/// eviction.  Once an entry's idle counter reaches this value it is
/// dropped on the next `reclaim()` call.
const POOL_IDLE_FRAMES: u32 = 60;

/// Per-frame texture pool.  Textures are "checked out" during a frame
/// and reclaimed at the start of the next frame for reuse.  New
/// textures are only allocated on a cache miss.
///
/// Returns [`TextureIdx`] rather than references so callers can
/// interleave multiple `get` calls without borrow-checker conflicts.
struct TexturePool {
    available: Vec<(TextureKey, wgpu::Texture, u32)>, // (key, tex, idle_frames)
    in_use: Vec<(TextureKey, wgpu::Texture)>,
}

impl TexturePool {
    const fn new() -> Self {
        Self { available: Vec::new(), in_use: Vec::new() }
    }

    /// Get a texture matching `key`, reusing a cached one when possible.
    fn get(&mut self, device: &wgpu::Device, key: TextureKey, label: &str) -> TextureIdx {
        // Search for a reusable texture with matching key.
        if let Some(idx) = self.available.iter().position(|(k, _, _)| *k == key) {
            let (k, tex, _) = self.available.swap_remove(idx);
            self.in_use.push((k, tex));
        } else {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: key.width,
                    height: key.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: key.format,
                usage: key.usage,
                view_formats: &[],
            });
            self.in_use.push((key, texture));
        }
        TextureIdx(self.in_use.len() - 1)
    }

    /// Look up a texture by the index returned from [`get`].
    fn texture(&self, idx: TextureIdx) -> &wgpu::Texture {
        &self.in_use[idx.0].1
    }

    /// Move all in-use textures back to the available pool and evict
    /// entries that have been idle for [`POOL_IDLE_FRAMES`] or more
    /// consecutive reclaim cycles.  Called once at the start of each
    /// frame.
    fn reclaim(&mut self) {
        // Age existing available entries and evict stale ones.
        self.available.retain_mut(|(_, _, idle)| {
            *idle += 1;
            *idle < POOL_IDLE_FRAMES
        });
        // Move in-use back to available with idle counter reset.
        for (k, tex) in self.in_use.drain(..) {
            self.available.push((k, tex, 0));
        }
    }
}

/// Key for buffer pool lookup.
#[derive(Clone, PartialEq, Eq, Hash)]
struct BufferKey {
    size: u64,
    usage: wgpu::BufferUsages,
}

/// Opaque index into [`BufferPool::in_use`], analogous to [`TextureIdx`].
#[derive(Clone, Copy)]
struct BufferIdx(usize);

/// Per-frame buffer pool, analogous to [`TexturePool`].
struct BufferPool {
    available: Vec<(BufferKey, wgpu::Buffer, u32)>, // (key, buf, idle_frames)
    in_use: Vec<(BufferKey, wgpu::Buffer)>,
}

impl BufferPool {
    const fn new() -> Self {
        Self { available: Vec::new(), in_use: Vec::new() }
    }

    /// Get a buffer matching `key`, reusing a cached one when possible.
    fn get(&mut self, device: &wgpu::Device, key: BufferKey, label: &str) -> BufferIdx {
        if let Some(idx) = self.available.iter().position(|(k, _, _)| *k == key) {
            let (k, buf, _) = self.available.swap_remove(idx);
            self.in_use.push((k, buf));
        } else {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: key.size,
                usage: key.usage,
                mapped_at_creation: false,
            });
            self.in_use.push((key, buffer));
        }
        BufferIdx(self.in_use.len() - 1)
    }

    /// Look up a buffer by the index returned from [`get`].
    fn buffer(&self, idx: BufferIdx) -> &wgpu::Buffer {
        &self.in_use[idx.0].1
    }

    /// Move all in-use buffers back to the available pool and evict
    /// entries idle for [`POOL_IDLE_FRAMES`] or more consecutive
    /// reclaim cycles.
    fn reclaim(&mut self) {
        self.available.retain_mut(|(_, _, idle)| {
            *idle += 1;
            *idle < POOL_IDLE_FRAMES
        });
        for (k, buf) in self.in_use.drain(..) {
            self.available.push((k, buf, 0));
        }
    }
}

// ── Per-frame draw types ────────────────────────────────────────────────────

/// A single item in the z-sorted draw list built each frame.
struct DrawItem {
    /// Index into `texture_pool.in_use` (valid this frame only).
    tex_idx: TextureIdx,
    uniforms: LayerUniforms,
    sort_key: (i32, usize),
}

/// Pre-created bind groups for a single draw call.
struct PreparedDraw {
    uniform_bg: wgpu::BindGroup,
    texture_bg: wgpu::BindGroup,
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

    /// Per-frame texture pool — reuses GPU textures across frames.
    texture_pool: TexturePool,

    /// Per-frame buffer pool — reuses GPU buffers across frames.
    buffer_pool: BufferPool,

    /// Cached YUV staging buffers — reused across frames when canvas
    /// size and output format are unchanged.
    yuv_staging: Option<YuvStagingCache>,
}

impl GpuContext {
    /// Attempt to create a GPU context.  Returns `None` if no suitable
    /// adapter is found or device creation fails.
    ///
    /// Uses `pollster::block_on` since this runs on a blocking thread,
    /// not inside a tokio runtime.
    pub fn try_init() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::METAL | wgpu::Backends::DX12,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
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
                    bind_group_layouts: &[Some(&yuv_to_rgba_bgl)],
                    immediate_size: 0,
                })),
                module: &yuv_shader,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
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
                bind_group_layouts: &[Some(&layer_uniforms_bgl), Some(&layer_texture_bgl)],
                immediate_size: 0,
            });

        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite_pipeline"),
            layout: Some(&composite_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
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
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
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
                    bind_group_layouts: &[Some(&rgba_to_yuv_bgl)],
                    immediate_size: 0,
                })),
                module: &rgba_to_yuv_shader,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
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
            texture_pool: TexturePool::new(),
            buffer_pool: BufferPool::new(),
            yuv_staging: None,
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

    /// Ensure YUV staging buffers match the requested dimensions and
    /// format.  Recreates them only when the canvas size or output
    /// pixel format changes.
    fn ensure_yuv_staging(
        &mut self,
        width: u32,
        height: u32,
        format: PixelFormat,
        y_size: u64,
        uv_size: u64,
    ) {
        let needs_recreate = self
            .yuv_staging
            .as_ref()
            .is_none_or(|s| s.width != width || s.height != height || s.format != format);

        if needs_recreate {
            let y_staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("y_staging_cached"),
                size: y_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let uv_staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("uv_staging_cached"),
                size: uv_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            self.yuv_staging =
                Some(YuvStagingCache { y_staging, uv_staging, width, height, format });
        }
    }

    /// Upload an RGBA8 buffer to a pooled GPU texture suitable for sampling.
    /// Returns the texture pool index for later lookup.
    fn upload_rgba_texture(&mut self, data: &[u8], width: u32, height: u32) -> TextureIdx {
        let key = TextureKey {
            width,
            height,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        };
        let idx = self.texture_pool.get(&self.device, key, "layer_rgba");
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: self.texture_pool.texture(idx),
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
        idx
    }

    /// Upload a YUV frame (I420 or NV12) and convert to RGBA8 on the GPU.
    ///
    /// Returns the texture pool index of the RGBA8 output texture.
    /// Appends the YUV→RGBA compute dispatch to `encoder` so the caller
    /// can batch multiple conversions into a single `queue.submit()`.
    fn upload_yuv_layer(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        data: &[u8],
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
    ) -> TextureIdx {
        let (y_tex_idx, uv_tex_idx, format_id) = match pixel_format {
            PixelFormat::Nv12 => {
                let y_size = (width as usize) * (height as usize);
                let y_idx = self.create_and_write_r8_texture(
                    "y_plane_nv12",
                    width,
                    height,
                    &data[..y_size],
                );
                let chroma_w = width.div_ceil(2);
                let chroma_h = height.div_ceil(2);
                let uv_idx = self.create_and_write_rg8_texture(
                    "uv_plane_nv12",
                    chroma_w,
                    chroma_h,
                    &data[y_size..y_size + (chroma_w as usize) * (chroma_h as usize) * 2],
                );
                (y_idx, uv_idx, 0u32)
            },
            PixelFormat::I420 => {
                let y_size = (width as usize) * (height as usize);
                let y_idx = self.create_and_write_r8_texture(
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
                let uv_idx = self.create_and_write_r8_texture(
                    "uv_plane_i420",
                    chroma_w,
                    chroma_h * 2,
                    &packed,
                );
                (y_idx, uv_idx, 1u32)
            },
            _ => unreachable!("upload_yuv_layer called with non-YUV format"),
        };

        // Output RGBA8 texture (written by compute shader).
        let output_key = TextureKey {
            width,
            height,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        };
        let output_idx = self.texture_pool.get(&self.device, output_key, "yuv_to_rgba_output");

        let params_key = BufferKey {
            size: std::mem::size_of::<YuvToRgbaParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        };
        let params_idx = self.buffer_pool.get(&self.device, params_key, "yuv_params");
        self.queue.write_buffer(
            self.buffer_pool.buffer(params_idx),
            0,
            bytemuck::bytes_of(&YuvToRgbaParams { width, height, format: format_id, _pad: 0 }),
        );

        let y_view = self
            .texture_pool
            .texture(y_tex_idx)
            .create_view(&wgpu::TextureViewDescriptor::default());
        let uv_view = self
            .texture_pool
            .texture(uv_tex_idx)
            .create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = self
            .texture_pool
            .texture(output_idx)
            .create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuv_to_rgba_bg"),
            layout: &self.yuv_to_rgba_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&y_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&uv_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&output_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.buffer_pool.buffer(params_idx).as_entire_binding(),
                },
            ],
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

        output_idx
    }

    /// Upload a single layer to a GPU texture.
    /// Returns the texture pool index (after YUV conversion if needed).
    /// YUV conversions are appended to `encoder` for batched submission.
    fn upload_layer_texture(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        data: &[u8],
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
    ) -> TextureIdx {
        match pixel_format {
            PixelFormat::Rgba8 => self.upload_rgba_texture(data, width, height),
            PixelFormat::I420 | PixelFormat::Nv12 => {
                self.upload_yuv_layer(encoder, data, width, height, pixel_format)
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
    /// 1. Upload each visible layer/overlay to a GPU texture (YUV
    ///    conversions are batched into a single `queue.submit()`).
    /// 2. Clear the canvas.
    /// 3. For each item in z-sorted order, draw a textured quad with
    ///    the layer's transform + opacity.
    /// 4. Optionally convert RGBA→YUV on GPU.
    /// 5. Copy output texture → staging buffer → CPU.
    // Allow: GPU compositing coordinates upload → render pass → readback in a
    // single function; splitting would add complexity without improving clarity.
    #[allow(
        clippy::too_many_lines,
        clippy::too_many_arguments,
        clippy::missing_panics_doc,
        clippy::expect_used
    )]
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
        // Reclaim pooled resources from the previous frame.
        self.texture_pool.reclaim();
        self.buffer_pool.reclaim();

        self.ensure_canvas(canvas_w, canvas_h);

        // ── Build z-sorted draw list ────────────────────────────────
        // A single encoder collects all YUV→RGBA compute dispatches so
        // they are submitted in one batch before the render pass.

        let mut yuv_encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("yuv_batch_encoder"),
        });
        let mut has_yuv_work = false;

        let mut items: Vec<DrawItem> =
            Vec::with_capacity(layers.len() + image_overlays.len() + text_overlays.len());
        let mut insertion_order: usize = 0;

        // Video layers.
        for layer in layers.iter().flatten() {
            if matches!(layer.pixel_format, PixelFormat::I420 | PixelFormat::Nv12) {
                has_yuv_work = true;
            }
            let tex_idx = self.upload_layer_texture(
                &mut yuv_encoder,
                layer.data.as_slice(),
                layer.width,
                layer.height,
                layer.pixel_format,
            );
            let dst = layer.rect.unwrap_or(Rect { x: 0, y: 0, width: canvas_w, height: canvas_h });
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
            items.push(DrawItem { tex_idx, uniforms, sort_key: (layer.z_index, insertion_order) });
            insertion_order += 1;
        }

        // Image overlays.
        for ov in image_overlays {
            let tex_idx = self.upload_rgba_texture(&ov.rgba_data, ov.width, ov.height);
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
            items.push(DrawItem { tex_idx, uniforms, sort_key: (ov.z_index, insertion_order) });
            insertion_order += 1;
        }

        // Text overlays.
        for ov in text_overlays {
            let tex_idx = self.upload_rgba_texture(&ov.rgba_data, ov.width, ov.height);
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
            items.push(DrawItem { tex_idx, uniforms, sort_key: (ov.z_index, insertion_order) });
            insertion_order += 1;
        }

        // Stable sort: lower z_index drawn first (bottom).
        items.sort_by_key(|item| item.sort_key);

        // Submit all batched YUV→RGBA conversions before the render pass.
        if has_yuv_work {
            self.queue.submit(std::iter::once(yuv_encoder.finish()));
        }

        // ── Render pass: composite all layers onto the canvas ────────
        let canvas = self.canvas.as_ref().expect("canvas was just ensured");

        // Pre-create per-layer uniform buffers and bind groups so the
        // render pass loop only needs to set them.
        let uniform_buf_size = std::mem::size_of::<LayerUniforms>() as u64;
        let uniform_usage = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;

        let prepared: Vec<PreparedDraw> = items
            .iter()
            .map(|item| {
                let buf_key = BufferKey { size: uniform_buf_size, usage: uniform_usage };
                let buf_idx = self.buffer_pool.get(&self.device, buf_key, "layer_uniform_buf");
                let uniform_buf = self.buffer_pool.buffer(buf_idx);
                self.queue.write_buffer(uniform_buf, 0, bytemuck::bytes_of(&item.uniforms));

                let uniform_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("layer_uniform_bg"),
                    layout: &self.layer_uniforms_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform_buf.as_entire_binding(),
                    }],
                });

                let tex_view = self
                    .texture_pool
                    .texture(item.tex_idx)
                    .create_view(&wgpu::TextureViewDescriptor::default());
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

                PreparedDraw { uniform_bg, texture_bg }
            })
            .collect();

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
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.composite_pipeline);

            for draw in &prepared {
                pass.set_bind_group(0, &draw.uniform_bg, &[]);
                pass.set_bind_group(1, &draw.texture_bg, &[]);
                pass.draw(0..6, 0..1); // 6 vertices = fullscreen quad
            }
        }

        // ── Output conversion or readback ───────────────────────────
        let (output_data, pix_fmt) =
            if let Some(fmt @ (PixelFormat::Nv12 | PixelFormat::I420)) = output_format {
                // Convert RGBA→YUV on GPU, then read back the YUV planes.
                self.queue.submit(std::iter::once(encoder.finish()));
                let yuv_data = self.convert_and_readback_yuv(canvas_w, canvas_h, fmt, video_pool);
                (yuv_data, fmt)
            } else {
                // Read back RGBA8 directly.
                let readback_buf =
                    self.readback_buffer.as_ref().expect("readback buffer was just ensured");
                let padded_row = padded_bytes_per_row(canvas_w, 4);
                #[allow(clippy::cast_possible_truncation)] // padded_row ≤ 8K × 4, fits u32
                let padded_row_u32 = padded_row as u32;
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
                            bytes_per_row: Some(padded_row_u32),
                            rows_per_image: Some(canvas_h),
                        },
                    },
                    wgpu::Extent3d { width: canvas_w, height: canvas_h, depth_or_array_layers: 1 },
                );
                self.queue.submit(std::iter::once(encoder.finish()));

                let data = self.readback_rgba(canvas_w, canvas_h, video_pool);
                (data, PixelFormat::Rgba8)
            };

        (output_data, pix_fmt)
    }

    /// Read back the RGBA8 canvas from the staging buffer into CPU memory.
    #[allow(clippy::expect_used)] // readback_buffer is always set after ensure_canvas
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
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
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
    #[allow(clippy::expect_used, clippy::cast_possible_truncation)] // invariants ensured by callers
    fn convert_and_readback_yuv(
        &mut self,
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
        let y_buf_size = u64::from(y_stride) * u64::from(height);
        // Round up to 4 bytes for u32 array.
        let y_buf_size_aligned = align_up(y_buf_size as usize, 4) as u64;

        let y_buf_key = BufferKey {
            size: y_buf_size_aligned,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        };
        let y_buf_idx = self.buffer_pool.get(&self.device, y_buf_key, "y_output_buf");

        // UV buffer: for NV12, 2 bytes per chroma sample; for I420, 1 byte per
        // sample but U and V planes stacked vertically (2× chroma_h rows).
        let uv_row_bytes: u32 = if format == PixelFormat::I420 { chroma_w } else { chroma_w * 2 };
        let uv_stride = align_up(uv_row_bytes as usize, 4) as u32;
        let uv_rows: u32 = if format == PixelFormat::I420 { chroma_h * 2 } else { chroma_h };
        let uv_buf_size = u64::from(uv_stride) * u64::from(uv_rows);
        let uv_buf_size_aligned = align_up(uv_buf_size as usize, 4) as u64;

        let uv_buf_key = BufferKey {
            size: uv_buf_size_aligned,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        };
        let uv_buf_idx = self.buffer_pool.get(&self.device, uv_buf_key, "uv_output_buf");

        let format_id: u32 = u32::from(format == PixelFormat::I420);
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
        let params_key = BufferKey {
            size: std::mem::size_of::<RgbaToYuvParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        };
        let params_idx = self.buffer_pool.get(&self.device, params_key, "rgba_to_yuv_params");
        self.queue.write_buffer(
            self.buffer_pool.buffer(params_idx),
            0,
            bytemuck::bytes_of(&params),
        );

        let canvas_view = canvas.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rgba_to_yuv_bg"),
            layout: &self.rgba_to_yuv_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&canvas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.buffer_pool.buffer(y_buf_idx).as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.buffer_pool.buffer(uv_buf_idx).as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.buffer_pool.buffer(params_idx).as_entire_binding(),
                },
            ],
        });

        // INVARIANT: output buffers MUST be zero-filled before dispatch.
        // The shader uses atomicOr to pack individual bytes into u32 words;
        // if the buffers contain stale data the OR will silently corrupt
        // the output.  Do not remove or reorder this clear.
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("rgba_to_yuv_encoder"),
        });
        encoder.clear_buffer(self.buffer_pool.buffer(y_buf_idx), 0, None);
        encoder.clear_buffer(self.buffer_pool.buffer(uv_buf_idx), 0, None);
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
        // Staging buffers are cached on GpuContext and only recreated
        // when the canvas size or output format changes.
        self.ensure_yuv_staging(width, height, format, y_buf_size_aligned, uv_buf_size_aligned);
        let staging = self.yuv_staging.as_ref().expect("just ensured");

        encoder.copy_buffer_to_buffer(
            self.buffer_pool.buffer(y_buf_idx),
            0,
            &staging.y_staging,
            0,
            y_buf_size_aligned,
        );
        encoder.copy_buffer_to_buffer(
            self.buffer_pool.buffer(uv_buf_idx),
            0,
            &staging.uv_staging,
            0,
            uv_buf_size_aligned,
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        // Read back Y plane, stripping row-stride padding.
        let staging = self.yuv_staging.as_ref().expect("staging exists");
        let y_data =
            self.map_and_read_buffer(&staging.y_staging, width as usize, height as usize, y_stride);
        let uv_data = self.map_and_read_buffer(
            &staging.uv_staging,
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
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
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
        &mut self,
        label: &str,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> TextureIdx {
        let key = TextureKey {
            width,
            height,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        };
        let idx = self.texture_pool.get(&self.device, key, label);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: self.texture_pool.texture(idx),
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
        idx
    }

    fn create_and_write_rg8_texture(
        &mut self,
        label: &str,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> TextureIdx {
        let key = TextureKey {
            width,
            height,
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        };
        let idx = self.texture_pool.get(&self.device, key, label);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: self.texture_pool.texture(idx),
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
        idx
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
    let tx = 2.0f32.mul_add(dst_rect.x as f32, dst_rect.width as f32) / cw - 1.0;
    let ty = 1.0 - 2.0f32.mul_add(dst_rect.y as f32, dst_rect.height as f32) / ch;

    // Destination rect pixel dimensions (needed for cross-axis rotation
    // terms that account for the canvas aspect ratio).
    let rw = dst_rect.width as f32;
    let rh = dst_rect.height as f32;

    // Mirror: flip scale signs.
    let mx: f32 = if mirror_h { -1.0 } else { 1.0 };
    let my: f32 = if mirror_v { -1.0 } else { 1.0 };

    // Rotation in *pixel space*, then mapped to NDC.
    //
    // The CPU path rotates in screen-pixel coordinates (Y-down) where
    // positive `rotation_degrees` means clockwise.  To match, the GPU
    // must also rotate in pixel space.  Because NDC axes are normalised
    // differently per axis (2/cw horizontally, 2/ch vertically), the
    // off-diagonal (sin) terms use the *cross-axis* canvas dimension:
    //
    //   Decomposition: S_pixel → R_screen → P_ndc → T
    //
    //   S_pixel = diag(rw/2 · mx, −rh/2 · my)   (quad → pixel offset)
    //   R_screen = [cos, −sin; sin, cos]          (CW for +θ in Y-down)
    //   P_ndc = diag(2/cw, −2/ch)                (pixel offset → NDC)
    //   T = translate(tx, ty)
    //
    // Multiplied out the combined matrix is:
    //
    //   | sx·mx·cos    (rh/cw)·my·sin  0  tx |
    //   | −(rw/ch)·mx·sin   sy·my·cos  0  ty |
    //   |      0              0         1   0 |
    //   |      0              0         0   1 |
    let angle_rad = rotation_degrees.to_radians();
    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();

    let transform: [[f32; 4]; 4] = [
        [sx * mx * cos_a, -(rw / ch) * mx * sin_a, 0.0, 0.0], // column 0
        [(rh / cw) * my * sin_a, sy * my * cos_a, 0.0, 0.0],  // column 1
        [0.0, 0.0, 1.0, 0.0],                                 // column 2
        [tx, ty, 0.0, 1.0],                                   // column 3
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

    let aspect_ratio = dst_rect.width as f32 / dst_rect.height.max(1) as f32;

    LayerUniforms {
        transform,
        src_region,
        opacity,
        circle_crop: if crop_shape == CropShape::Circle { 1.0 } else { 0.0 },
        aspect_ratio,
        _pad: 0.0,
    }
}

// ── Bind group layout helpers ───────────────────────────────────────────────

const fn bgl_texture_entry(
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

const fn bgl_storage_texture_entry(
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

const fn bgl_uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
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

// ── Row-padding helpers ─────────────────────────────────────────────────────

/// Compute the padded bytes-per-row for a texture with `bytes_per_pixel` bytes per texel.
///
/// wgpu requires rows to be aligned to [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`].
const fn padded_bytes_per_row(width: u32, bytes_per_pixel: u32) -> usize {
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
#[repr(u8)]
pub enum GpuMode {
    /// Probe for GPU at startup; use it when scene complexity warrants
    /// (multi-layer, high-res, effects).  Simple scenes use CPU.
    Auto = 0,
    /// Force GPU compositing (log warning and fall back to CPU if unavailable).
    ForceGpu = 1,
    /// Force CPU compositing (ignore GPU even if available).
    ForceCpu = 2,
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

    /// Reconstruct from a `u8` stored in an atomic.
    /// Unknown values map to `Auto`.
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::ForceGpu,
            2 => Self::ForceCpu,
            _ => Self::Auto,
        }
    }
}

/// Decide whether to use GPU compositing for this frame based on scene
/// complexity.  Used when `GpuMode::Auto` is selected.
///
/// GPU wins for: multi-layer, high-resolution, effects (rotation/crop),
/// or YUV output (the GPU `rgba_to_yuv.wgsl` shader eliminates the
/// expensive CPU RGBA→NV12/I420 conversion — ~14% of CPU time in
/// profiled pipelines).
/// CPU wins for: single opaque layer at identity scale with RGBA output
/// (memcpy fast path).
pub fn should_use_gpu(
    canvas_w: u32,
    canvas_h: u32,
    layers: &[Option<LayerSnapshot>],
    image_overlays: &[Arc<DecodedOverlay>],
    text_overlays: &[Arc<DecodedOverlay>],
    output_format: Option<PixelFormat>,
) -> bool {
    let visible_layers = layers.iter().filter(|l| l.is_some()).count();
    let total_items = visible_layers + image_overlays.len() + text_overlays.len();
    let total_pixels = u64::from(canvas_w) * u64::from(canvas_h);
    let has_effects = layers.iter().flatten().any(|l| {
        l.rotation_degrees.abs() > 0.01 || l.crop_zoom > 1.01 || l.crop_shape != CropShape::Rect
    });

    // When the output needs YUV (NV12/I420), the GPU path eliminates the
    // CPU RGBA→YUV conversion entirely — the `rgba_to_yuv.wgsl` compute
    // shader handles it on the GPU and the CPU only receives the
    // already-converted buffer from the readback.
    let needs_yuv_output = matches!(output_format, Some(PixelFormat::Nv12 | PixelFormat::I420));

    // GPU is worthwhile when there's enough work to amortise
    // the upload + readback overhead (~0.5ms for 1080p).
    total_items >= 2 || total_pixels >= 1920 * 1080 || has_effects || needs_yuv_output
}

// ── GPU/CPU path hysteresis ─────────────────────────────────────────────────

/// Number of consecutive frames that must vote for the opposite path
/// before the compositor actually switches.  Prevents thrashing when
/// scene complexity oscillates near the GPU/CPU threshold.
const HYSTERESIS_FRAMES: u32 = 5;

/// Hysteresis state for `GpuMode::Auto` path selection.
///
/// Tracks the last path used and counts how many consecutive frames
/// have voted for the opposite path.  The switch only happens after
/// [`HYSTERESIS_FRAMES`] consecutive votes.
pub struct GpuPathState {
    /// `true` when the GPU path was used last frame.
    last_used_gpu: bool,
    /// Number of consecutive frames voting opposite to `last_used_gpu`.
    consecutive_flip_votes: u32,
}

impl GpuPathState {
    /// Create a new state seeded with the initial heuristic evaluation.
    ///
    /// This avoids a [`HYSTERESIS_FRAMES`]-frame warm-up period where
    /// the CPU path would be used even though the scene clearly wants
    /// GPU compositing.
    pub const fn new_seeded(initial_vote_gpu: bool) -> Self {
        Self { last_used_gpu: initial_vote_gpu, consecutive_flip_votes: 0 }
    }
}

/// Wrapper around [`should_use_gpu`] that adds hysteresis.
///
/// The raw heuristic is evaluated each frame, but the path only flips
/// after [`HYSTERESIS_FRAMES`] consecutive frames vote for the other
/// path.  This avoids per-frame GPU↔CPU thrashing when the scene
/// oscillates around the complexity threshold.
pub fn should_use_gpu_with_state(
    state: &mut GpuPathState,
    canvas_w: u32,
    canvas_h: u32,
    layers: &[Option<LayerSnapshot>],
    image_overlays: &[Arc<DecodedOverlay>],
    text_overlays: &[Arc<DecodedOverlay>],
    output_format: Option<PixelFormat>,
) -> bool {
    let vote_gpu =
        should_use_gpu(canvas_w, canvas_h, layers, image_overlays, text_overlays, output_format);

    if vote_gpu == state.last_used_gpu {
        // Same path as last frame — reset the flip counter.
        state.consecutive_flip_votes = 0;
    } else {
        state.consecutive_flip_votes += 1;
        if state.consecutive_flip_votes >= HYSTERESIS_FRAMES {
            // Enough consecutive votes to flip.
            state.last_used_gpu = vote_gpu;
            state.consecutive_flip_votes = 0;
        }
    }

    state.last_used_gpu
}

// ── Pool unit tests ─────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::expect_used, clippy::disallowed_macros, clippy::significant_drop_tightening)]
mod tests {
    use std::sync::{LazyLock, Mutex};

    use super::*;

    /// Shared wgpu device for pool tests.
    ///
    /// Same rationale as the `SHARED_GPU` context in `gpu_tests.rs`:
    /// creating a separate device per test can overwhelm the Vulkan driver
    /// when tests run in parallel.
    static SHARED_DEVICE: LazyLock<Mutex<(wgpu::Device, wgpu::Queue)>> = LazyLock::new(|| {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::METAL | wgpu::Backends::DX12,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter available");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("failed to create device");
        Mutex::new((device, queue))
    });

    /// Simulate a texture pool that grows when many layers are active,
    /// then verify idle entries are evicted after `POOL_IDLE_FRAMES`.
    #[test]
    fn texture_pool_trims_idle_entries() {
        let guard = SHARED_DEVICE.lock().expect("device mutex poisoned");
        let (ref device, _) = *guard;

        let mut pool = TexturePool::new();

        let key = TextureKey {
            width: 64,
            height: 64,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
        };

        // Simulate a burst: allocate 10 textures in one frame.
        for _ in 0..10 {
            pool.get(device, key.clone(), "test");
        }
        assert_eq!(pool.in_use.len(), 10);
        assert_eq!(pool.available.len(), 0);

        // Reclaim — all 10 move to available.
        pool.reclaim();
        assert_eq!(pool.in_use.len(), 0);
        assert_eq!(pool.available.len(), 10);

        // Now only use 2 per frame for POOL_IDLE_FRAMES frames.
        // The remaining entries should be evicted.
        for _ in 0..POOL_IDLE_FRAMES {
            pool.get(device, key.clone(), "test");
            pool.get(device, key.clone(), "test");
            pool.reclaim();
        }

        // After POOL_IDLE_FRAMES reclaim cycles only using 2,
        // the truly idle entries should have been evicted.
        //
        // Note: `swap_remove` in `get()` causes position-0 and
        // the last element to cycle, so up to 3 entries may
        // participate in reuse rather than exactly 2.
        assert!(
            pool.available.len() <= 4,
            "Pool should have trimmed idle entries, got {}",
            pool.available.len()
        );
        assert!(pool.available.len() < 10, "Pool must have evicted at least some idle entries",);
    }

    /// Verify that `BufferPool` also evicts idle entries.
    #[test]
    fn buffer_pool_trims_idle_entries() {
        let guard = SHARED_DEVICE.lock().expect("device mutex poisoned");
        let (ref device, _) = *guard;

        let mut pool = BufferPool::new();

        let key = BufferKey {
            size: 256,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        };

        // Burst: 5 buffers.
        for _ in 0..5 {
            pool.get(device, key.clone(), "test");
        }
        pool.reclaim();
        assert_eq!(pool.available.len(), 5);

        // Use only 1 per frame for enough frames to evict the rest.
        for _ in 0..POOL_IDLE_FRAMES {
            pool.get(device, key.clone(), "test");
            pool.reclaim();
        }

        // swap_remove cycling means up to 2 entries may participate
        // in reuse rather than exactly 1.
        assert!(
            pool.available.len() <= 3,
            "Buffer pool should have trimmed idle entries, got {}",
            pool.available.len()
        );
        assert!(
            pool.available.len() < 5,
            "Buffer pool must have evicted at least some idle entries",
        );
    }
}
