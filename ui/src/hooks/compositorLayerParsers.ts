// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Types, parsing, serialization, and merge utilities for compositor layer state.
 *
 * Extracted from useCompositorLayers to keep the hook focused on
 * React state management and pointer interaction.
 */

import type {
  ImageOverlayConfig,
  LayerConfig,
  Rect,
  TextOverlayConfig,
} from '@/types/generated/compositor-types';

import {
  DEFAULT_OPACITY,
  DEFAULT_ROTATION_DEGREES,
  DEFAULT_Z_INDEX,
  DEFAULT_MIRROR_HORIZONTAL,
  DEFAULT_MIRROR_VERTICAL,
  DEFAULT_VISIBLE,
  DEFAULT_FONT_SIZE,
  DEFAULT_FONT_NAME,
  DEFAULT_TEXT_COLOR,
  DEFAULT_TEXT_WIDTH,
  DEFAULT_TEXT_HEIGHT,
  DEFAULT_CROP_ZOOM,
  DEFAULT_CROP_X,
  DEFAULT_CROP_Y,
  DEFAULT_CROP_SHAPE,
} from './compositorConstants';

export type { LayerKind } from './compositorConstants';

// ── Public types ────────────────────────────────────────────────────────────

export interface LayerState {
  /** Pin name, e.g. "in_0" */
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
  opacity: number;
  zIndex: number;
  rotationDegrees: number;
  /** Mirror horizontally (flip left ↔ right) */
  mirrorHorizontal: boolean;
  /** Mirror vertically (flip top ↔ bottom) */
  mirrorVertical: boolean;
  /** Client-side visibility toggle (hidden layers send opacity=0 to backend) */
  visible: boolean;
  /** Virtual PTZ crop zoom (1.0 = full source, 2.0 = 2× zoom). Only video layers. */
  cropZoom: number;
  /** Normalised crop pan X (0.0–1.0). Only meaningful when cropZoom > 1.0. */
  cropX: number;
  /** Normalised crop tilt Y (0.0–1.0). Only meaningful when cropZoom > 1.0. */
  cropY: number;
  /** Shape clipping applied to the layer. */
  cropShape: 'rect' | 'circle';
  /** When true, the source is fitted within the rect preserving its native
   *  aspect ratio (letterbox/pillarbox).  When false, the source is stretched
   *  to fill the rect exactly.  Default true. */
  aspectFit: boolean;
  /** True for layers materialized from server view data with no config entry
   *  in params (auto-PiP stubs).  These must NOT be serialized back to the
   *  server — doing so would create explicit config that disables aspect-fit. */
  serverOnly?: boolean;
}

/** A text overlay stored in compositor config */
export interface TextOverlayState {
  /** Stable unique identifier (UUID, assigned by backend or frontend). */
  id: string;
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
  color: [number, number, number, number];
  fontSize: number;
  /** Named font from the server's curated set (e.g. "dejavu-sans"). */
  fontName: string;
  opacity: number;
  rotationDegrees: number;
  zIndex: number;
  /** Mirror horizontally (flip left ↔ right) */
  mirrorHorizontal: boolean;
  /** Mirror vertically (flip top ↔ bottom) */
  mirrorVertical: boolean;
  /** Client-side visibility toggle (hidden overlays send opacity=0 to backend) */
  visible: boolean;
  /** Actual text width measured by the server's font engine. */
  measuredTextWidth?: number;
  /** Actual text height measured by the server's font engine. */
  measuredTextHeight?: number;
}

/** An image overlay stored in compositor config */
export interface ImageOverlayState {
  /** Stable unique identifier (UUID, assigned by backend or frontend). */
  id: string;
  /** Server-relative path to an uploaded image asset (e.g. `samples/images/user/logo.png`). */
  assetPath: string;
  x: number;
  y: number;
  width: number;
  height: number;
  opacity: number;
  rotationDegrees: number;
  zIndex: number;
  /** Mirror horizontally (flip left ↔ right) */
  mirrorHorizontal: boolean;
  /** Mirror vertically (flip top ↔ bottom) */
  mirrorVertical: boolean;
  /** Client-side visibility toggle (hidden overlays send opacity=0 to backend) */
  visible: boolean;
}

