// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Types, parsing, serialization, and merge utilities for compositor layer state.
 *
 * Extracted from useCompositorLayers to keep the hook focused on
 * React state management and pointer interaction.
 */

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
  /** Base64-encoded image data */
  dataBase64: string;
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

/** Which category a layer belongs to for drag commit routing */
export type LayerKind = 'video' | 'text' | 'image';

// ── Constants ───────────────────────────────────────────────────────────────

/** Grid step used when snap-to-grid is active (pixels in canvas space). */
export const SNAP_GRID = 10;
/** Distance threshold for snapping to centre guidelines (pixels). */
export const SNAP_THRESHOLD = 8;

// ── Wire-format interfaces (internal) ───────────────────────────────────────

interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface LayerConfig {
  rect?: Rect;
  opacity?: number;
  z_index?: number;
  rotation_degrees?: number;
  mirror_horizontal?: boolean;
  mirror_vertical?: boolean;
}

interface TextOverlayConfig {
  id: string;
  text: string;
  rect: Rect;
  color?: [number, number, number, number];
  font_size?: number;
  font_name?: string;
  opacity?: number;
  rotation_degrees?: number;
  z_index?: number;
  mirror_horizontal?: boolean;
  mirror_vertical?: boolean;
}

interface ImageOverlayConfig {
  id: string;
  data_base64: string;
  rect: Rect;
  opacity?: number;
  rotation_degrees?: number;
  z_index?: number;
  mirror_horizontal?: boolean;
  mirror_vertical?: boolean;
}

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
    opacity: readField<number>(raw, t, 'opacity', 1.0),
    rotationDegrees: readField<number>(raw, t, 'rotation_degrees', 0),
    zIndex: readField<number>(raw, t, 'z_index', defaults.zIndex),
    mirrorHorizontal: readField<boolean>(raw, t, 'mirror_horizontal', false),
    mirrorVertical: readField<boolean>(raw, t, 'mirror_vertical', false),
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
    .map(([id, cfg]) => ({
      id,
      x: cfg.rect?.x ?? 0,
      y: cfg.rect?.y ?? 0,
      width: cfg.rect?.width ?? canvasWidth,
      height: cfg.rect?.height ?? canvasHeight,
      opacity: cfg.opacity ?? 1.0,
      zIndex: cfg.z_index ?? 0,
      rotationDegrees: cfg.rotation_degrees ?? 0,
      mirrorHorizontal: cfg.mirror_horizontal ?? false,
      mirrorVertical: cfg.mirror_vertical ?? false,
      visible: true,
    }))
    .sort((a, b) => a.zIndex - b.zIndex);
}

/** Parse text overlays from compositor params */
export function parseTextOverlays(params: Record<string, unknown>): TextOverlayState[] {
  const overlays = params.text_overlays as TextOverlayConfig[] | undefined;
  if (!Array.isArray(overlays)) return [];
  return overlays.map((o, i) => ({
    id: o.id ?? `text_${i}`,
    text: o.text ?? '',
    color: o.color ?? [255, 255, 255, 255],
    fontSize: o.font_size ?? 24,
    fontName: o.font_name ?? 'dejavu-sans',
    ...parseTransformFields(o as unknown as Record<string, unknown>, {
      width: 200,
      height: 40,
      zIndex: 100 + i,
    }),
    visible: true,
  }));
}

/** Parse image overlays from compositor params.
 *  Z-index band: image overlays default to 200+i (video: 0–99, text: 100–199,
 *  image: 200+). */
export function parseImageOverlays(params: Record<string, unknown>): ImageOverlayState[] {
  const overlays = params.image_overlays as ImageOverlayConfig[] | undefined;
  if (!Array.isArray(overlays)) return [];
  return overlays.map((o, i) => ({
    id: o.id ?? `img_${i}`,
    dataBase64: o.data_base64 ?? '',
    ...parseTransformFields(o as unknown as Record<string, unknown>, {
      width: 200,
      height: 200,
      zIndex: 200 + i,
    }),
    visible: true,
  }));
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
export function serializeTextOverlays(overlays: TextOverlayState[]): TextOverlayConfig[] {
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

/** Serialize image overlays back to config format */
export function serializeImageOverlays(overlays: ImageOverlayState[]): ImageOverlayConfig[] {
  return overlays.map((o) => ({
    id: o.id,
    data_base64: o.dataBase64,
    rect: serializeRect(o),
    ...serializeSpatialFields(o),
  }));
}

/** Serialize video layers to the wire format used by the backend. */
export function serializeLayers(layers: LayerState[]): Record<string, LayerConfig> {
  const layersMap: Record<string, LayerConfig> = {};
  for (const layer of layers) {
    layersMap[layer.id] = {
      rect: serializeRect(layer),
      ...serializeSpatialFields(layer),
    };
  }
  return layersMap;
}

// ── Merge / diff ────────────────────────────────────────────────────────────

/** Merge parsed overlays with existing state, preserving client-side visibility.
 *  Returns the same array reference if nothing changed (avoiding re-renders).
 *  An optional `hasExtraChanges` comparator can detect changes in type-specific
 *  fields (e.g. `text`, `fontSize` for text overlays). */
export function mergeOverlayState<T extends OverlayBase>(
  current: T[],
  parsed: T[],
  hasExtraChanges?: (a: T, b: T) => boolean
): T[] {
  const merged = parsed.map((p) => {
    const existing = current.find((o) => o.id === p.id);
    if (existing) {
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

/** Build the full compositor config from current params + updated layers */
export function buildConfig(
  params: Record<string, unknown>,
  layers: LayerState[],
  textOverlays?: TextOverlayState[],
  imageOverlays?: ImageOverlayState[]
): Record<string, unknown> {
  return {
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
  return computeResizePosition(orig, handle!, rawDx, rawDy);
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
  rawDy: number
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
