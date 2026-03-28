// This file is auto-generated. Do not edit it manually.

export type Rect = { x: number, y: number, width: number, height: number, };

export type CropShape = "rect" | "circle";

export type OverlayTransform = { 
/**
 * Destination rectangle on the output canvas.
 */
rect: Rect, 
/**
 * Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque).
 */
opacity: number, 
/**
 * Clockwise rotation in degrees around the rect centre.  Default 0.0.
 */
rotation_degrees: number, 
/**
 * Visual stacking order.  Lower values are drawn first (bottom);
 * higher values are drawn on top.  Default 0.
 */
z_index: number, 
/**
 * Mirror the layer horizontally (flip left ↔ right).  Default `false`.
 */
mirror_horizontal: boolean, 
/**
 * Mirror the layer vertically (flip top ↔ bottom).  Default `false`.
 */
mirror_vertical: boolean, };

export type ImageOverlayConfig = { 
/**
 * Stable unique identifier.  Auto-generated (UUID v4) when omitted.
 */
id: string, 
/**
 * Server-relative path to an uploaded image asset
 * (e.g. `samples/images/user/logo.png`).
 */
asset_path: string, 
/**
 * Destination rectangle on the output canvas.
 */
rect: Rect, 
/**
 * Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque).
 */
opacity: number, 
/**
 * Clockwise rotation in degrees around the rect centre.  Default 0.0.
 */
rotation_degrees: number, 
/**
 * Visual stacking order.  Lower values are drawn first (bottom);
 * higher values are drawn on top.  Default 0.
 */
z_index: number, 
/**
 * Mirror the layer horizontally (flip left ↔ right).  Default `false`.
 */
mirror_horizontal: boolean, 
/**
 * Mirror the layer vertically (flip top ↔ bottom).  Default `false`.
 */
mirror_vertical: boolean, };

export type TextOverlayConfig = { 
/**
 * Stable unique identifier.  Auto-generated (UUID v4) when omitted.
 */
id: string, 
/**
 * The text string to render.
 */
text: string, 
/**
 * RGBA colour, e.g. `[255, 255, 255, 255]`.
 */
color: [number, number, number, number], 
/**
 * Font size in pixels.
 */
font_size: number, 
/**
 * Font identifier: a font asset path under `samples/fonts/`, e.g.
 * `"samples/fonts/system/DejaVuSans.ttf"` or
 * `"samples/fonts/system/Inter.ttf"`.
 *
 * Font assets are TTF/OTF files managed via the `/api/v1/assets/fonts`
 * REST API and stored under `samples/fonts/{system,user}/`.
 *
 * When omitted, the default system font (DejaVu Sans) is used.
 */
font_name: string | null, 
/**
 * Destination rectangle on the output canvas.
 */
rect: Rect, 
/**
 * Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque).
 */
opacity: number, 
/**
 * Clockwise rotation in degrees around the rect centre.  Default 0.0.
 */
rotation_degrees: number, 
/**
 * Visual stacking order.  Lower values are drawn first (bottom);
 * higher values are drawn on top.  Default 0.
 */
z_index: number, 
/**
 * Mirror the layer horizontally (flip left ↔ right).  Default `false`.
 */
mirror_horizontal: boolean, 
/**
 * Mirror the layer vertically (flip top ↔ bottom).  Default `false`.
 */
mirror_vertical: boolean, };