/** Which edge/corner is being resized */
export type ResizeHandle = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw';

// ── Constants ───────────────────────────────────────────────────────────────

/** Grid step used when snap-to-grid is active (pixels in canvas space). */
export const SNAP_GRID = 10;
/** Distance threshold for snapping to centre guidelines (pixels). */
export const SNAP_THRESHOLD = 8;

// Wire-format types are generated from Rust via ts-rs.
// See: ui/src/types/generated/compositor-types.ts

/** Common spatial fields shared by all overlay state types (text and image). */
export interface OverlayBase {
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
  opacity: number;
  rotationDegrees: number;
  zIndex: number;
  mirrorHorizontal: boolean;
  mirrorVertical: boolean;
  visible: boolean;
}

// ── Parsing ─────────────────────────────────────────────────────────────────

/** Read a field from either the flat format or the legacy nested "transform:" format. */
function readField<T>(
  raw: Record<string, unknown>,
  t: Record<string, unknown> | undefined,
  key: string,
  fallback: T
): T {
  return (raw[key] as T | undefined) ?? (t?.[key] as T | undefined) ?? fallback;
}

/** Parse spatial transform fields shared by both overlay types.
 *  Supports the flat serde(flatten) format and the legacy nested "transform:" format. */
function parseTransformFields(
  raw: Record<string, unknown>,
  defaults: { width: number; height: number; zIndex: number }
): Omit<OverlayBase, 'id' | 'visible'> {
  const t = raw.transform as Record<string, unknown> | undefined;
  const rect = (raw.rect ?? t?.rect) as Rect | undefined;
  return {
    x: rect?.x ?? 0,
    y: rect?.y ?? 0,
    width: rect?.width ?? defaults.width,
    height: rect?.height ?? defaults.height,
    opacity: readField<number>(raw, t, 'opacity', DEFAULT_OPACITY),
    rotationDegrees: readField<number>(raw, t, 'rotation_degrees', DEFAULT_ROTATION_DEGREES),
    zIndex: readField<number>(raw, t, 'z_index', defaults.zIndex),
    mirrorHorizontal: readField<boolean>(raw, t, 'mirror_horizontal', DEFAULT_MIRROR_HORIZONTAL),
    mirrorVertical: readField<boolean>(raw, t, 'mirror_vertical', DEFAULT_MIRROR_VERTICAL),
  };
}

/** Extract position/size from a layer config's rect, falling back to canvas dimensions. */
function parseLayerRect(rect: Rect | null | undefined, canvasWidth: number, canvasHeight: number) {
  return {
    x: rect?.x ?? 0,
    y: rect?.y ?? 0,
    width: rect?.width ?? canvasWidth,
    height: rect?.height ?? canvasHeight,
  };
}

/** Map a single layer config entry to LayerState. */
function layerConfigToState(
  id: string,
  cfg: LayerConfig,
  canvasWidth: number,
  canvasHeight: number
): LayerState {
  return {
    id,
    ...parseLayerRect(cfg.rect, canvasWidth, canvasHeight),
    opacity: cfg.opacity ?? DEFAULT_OPACITY,
    zIndex: cfg.z_index ?? DEFAULT_Z_INDEX,
    rotationDegrees: cfg.rotation_degrees ?? DEFAULT_ROTATION_DEGREES,
    mirrorHorizontal: cfg.mirror_horizontal ?? DEFAULT_MIRROR_HORIZONTAL,
    mirrorVertical: cfg.mirror_vertical ?? DEFAULT_MIRROR_VERTICAL,
    visible: DEFAULT_VISIBLE,
    cropZoom: cfg.crop_zoom ?? DEFAULT_CROP_ZOOM,
    cropX: cfg.crop_x ?? DEFAULT_CROP_X,
    cropY: cfg.crop_y ?? DEFAULT_CROP_Y,
    cropShape: cfg.crop_shape ?? DEFAULT_CROP_SHAPE,
    aspectFit: cfg.aspect_fit ?? true,
  };
}

