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
  /** Font asset path (e.g. "samples/fonts/system/DejaVuSans.ttf"). */
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

// Snap/drag/resize computation was extracted to compositorResizeHelpers.ts.
// Re-export here for backward compatibility with existing imports.
export { SNAP_GRID, detectSnapGuides, computeUpdatedLayer } from './compositorResizeHelpers';

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