export type LayerConfig = { 
/**
 * Destination rectangle on the output canvas. If `None`, the input is
 * scaled to fill the entire canvas.
 */
rect: Rect | null, 
/**
 * When `true` (the default), the source is fitted within the
 * destination rect while preserving its native aspect ratio
 * (letterbox / pillarbox).  Set to `false` to stretch the source
 * to fill the rect exactly.
 */
aspect_fit: boolean, 
/**
 * Opacity (0.0 .. 1.0). Default 1.0.
 */
opacity: number, 
/**
 * Visual stacking order.  Lower values are drawn first (bottom);
 * higher values are drawn on top.  Ties are broken by slot index
 * (pin insertion order).  Default 0.
 */
z_index: number, 
/**
 * Clockwise rotation in degrees.  Default 0.0 (no rotation).
 * The layer is rotated around its destination rect centre.
 */
rotation_degrees: number, 
/**
 * Mirror the layer horizontally (flip left ↔ right).  Default `false`.
 */
mirror_horizontal: boolean, 
/**
 * Mirror the layer vertically (flip top ↔ bottom).  Default `false`.
 */
mirror_vertical: boolean, 
/**
 * Zoom factor for virtual PTZ crop (1.0 = full source, 2.0 = 2× zoom
 * showing the central 50% of the source).  Default 1.0.
 */
crop_zoom: number, 
/**
 * Normalized horizontal pan position for the crop window
 * (0.0 = left edge, 0.5 = centred, 1.0 = right edge).  Only has a
 * visible effect when `crop_zoom > 1.0`.  Default 0.5.
 */
crop_x: number, 
/**
 * Normalized vertical tilt position for the crop window
 * (0.0 = top edge, 0.5 = centred, 1.0 = bottom edge).  Only has a
 * visible effect when `crop_zoom > 1.0`.  Default 0.5.
 */
crop_y: number, 
/**
 * Shape clipping applied to the layer.  Default `Rect` (no clipping).
 * Set to `Circle` for Loom-style circular webcam PIP overlays.
 */
crop_shape: CropShape, };

export type CompositorConfig = { 
/**
 * Output canvas width in pixels.
 */
width: number, 
/**
 * Output canvas height in pixels.
 */
height: number, 
/**
 * Output frame rate.  The compositor ticks at this fixed rate
 * regardless of input frame rates, compositing with the latest
 * available frame from each input.
 */
fps: number, 
/**
 * Number of input pins to pre-create.
 * Required for stateless/oneshot pipelines where pins must exist before
 * graph building. Optional for dynamic pipelines where pins are created
 * on-demand. If specified, pins will be named in_0, in_1, ..., in_{N-1}.
 */
num_inputs: number | null, 
/**
 * Per-layer configuration, keyed by pin name (e.g. `"in_0"`).
 * Layers without an entry here are scaled to fill the canvas.
 */
layers: { [key in string]: LayerConfig }, 
/**
 * Static image overlays (decoded once during init).
 */
image_overlays: Array<ImageOverlayConfig>, 
/**
 * Text overlays (rasterized once per `UpdateParams`).
 */
text_overlays: Array<TextOverlayConfig>, };

export type ResolvedLayer = { 
/**
 * Pin name (e.g. `"in_0"`).
 */
id: string, x: number, y: number, width: number, height: number, 
/**
 * Source frame width (from the input slot's latest frame).
 * The client uses this to compute aspect-fit locally for zero-latency
 * feedback on auto-PiP layers.
 * `None` when no frame has been received yet for this input.
 */
source_width: number | null, 
/**
 * Source frame height (from the input slot's latest frame).
 * `None` when no frame has been received yet for this input.
 */
source_height: number | null, };

export type ResolvedOverlay = { 
/**
 * Stable overlay identifier (matches the config `id` field).
 */
id: string, x: number, y: number, 
/**
 * Width after text measurement / image aspect-fit (may differ from
 * the config rect when content doesn't fill it exactly).
 */
width: number, 
/**
 * Height after text measurement / image aspect-fit.
 */
height: number, 
/**
 * Actual text width measured by the font engine (text overlays only).
 */
measured_text_width: number | null, 
/**
 * Actual text height measured by the font engine (text overlays only).
 */
measured_text_height: number | null, };

export type CompositorLayout = { canvas_width: number, canvas_height: number, layers: Array<ResolvedLayer>, text_overlays: Array<ResolvedOverlay>, image_overlays: Array<ResolvedOverlay>, };