/** Parse layers from compositor params into LayerState array */
export function parseLayers(
  params: Record<string, unknown>,
  canvasWidth: number,
  canvasHeight: number
): LayerState[] {
  const layers = params.layers as Record<string, LayerConfig> | undefined;
  if (!layers || typeof layers !== 'object') return [];

  return Object.entries(layers)
    .map(([id, cfg]) => layerConfigToState(id, cfg, canvasWidth, canvasHeight))
    .sort((a, b) => a.zIndex - b.zIndex);
}

/** Parse text overlays from compositor params */
export function parseTextOverlays(params: Record<string, unknown>): TextOverlayState[] {
  const overlays = params.text_overlays as TextOverlayConfig[] | undefined;
  if (!Array.isArray(overlays)) return [];
  return overlays.map((o, i) => ({
    id: o.id ?? `text_${i}`,
    text: o.text ?? '',
    color: o.color ?? DEFAULT_TEXT_COLOR,
    fontSize: o.font_size ?? DEFAULT_FONT_SIZE,
    fontName: o.font_name ?? DEFAULT_FONT_NAME,
    ...parseTransformFields(o as unknown as Record<string, unknown>, {
      width: DEFAULT_TEXT_WIDTH,
      height: DEFAULT_TEXT_HEIGHT,
      zIndex: 100 + i,
    }),
    visible: DEFAULT_VISIBLE,
  }));
}

/** Parse image overlays from compositor params.
 *  Z-index band: image overlays default to 200+i (video: 0–99, text: 100–199,
 *  image: 200+). */
export function parseImageOverlays(params: Record<string, unknown>): ImageOverlayState[] {
  const overlays = params.image_overlays as ImageOverlayConfig[] | undefined;
  if (!Array.isArray(overlays)) return [];
  return overlays.map((o, i) => {
    const raw = o as Record<string, unknown>;
    return {
      id: o.id ?? `img_${i}`,
      assetPath: o.asset_path ?? '',
      ...parseTransformFields(raw, {
        width: 200,
        height: 200,
        zIndex: 200 + i,
      }),
      visible: DEFAULT_VISIBLE,
    };
  });
}

// ── Serialization ───────────────────────────────────────────────────────────

/** Round and clamp a layer's rect for the wire format. */
function serializeRect(o: OverlayBase): Rect {
  return {
    x: Math.round(o.x),
    y: Math.round(o.y),
    width: Math.max(1, Math.round(o.width)),
    height: Math.max(1, Math.round(o.height)),
  };
}

/** Serialize the spatial fields shared by all layer/overlay types. */
function serializeSpatialFields(o: OverlayBase) {
  return {
    opacity: o.visible ? Math.round(o.opacity * 100) / 100 : 0,
    rotation_degrees: Math.round(o.rotationDegrees * 10) / 10,
    z_index: o.zIndex,
    mirror_horizontal: o.mirrorHorizontal,
    mirror_vertical: o.mirrorVertical,
  };
}

/** Serialize text overlays back to config format */
export function serializeTextOverlays(
  overlays: TextOverlayState[]
): Omit<TextOverlayConfig, 'transform'>[] {
  return overlays.map((o) => ({
    id: o.id,
    text: o.text,
    rect: serializeRect(o),
    color: o.color,
    font_size: o.fontSize,
    font_name: o.fontName,
    ...serializeSpatialFields(o),
  }));
}

/** Serialize image overlays back to config format. */
export function serializeImageOverlays(overlays: ImageOverlayState[]): {
  id: string;
  asset_path: string;
  rect: Rect;
  opacity: number;
  rotation_degrees: number;
  z_index: number;
  mirror_horizontal: boolean;
  mirror_vertical: boolean;
}[] {
  return overlays.map((o) => ({
    id: o.id,
    asset_path: o.assetPath,
    rect: serializeRect(o),
    ...serializeSpatialFields(o),
  }));
}

/** Serialize video layers to the wire format used by the backend. */
export function serializeLayers(layers: LayerState[]): Record<string, LayerConfig> {
  const layersMap: Record<string, LayerConfig> = {};
  for (const layer of layers) {
    // Skip server-only layers (auto-PiP stubs) — serializing them would
    // create explicit config that disables aspect-fit on the server.
    if (layer.serverOnly) continue;
    layersMap[layer.id] = {
      rect: serializeRect(layer),
      aspect_fit: layer.aspectFit,
      ...serializeSpatialFields(layer),
      crop_zoom: Math.round(layer.cropZoom * 100) / 100,
      crop_x: Math.round(layer.cropX * 100) / 100,
      crop_y: Math.round(layer.cropY * 100) / 100,
      crop_shape: layer.cropShape,
    };
  }
  return layersMap;
}

// ── Merge / diff ────────────────────────────────────────────────────────────

/** Keys that belong to OverlayBase and are resolved by the server.
 *  Used by `mergeOverlayState` to separate server-owned fields from
 *  type-specific config fields (text, fontSize, assetPath, etc.). */
const OVERLAY_BASE_KEYS: ReadonlySet<string> = new Set([
  'id',
  'x',
  'y',
  'width',
  'height',
  'opacity',
  'rotationDegrees',
  'zIndex',
  'mirrorHorizontal',
  'mirrorVertical',
  'visible',
]);

/** Extract type-specific config fields from a parsed overlay by removing
 *  all OverlayBase keys.  The result contains only fields like `text`,
 *  `fontSize`, `fontName`, `color`, `assetPath`, etc. */
function pickConfigFields<T extends OverlayBase>(parsed: T): Partial<T> {
  const config: Record<string, unknown> = {};
  for (const key of Object.keys(parsed)) {
    if (!OVERLAY_BASE_KEYS.has(key)) {
      config[key] = (parsed as Record<string, unknown>)[key];
    }
  }
  return config as Partial<T>;
}

/** Pick only config fields that actually changed between two parsed overlays.
 *  Returns only keys where the value in `newParsed` differs from `prevParsed`.
 *  This prevents topology rebuilds (which re-parse potentially stale params)
 *  from overwriting local inspector edits (color, fontSize, etc.) when the
 *  parsed params haven't actually changed. */
function pickChangedConfigFields<T extends OverlayBase>(
  newParsed: T,
  prevParsed: T | undefined
): Partial<T> {
  if (!prevParsed) return pickConfigFields(newParsed);
  const diff: Record<string, unknown> = {};
  const newRec = newParsed as Record<string, unknown>;
  const prevRec = prevParsed as Record<string, unknown>;
  for (const key of Object.keys(newRec)) {
    if (OVERLAY_BASE_KEYS.has(key)) continue;
    const nv = newRec[key];
    const pv = prevRec[key];
    if (Array.isArray(nv) && Array.isArray(pv)) {
      if (nv.length !== pv.length || nv.some((v, i) => v !== pv[i])) {
        diff[key] = nv;
      }
    } else if (nv !== pv) {
      diff[key] = nv;
    }
  }
  return diff as Partial<T>;
}

/** Merge parsed overlays with existing state, preserving client-side visibility.
 *  Returns the same array reference if nothing changed (avoiding re-renders).
 *  An optional `hasExtraChanges` comparator can detect changes in type-specific
 *  fields (e.g. `text`, `fontSize` for text overlays).
 *
 *  When `preserveGeometry` is true (Monitor view), ALL server-resolved fields
 *  are kept from `current` — not just positions but also opacity, rotation,
 *  z-index, mirror flags, and any runtime-only fields (e.g. measuredTextWidth).
 *  Only type-specific config fields (text, fontSize, color, assetPath, …) are
 *  taken from `parsed`.  This prevents config-derived values from clobbering the
 *  server's resolved layout that useServerLayoutSync applied.
 *
 *  When `previousParsed` is provided (Monitor view), only config fields that
 *  actually changed between the previous and new parsed data are applied.  This
 *  prevents topology rebuilds with stale params from overwriting inspector
 *  edits (e.g. color alpha changes). */
export function mergeOverlayState<T extends OverlayBase>(
  current: T[],
  parsed: T[],
  hasExtraChanges?: (a: T, b: T) => boolean,
  preserveGeometry?: boolean,
  previousParsed?: T[]
): T[] {
  const merged = parsed.map((p) => {
    const existing = current.find((o) => o.id === p.id);
    if (existing) {
      if (preserveGeometry) {
        // Monitor view: server is the source of truth for all OverlayBase
        // fields.  Start from `existing` (preserves server-resolved spatial
        // values AND runtime-only fields like measuredTextWidth), then
        // overlay only type-specific config fields that actually changed
        // in the parsed params.
        const prev = previousParsed?.find((o) => o.id === p.id);
        const configDiff = pickChangedConfigFields(p, prev);
        // No config fields changed → return the same reference to preserve
        // referential equality and avoid unnecessary atom writes / re-renders.
        if (Object.keys(configDiff).length === 0) return existing;
        return { ...existing, ...configDiff } as T;
      }
      return {
        ...p,
        visible: existing.visible,
        opacity: existing.visible ? p.opacity : existing.opacity,
      };
    }
    return p;
  });
  const changed =
    merged.length !== current.length ||
    merged.some(
      (m, i) =>
        m.id !== current[i].id ||
        m.x !== current[i].x ||
        m.y !== current[i].y ||
        m.width !== current[i].width ||
        m.height !== current[i].height ||
        m.opacity !== current[i].opacity ||
        m.rotationDegrees !== current[i].rotationDegrees ||
        m.zIndex !== current[i].zIndex ||
        m.mirrorHorizontal !== current[i].mirrorHorizontal ||
        m.mirrorVertical !== current[i].mirrorVertical ||
        m.visible !== current[i].visible ||
        (hasExtraChanges ? hasExtraChanges(m, current[i]) : false)
    );
  return changed ? merged : current;
}

/** Build the full compositor config from current params + updated layers.
 *  Spreads existing params first so fields the UI doesn't manage
 *  (fps, num_inputs, etc.) are preserved across round-trips. */
export function buildConfig(
  params: Record<string, unknown>,
  layers: LayerState[],
  textOverlays?: TextOverlayState[],
  imageOverlays?: ImageOverlayState[]
): Record<string, unknown> {
  return {
    ...params,
    width: params.width ?? 1280,
    height: params.height ?? 720,
    layers: serializeLayers(layers),
    image_overlays: imageOverlays
      ? serializeImageOverlays(imageOverlays)
      : (params.image_overlays ?? []),
    text_overlays: textOverlays
      ? serializeTextOverlays(textOverlays)
      : (params.text_overlays ?? []),
  };
}

// ── Aspect-fit prediction ───────────────────────────────────────────────────

/** Compute a destination rect that fits `srcW × srcH` within `bounds`
 *  while preserving the source aspect ratio.  The fitted rect is centred
 *  within the bounds.
 *
 *  Port of Rust `fit_rect_preserving_aspect` (mod.rs:249-266).
 *  JS `Math.round()` and Rust `.round()` produce matching results for
 *  non-negative values, which is all this function rounds. */
export function fitRectPreservingAspect(
  srcW: number,
  srcH: number,
  bounds: { x: number; y: number; width: number; height: number }
): { x: number; y: number; width: number; height: number } {
  if (srcW === 0 || srcH === 0 || bounds.width === 0 || bounds.height === 0) {
    return { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height };
  }
  const scaleW = bounds.width / srcW;
  const scaleH = bounds.height / srcH;
  const scale = Math.min(scaleW, scaleH);
  const fitW = Math.round(srcW * scale);
  const fitH = Math.round(srcH * scale);
  // Centre within the bounding rect.
  // Use integer division (Math.floor on the half-difference) to match
  // Rust's saturating_sub / 2 behaviour for non-negative values.
  const offsetX = Math.floor((bounds.width - fitW) / 2);
  const offsetY = Math.floor((bounds.height - fitH) / 2);
  return { x: bounds.x + offsetX, y: bounds.y + offsetY, width: fitW, height: fitH };
}

// ── Snap guide detection ────────────────────────────────────────────────────

/** Which snap guides are currently active during a drag. */
export interface SnapGuides {
  /** Horizontal centre line (layer centred vertically on canvas). */
  horizontalCenter: boolean;
  /** Vertical centre line (layer centred horizontally on canvas). */
  verticalCenter: boolean;
  /** Layer left edge aligned with canvas left edge (x ≈ 0). */
  leftEdge: boolean;
  /** Layer right edge aligned with canvas right edge (x + w ≈ canvasWidth). */
  rightEdge: boolean;
  /** Layer top edge aligned with canvas top edge (y ≈ 0). */
  topEdge: boolean;
  /** Layer bottom edge aligned with canvas bottom edge (y + h ≈ canvasHeight). */
  bottomEdge: boolean;
}

/** Determine which snap guides are active for a given layer position.
 *  Mirrors the snapping logic in `computeDragPosition` — a guide is shown
 *  whenever the layer snaps to a canvas centre axis or edge. */
export function detectSnapGuides(
  layer: LayerState,
  canvasWidth: number,
  canvasHeight: number
): SnapGuides {
  const midX = layer.x + layer.width / 2;
  const midY = layer.y + layer.height / 2;
  return {
    verticalCenter: Math.abs(midX - canvasWidth / 2) < 1,
    horizontalCenter: Math.abs(midY - canvasHeight / 2) < 1,
    leftEdge: Math.abs(layer.x) < 1,
    rightEdge: Math.abs(layer.x + layer.width - canvasWidth) < 1,
    topEdge: Math.abs(layer.y) < 1,
    bottomEdge: Math.abs(layer.y + layer.height - canvasHeight) < 1,
  };
}

// ── Drag / resize computation ───────────────────────────────────────────────

/** Compute the updated layer position/size from a drag or resize interaction.
 *  Pure function — no React dependencies. */
export function computeUpdatedLayer(
  orig: LayerState,
  type: 'drag' | 'resize',
  handle: ResizeHandle | undefined,
  rawDx: number,
  rawDy: number,
  canvasWidth: number,
  canvasHeight: number
): LayerState {
  if (type === 'drag') {
    return computeDragPosition(orig, rawDx, rawDy, canvasWidth, canvasHeight);
  }
  return computeResizePosition(orig, handle!, rawDx, rawDy, canvasWidth, canvasHeight);
}

function computeDragPosition(
  orig: LayerState,
  rawDx: number,
  rawDy: number,
  canvasWidth: number,
  canvasHeight: number
): LayerState {
  let nx = orig.x + rawDx;
  let ny = orig.y + rawDy;

  // Only apply snapping when the pointer actually moved
  if (rawDx !== 0 || rawDy !== 0) {
    nx = Math.round(nx / SNAP_GRID) * SNAP_GRID;
    ny = Math.round(ny / SNAP_GRID) * SNAP_GRID;

    // Snap to canvas edges (layer edges → canvas boundaries)
    if (Math.abs(nx) < SNAP_THRESHOLD) {
      nx = 0;
    } else if (Math.abs(nx + orig.width - canvasWidth) < SNAP_THRESHOLD) {
      nx = canvasWidth - orig.width;
    }
    if (Math.abs(ny) < SNAP_THRESHOLD) {
      ny = 0;
    } else if (Math.abs(ny + orig.height - canvasHeight) < SNAP_THRESHOLD) {
      ny = canvasHeight - orig.height;
    }

    // Snap to canvas centre (layer midpoint → canvas midpoint)
    const midX = nx + orig.width / 2;
    const midY = ny + orig.height / 2;
    if (Math.abs(midX - canvasWidth / 2) < SNAP_THRESHOLD) {
      nx = (canvasWidth - orig.width) / 2;
    }
    if (Math.abs(midY - canvasHeight / 2) < SNAP_THRESHOLD) {
      ny = (canvasHeight - orig.height) / 2;
    }
  }

  return { ...orig, x: nx, y: ny };
}

function computeResizePosition(
  orig: LayerState,
  handle: ResizeHandle,
  rawDx: number,
  rawDy: number,
  canvasWidth: number,
  canvasHeight: number
): LayerState {
  // Transform mouse delta into the layer's local coordinate system so
  // resize handles behave naturally on rotated layers.
  let dx = rawDx;
  let dy = rawDy;
  if (orig.rotationDegrees !== 0) {
    const rad = (-orig.rotationDegrees * Math.PI) / 180;
    const cos = Math.cos(rad);
    const sin = Math.sin(rad);
    dx = rawDx * cos - rawDy * sin;
    dy = rawDx * sin + rawDy * cos;
  }

  let newX = orig.x;
  let newY = orig.y;
  let newW = orig.width;
  let newH = orig.height;

  if (handle.includes('e')) newW = Math.max(20, orig.width + dx);
  if (handle.includes('w')) {
    newW = Math.max(20, orig.width - dx);
    newX = orig.x + (orig.width - newW);
  }
  if (handle.includes('s')) newH = Math.max(20, orig.height + dy);
  if (handle.includes('n')) {
    newH = Math.max(20, orig.height - dy);
    newY = orig.y + (orig.height - newH);
  }

  // Constrain resize to maintain aspect ratio for all layer types.
  if (orig.width > 0 && orig.height > 0) {
    const result = constrainAspectRatio(orig, handle, newW, newH);
    newW = result.width;
    newH = result.height;

    if (handle.includes('w')) newX = orig.x + (orig.width - newW);
    if (handle.includes('n')) newY = orig.y + (orig.height - newH);
  }

  // Snap resize edges to canvas boundaries, re-applying the aspect ratio
  // constraint after each snap so the layer isn't distorted.
  // For corner handles, only snap the axis closest to a boundary to avoid
  // the second snap's AR correction undoing the first snap's alignment.
  const ar = orig.width > 0 && orig.height > 0 ? orig.width / orig.height : 0;

  let snappedH = false; // true once an east/west snap has been applied
  let snappedV = false; // true once a north/south snap has been applied

  const deltaE = handle.includes('e') ? Math.abs(newX + newW - canvasWidth) : Infinity;
  const deltaW = handle.includes('w') ? Math.abs(newX) : Infinity;
  const deltaS = handle.includes('s') ? Math.abs(newY + newH - canvasHeight) : Infinity;
  const deltaN = handle.includes('n') ? Math.abs(newY) : Infinity;

  // For corner handles, only snap the closer axis to avoid conflicts.
  const isCorner =
    (handle.includes('n') || handle.includes('s')) &&
    (handle.includes('e') || handle.includes('w'));
  const bestH = Math.min(deltaE, deltaW);
  const bestV = Math.min(deltaN, deltaS);
  const skipH = isCorner && bestH < SNAP_THRESHOLD && bestV < SNAP_THRESHOLD && bestH > bestV;
  const skipV = isCorner && bestH < SNAP_THRESHOLD && bestV < SNAP_THRESHOLD && bestV > bestH;

  if (!skipH && deltaE < SNAP_THRESHOLD) {
    newW = canvasWidth - newX;
    if (ar > 0) newH = newW / ar;
    if (handle.includes('n')) newY = orig.y + (orig.height - newH);
    snappedH = true;
  }
  if (!skipH && !snappedH && deltaW < SNAP_THRESHOLD) {
    newW += newX;
    newX = 0;
    if (ar > 0) newH = newW / ar;
    if (handle.includes('n')) newY = orig.y + (orig.height - newH);
    snappedH = true;
  }
  if (!skipV && deltaS < SNAP_THRESHOLD && !snappedH) {
    newH = canvasHeight - newY;
    if (ar > 0) newW = newH * ar;
    if (handle.includes('w')) newX = orig.x + (orig.width - newW);
    snappedV = true;
  }
  if (!skipV && !snappedV && deltaN < SNAP_THRESHOLD && !snappedH) {
    newH += newY;
    newY = 0;
    if (ar > 0) newW = newH * ar;
    if (handle.includes('w')) newX = orig.x + (orig.width - newW);
  }

  return { ...orig, x: newX, y: newY, width: newW, height: newH };
}

function constrainAspectRatio(
  orig: LayerState,
  handle: ResizeHandle,
  rawW: number,
  rawH: number
): { width: number; height: number } {
  const ar = orig.width / orig.height;
  const isCorner = handle.length === 2;
  let newW = rawW;
  let newH = rawH;

  if (isCorner) {
    const dw = Math.abs(newW - orig.width);
    const dh = Math.abs(newH - orig.height);
    if (dw >= dh) {
      newH = newW / ar;
    } else {
      newW = newH * ar;
    }
  } else if (handle === 'e' || handle === 'w') {
    newH = newW / ar;
  } else {
    newW = newH * ar;
  }

  return {
    width: Math.max(20, newW),
    height: Math.max(20, newH),
  };
}
