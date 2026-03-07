// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook for managing compositor layer state with zero-render drag/resize.
 *
 * During pointer-driven interactions (drag, resize), visual updates are
 * applied directly to DOM elements via refs and requestAnimationFrame.
 * React state is only committed on pointer-up (or throttled for live mode),
 * keeping the experience butter-smooth with no mid-drag re-renders.
 */

import { throttle } from 'lodash-es';
import { useState, useEffect, useCallback, useRef, useMemo } from 'react';

import { useSessionStore, selectNodeViewData } from '@/stores/sessionStore';

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
  /** Unique client-side id (index-based) */
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
}

/** An image overlay stored in compositor config */
export interface ImageOverlayState {
  /** Unique client-side id (index-based) */
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

/** Grid step used when snap-to-grid is active (pixels in canvas space). */
const SNAP_GRID = 10;
/** Distance threshold for snapping to centre guidelines (pixels). */
const SNAP_THRESHOLD = 8;

export interface UseCompositorLayersOptions {
  nodeId: string;
  sessionId?: string;
  canvasWidth: number;
  canvasHeight: number;
  params: Record<string, unknown>;
  onConfigChange?: (nodeId: string, config: Record<string, unknown>) => void;
  onParamChange?: (nodeId: string, paramName: string, value: unknown) => void;
  isStaged?: boolean;
  throttleMs?: number;
}

/** Which category a layer belongs to for drag commit routing */
export type LayerKind = 'video' | 'text' | 'image';

export interface UseCompositorLayersResult {
  layers: LayerState[];
  selectedLayerId: string | null;
  selectLayer: (id: string | null) => void;
  handleLayerPointerDown: (layerId: string, e: React.PointerEvent) => void;
  handleResizePointerDown: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  updateLayerOpacity: (layerId: string, opacity: number) => void;
  updateLayerRotation: (layerId: string, degrees: number) => void;
  updateLayerZIndex: (layerId: string, zIndex: number) => void;
  toggleLayerVisibility: (layerId: string) => void;
  /** Toggle horizontal or vertical mirroring for a layer (video, text, or image). */
  updateLayerMirror: (layerId: string, axis: 'horizontal' | 'vertical') => void;
  /** Ref map: layer elements register here for direct DOM manipulation during drag */
  layerRefs: React.MutableRefObject<Map<string, HTMLDivElement>>;
  /** Whether a drag/resize is currently in progress */
  isDragging: boolean;
  /** Text overlays */
  textOverlays: TextOverlayState[];
  /** Image overlays */
  imageOverlays: ImageOverlayState[];
  addTextOverlay: (text: string) => void;
  updateTextOverlay: (id: string, updates: Partial<Omit<TextOverlayState, 'id'>>) => void;
  removeTextOverlay: (id: string) => void;
  addImageOverlay: (dataBase64: string, naturalWidth?: number, naturalHeight?: number) => void;
  updateImageOverlay: (id: string, updates: Partial<Omit<ImageOverlayState, 'id'>>) => void;
  removeImageOverlay: (id: string) => void;
  /** Atomically reassign z-index values for all layer types in one commit.
   *  Each entry maps a layer id + kind to its new z-index. */
  reorderLayers: (entries: Array<{ id: string; kind: LayerKind; zIndex: number }>) => void;
}

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
  data_base64: string;
  rect: Rect;
  opacity?: number;
  rotation_degrees?: number;
  z_index?: number;
  mirror_horizontal?: boolean;
  mirror_vertical?: boolean;
}

/** Parse layers from compositor params into LayerState array */
function parseLayers(
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
function parseTextOverlays(params: Record<string, unknown>): TextOverlayState[] {
  const overlays = params.text_overlays as TextOverlayConfig[] | undefined;
  if (!Array.isArray(overlays)) return [];
  return overlays.map((o, i) => {
    // Support both flat format (rect/opacity/z_index at top level, matching
    // #[serde(flatten)]) and legacy nested format (under "transform:").
    const t = (o as Record<string, unknown>).transform as Record<string, unknown> | undefined;
    const rect = o.rect ?? (t?.rect as TextOverlayConfig['rect'] | undefined);
    return {
      id: `text_${i}`,
      text: o.text ?? '',
      x: rect?.x ?? 0,
      y: rect?.y ?? 0,
      width: rect?.width ?? 200,
      height: rect?.height ?? 40,
      color: o.color ?? [255, 255, 255, 255],
      fontSize: o.font_size ?? 24,
      fontName: o.font_name ?? 'dejavu-sans',
      opacity: o.opacity ?? (t?.opacity as number | undefined) ?? 1.0,
      rotationDegrees: o.rotation_degrees ?? (t?.rotation_degrees as number | undefined) ?? 0,
      zIndex: o.z_index ?? (t?.z_index as number | undefined) ?? 100 + i,
      mirrorHorizontal: o.mirror_horizontal ?? (t?.mirror_horizontal as boolean | undefined) ?? false,
      mirrorVertical: o.mirror_vertical ?? (t?.mirror_vertical as boolean | undefined) ?? false,
      visible: true,
    };
  });
}

/** Parse image overlays from compositor params.
 *  Z-index band: image overlays default to 200+i (video: 0–99, text: 100–199,
 *  image: 200+). */
function parseImageOverlays(params: Record<string, unknown>): ImageOverlayState[] {
  const overlays = params.image_overlays as ImageOverlayConfig[] | undefined;
  if (!Array.isArray(overlays)) return [];
  return overlays.map((o, i) => {
    // Support both flat format and legacy nested "transform:" format.
    const t = (o as Record<string, unknown>).transform as Record<string, unknown> | undefined;
    const rect = o.rect ?? (t?.rect as ImageOverlayConfig['rect'] | undefined);
    return {
      id: `img_${i}`,
      dataBase64: o.data_base64 ?? '',
      x: rect?.x ?? 0,
      y: rect?.y ?? 0,
      width: rect?.width ?? 200,
      height: rect?.height ?? 200,
      opacity: o.opacity ?? (t?.opacity as number | undefined) ?? 1.0,
      rotationDegrees: o.rotation_degrees ?? (t?.rotation_degrees as number | undefined) ?? 0,
      zIndex: o.z_index ?? (t?.z_index as number | undefined) ?? 200 + i,
      mirrorHorizontal: o.mirror_horizontal ?? (t?.mirror_horizontal as boolean | undefined) ?? false,
      mirrorVertical: o.mirror_vertical ?? (t?.mirror_vertical as boolean | undefined) ?? false,
      visible: true,
    };
  });
}

/** Serialize text overlays back to config format */
function serializeTextOverlays(overlays: TextOverlayState[]): TextOverlayConfig[] {
  return overlays.map((o) => ({
    text: o.text,
    rect: {
      x: Math.round(o.x),
      y: Math.round(o.y),
      width: Math.max(1, Math.round(o.width)),
      height: Math.max(1, Math.round(o.height)),
    },
    color: o.color,
    font_size: o.fontSize,
    font_name: o.fontName,
    opacity: o.visible ? Math.round(o.opacity * 100) / 100 : 0,
    rotation_degrees: Math.round(o.rotationDegrees * 10) / 10,
    z_index: o.zIndex,
    mirror_horizontal: o.mirrorHorizontal,
    mirror_vertical: o.mirrorVertical,
  }));
}

/** Serialize image overlays back to config format */
function serializeImageOverlays(overlays: ImageOverlayState[]): ImageOverlayConfig[] {
  return overlays.map((o) => ({
    data_base64: o.dataBase64,
    rect: {
      x: Math.round(o.x),
      y: Math.round(o.y),
      width: Math.max(1, Math.round(o.width)),
      height: Math.max(1, Math.round(o.height)),
    },
    opacity: o.visible ? Math.round(o.opacity * 100) / 100 : 0,
    rotation_degrees: Math.round(o.rotationDegrees * 10) / 10,
    z_index: o.zIndex,
    mirror_horizontal: o.mirrorHorizontal,
    mirror_vertical: o.mirrorVertical,
  }));
}

/** Common spatial fields shared by all overlay state types (text and image). */
interface OverlayBase {
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

/** Merge parsed overlays with existing state, preserving client-side visibility.
 *  Returns the same array reference if nothing changed (avoiding re-renders).
 *  An optional `hasExtraChanges` comparator can detect changes in type-specific
 *  fields (e.g. `text`, `fontSize` for text overlays). */
function mergeOverlayState<T extends OverlayBase>(
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

/** Serialize video layers to the wire format used by the backend. */
function serializeLayers(layers: LayerState[]): Record<string, LayerConfig> {
  const layersMap: Record<string, LayerConfig> = {};
  for (const layer of layers) {
    layersMap[layer.id] = {
      rect: {
        x: Math.round(layer.x),
        y: Math.round(layer.y),
        width: Math.max(1, Math.round(layer.width)),
        height: Math.max(1, Math.round(layer.height)),
      },
      opacity: layer.visible ? Math.round(layer.opacity * 100) / 100 : 0,
      z_index: layer.zIndex,
      rotation_degrees: Math.round(layer.rotationDegrees * 10) / 10,
      mirror_horizontal: layer.mirrorHorizontal,
      mirror_vertical: layer.mirrorVertical,
    };
  }
  return layersMap;
}

/** Build the full compositor config from current params + updated layers */
function buildConfig(
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

export const useCompositorLayers = (
  options: UseCompositorLayersOptions
): UseCompositorLayersResult => {
  const {
    nodeId,
    sessionId,
    canvasWidth,
    canvasHeight,
    params,
    onConfigChange,
    onParamChange,
    throttleMs = 100,
  } = options;

  const [layers, setLayers] = useState<LayerState[]>(() =>
    parseLayers(params, canvasWidth, canvasHeight)
  );
  const [textOverlays, setTextOverlays] = useState<TextOverlayState[]>(() =>
    parseTextOverlays(params)
  );
  const [imageOverlays, setImageOverlays] = useState<ImageOverlayState[]>(() =>
    parseImageOverlays(params)
  );
  const [selectedLayerId, setSelectedLayerId] = useState<string | null>(null);
  const [isDragging, setIsDragging] = useState(false);

  // Keep a stable ref to params so throttled/memoised callbacks can read
  // the latest value at call-time without listing it as a dependency.
  // This prevents cascading callback recreations when only the params
  // object reference changes (e.g. server echo-back after a config update).
  const paramsRef = useRef(params);
  useEffect(() => {
    paramsRef.current = params;
  }, [params]);

  // Keep overlay refs in sync for config building
  const textOverlaysRef = useRef(textOverlays);
  const imageOverlaysRef = useRef(imageOverlays);
  useEffect(() => {
    textOverlaysRef.current = textOverlays;
  }, [textOverlays]);
  useEffect(() => {
    imageOverlaysRef.current = imageOverlays;
  }, [imageOverlays]);

  // Guard against sync-from-params overwriting a local overlay mutation.
  // After any local overlay commit we set this to Date.now().  The sync
  // effect skips overlay parsing while the guard is active (< 3 s).
  const overlayCommitGuardRef = useRef<number>(0);

  // Refs for zero-render drag/resize
  const layerRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const dragStateRef = useRef<{
    type: 'drag' | 'resize';
    layerId: string;
    /** Which layer array this item belongs to, for correct commit on pointer-up */
    layerKind: LayerKind;
    handle?: ResizeHandle;
    startX: number;
    startY: number;
    origLayer: LayerState;
    scale: number; // canvas scale factor
    rafId: number | null;
    currentX: number;
    currentY: number;
    /** Original font size stashed at resize-start for text overlays so we
     *  can scale fontSize proportionally to the width change. */
    origFontSize?: number;
  } | null>(null);
  const layersRef = useRef(layers);

  // Keep ref in sync
  useEffect(() => {
    layersRef.current = layers;
  }, [layers]);

  // Sync from props when params change (and not mid-drag)
  useEffect(() => {
    if (dragStateRef.current) return;
    const parsed = parseLayers(params, canvasWidth, canvasHeight);

    // Issue #4 fix: only update layers state if the merged result actually
    // differs from current state.  This prevents unnecessary re-renders that
    // cause positions to briefly revert when switching selected layers.
    const merged = mergeOverlayState(layersRef.current, parsed);
    if (merged !== layersRef.current) {
      setLayers(merged);
    }

    // Skip overlay re-parse if we just committed a local overlay change.
    // This prevents stale params from overwriting the local removal/add.
    const sinceCommit = Date.now() - overlayCommitGuardRef.current;
    if (sinceCommit < 3000) return;

    setTextOverlays((currentText) =>
      mergeOverlayState(
        currentText,
        parseTextOverlays(params),
        (a, b) =>
          a.text !== b.text ||
          a.fontSize !== b.fontSize ||
          a.fontName !== b.fontName ||
          a.color.some((v, i) => v !== b.color[i])
      )
    );
    setImageOverlays((currentImg) => mergeOverlayState(currentImg, parseImageOverlays(params)));
  }, [params, canvasWidth, canvasHeight]);

  // ── Server-driven layout (Monitor view only) ────────────────────────────
  // When a live pipeline is running (sessionId is set), subscribe to the
  // compositor's view data and apply server-computed positions/dimensions.
  // Server is the source of truth in Monitor view; client drives Design view.
  //
  // We use an external Zustand subscription (useSessionStore.subscribe)
  // instead of a store selector hook to avoid triggering a React re-render
  // on every view-data arrival.  The setters below already perform shallow
  // comparison and only produce a new state reference when something
  // actually changed, so the component only re-renders when needed.
  useEffect(() => {
    if (!sessionId) return;

    const applyServerLayout = (viewData: unknown) => {
      if (!viewData) return;
      if (dragStateRef.current) return; // ignore server updates while dragging

      const layout = viewData as {
        canvas_width: number;
        canvas_height: number;
        layers: Array<{
          id: string;
          x: number;
          y: number;
          width: number;
          height: number;
          opacity: number;
          z_index: number;
          rotation_degrees: number;
        }>;
        text_overlays: Array<{
          index: number;
          x: number;
          y: number;
          width: number;
          height: number;
          opacity: number;
          z_index: number;
          rotation_degrees: number;
        }>;
        image_overlays: Array<{
          index: number;
          x: number;
          y: number;
          width: number;
          height: number;
          opacity: number;
          z_index: number;
          rotation_degrees: number;
        }>;
      };

      if (!layout.layers) return;

      // Apply server layers
      setLayers((prev) => {
        const serverLayers: LayerState[] = layout.layers.map((sl) => {
          const existing = prev.find((l) => l.id === sl.id);
          // When a layer is hidden client-side, the backend receives opacity=0.
          // Preserve the original opacity so toggling visibility back ON restores it.
          const opacity = existing && !existing.visible ? existing.opacity : sl.opacity;
          return {
            id: sl.id,
            x: sl.x,
            y: sl.y,
            width: sl.width,
            height: sl.height,
            opacity,
            zIndex: sl.z_index,
            rotationDegrees: sl.rotation_degrees,
            mirrorHorizontal: existing?.mirrorHorizontal ?? false,
            mirrorVertical: existing?.mirrorVertical ?? false,
            visible: existing?.visible ?? true,
          };
        });
        const changed =
          serverLayers.length !== prev.length ||
          serverLayers.some(
            (s, i) =>
              s.id !== prev[i].id ||
              s.x !== prev[i].x ||
              s.y !== prev[i].y ||
              s.width !== prev[i].width ||
              s.height !== prev[i].height ||
              s.opacity !== prev[i].opacity ||
              s.zIndex !== prev[i].zIndex ||
              s.rotationDegrees !== prev[i].rotationDegrees ||
              s.visible !== prev[i].visible
          );
        return changed ? serverLayers : prev;
      });

      // Apply server text overlay dimensions
      if (layout.text_overlays) {
        setTextOverlays((prev) => {
          const next = prev.map((o, i) => {
            const so = layout.text_overlays.find((s) => s.index === i);
            if (!so) return o;
            // Preserve original opacity for hidden overlays (same guard as layers)
            const opacity = !o.visible ? o.opacity : so.opacity;
            if (
              o.x === so.x &&
              o.y === so.y &&
              o.width === so.width &&
              o.height === so.height &&
              o.opacity === opacity &&
              o.zIndex === so.z_index &&
              o.rotationDegrees === so.rotation_degrees
            ) {
              return o;
            }
            return {
              ...o,
              x: so.x,
              y: so.y,
              width: so.width,
              height: so.height,
              opacity,
              zIndex: so.z_index,
              rotationDegrees: so.rotation_degrees,
            };
          });
          return next.some((n, i) => n !== prev[i]) ? next : prev;
        });
      }

      // Apply server image overlay dimensions
      if (layout.image_overlays) {
        setImageOverlays((prev) => {
          const next = prev.map((o, i) => {
            const so = layout.image_overlays.find((s) => s.index === i);
            if (!so) return o;
            // Preserve original opacity for hidden overlays (same guard as layers)
            const opacity = !o.visible ? o.opacity : so.opacity;
            if (
              o.x === so.x &&
              o.y === so.y &&
              o.width === so.width &&
              o.height === so.height &&
              o.opacity === opacity &&
              o.zIndex === so.z_index &&
              o.rotationDegrees === so.rotation_degrees
            ) {
              return o;
            }
            return {
              ...o,
              x: so.x,
              y: so.y,
              width: so.width,
              height: so.height,
              opacity,
              zIndex: so.z_index,
              rotationDegrees: so.rotation_degrees,
            };
          });
          return next.some((n, i) => n !== prev[i]) ? next : prev;
        });
      }
    };

    // Apply current value immediately (if any)
    const current = selectNodeViewData(sessionId, nodeId)(useSessionStore.getState());
    applyServerLayout(current);

    // Subscribe externally — does NOT cause React re-renders
    const unsubscribe = useSessionStore.subscribe(
      selectNodeViewData(sessionId, nodeId),
      applyServerLayout
    );
    return unsubscribe;
  }, [sessionId, nodeId]);

  /** Resolve a layer ID to its state and kind across all layer types */
  const findAnyLayer = useCallback(
    (layerId: string): { state: LayerState; kind: LayerKind } | null => {
      const videoLayer = layersRef.current.find((l) => l.id === layerId);
      if (videoLayer) return { state: videoLayer, kind: 'video' };

      const textOverlay = textOverlaysRef.current.find((o) => o.id === layerId);
      if (textOverlay) {
        return {
          state: {
            id: textOverlay.id,
            x: textOverlay.x,
            y: textOverlay.y,
            width: textOverlay.width,
            height: textOverlay.height,
            opacity: textOverlay.opacity,
            zIndex: textOverlay.zIndex,
            rotationDegrees: textOverlay.rotationDegrees,
            mirrorHorizontal: textOverlay.mirrorHorizontal,
            mirrorVertical: textOverlay.mirrorVertical,
            visible: textOverlay.visible,
          },
          kind: 'text',
        };
      }

      const imgOverlay = imageOverlaysRef.current.find((o) => o.id === layerId);
      if (imgOverlay) {
        return {
          state: {
            id: imgOverlay.id,
            x: imgOverlay.x,
            y: imgOverlay.y,
            width: imgOverlay.width,
            height: imgOverlay.height,
            opacity: imgOverlay.opacity,
            zIndex: imgOverlay.zIndex,
            rotationDegrees: imgOverlay.rotationDegrees,
            mirrorHorizontal: imgOverlay.mirrorHorizontal,
            mirrorVertical: imgOverlay.mirrorVertical,
            visible: imgOverlay.visible,
          },
          kind: 'image',
        };
      }

      return null;
    },
    []
  );

  // Throttled config change
  const throttledConfigChange = useMemo(() => {
    if (!onConfigChange && !onParamChange) return null;
    return throttle(
      (currentLayers: LayerState[]) => {
        if (onConfigChange) {
          // Always include the latest local overlay state so we never
          // send stale overlay positions from params when committing a
          // video layer change.
          const config = buildConfig(
            paramsRef.current,
            currentLayers,
            textOverlaysRef.current,
            imageOverlaysRef.current
          );
          onConfigChange(nodeId, config);
        } else if (onParamChange) {
          // Design View path: update the layers param directly
          const layersMap: Record<string, LayerConfig> = {};
          for (const layer of currentLayers) {
            layersMap[layer.id] = {
              rect: {
                x: Math.round(layer.x),
                y: Math.round(layer.y),
                width: Math.max(1, Math.round(layer.width)),
                height: Math.max(1, Math.round(layer.height)),
              },
              opacity: layer.visible ? Math.round(layer.opacity * 100) / 100 : 0,
              z_index: layer.zIndex,
              rotation_degrees: Math.round(layer.rotationDegrees * 10) / 10,
              mirror_horizontal: layer.mirrorHorizontal,
              mirror_vertical: layer.mirrorVertical,
            };
          }
          onParamChange(nodeId, 'layers', layersMap);
        }
      },
      throttleMs,
      { leading: true, trailing: true }
    );
  }, [nodeId, onConfigChange, onParamChange, throttleMs]);

  // Throttled overlay commit for continuous updates (sliders, drag, etc.)
  // Prevents flooding the server with config changes on every slider tick.
  const throttledOverlayCommit = useMemo(() => {
    if (!onConfigChange && !onParamChange) return null;
    return throttle(
      (nextText: TextOverlayState[], nextImg: ImageOverlayState[]) => {
        if (onConfigChange) {
          const config = buildConfig(paramsRef.current, layersRef.current, nextText, nextImg);
          onConfigChange(nodeId, config);
        } else if (onParamChange) {
          onParamChange(nodeId, 'text_overlays', serializeTextOverlays(nextText));
          onParamChange(nodeId, 'image_overlays', serializeImageOverlays(nextImg));
        }
      },
      throttleMs,
      { leading: true, trailing: true }
    );
  }, [nodeId, onConfigChange, onParamChange, throttleMs]);

  // Cleanup throttles on unmount
  useEffect(
    () => () => {
      throttledConfigChange?.cancel();
      throttledOverlayCommit?.cancel();
    },
    [throttledConfigChange, throttledOverlayCommit]
  );

  /** Compute updated layer from current pointer position */
  const computeUpdatedLayer = useCallback(
    (
      state: NonNullable<typeof dragStateRef.current>,
      clientX: number,
      clientY: number
    ): LayerState => {
      const rawDx = (clientX - state.startX) / state.scale;
      const rawDy = (clientY - state.startY) / state.scale;
      const orig = state.origLayer;

      if (state.type === 'drag') {
        let nx = orig.x + rawDx;
        let ny = orig.y + rawDy;

        // Only apply snapping when the pointer actually moved (avoid
        // quantizing the position on a click-only selection).
        if (rawDx !== 0 || rawDy !== 0) {
          // Snap to grid: round position to nearest SNAP_GRID step
          nx = Math.round(nx / SNAP_GRID) * SNAP_GRID;
          ny = Math.round(ny / SNAP_GRID) * SNAP_GRID;

          // Snap to canvas centre guidelines
          const cw = canvasWidth;
          const ch = canvasHeight;
          const midX = nx + orig.width / 2;
          const midY = ny + orig.height / 2;

          // Horizontal centre
          if (Math.abs(midX - cw / 2) < SNAP_THRESHOLD) {
            nx = (cw - orig.width) / 2;
          }
          // Vertical centre
          if (Math.abs(midY - ch / 2) < SNAP_THRESHOLD) {
            ny = (ch - orig.height) / 2;
          }
        }

        return { ...orig, x: nx, y: ny };
      }

      // Issue #5 fix: transform mouse delta into the layer's local coordinate
      // system so resize handles behave naturally on rotated layers.
      let dx = rawDx;
      let dy = rawDy;
      if (orig.rotationDegrees !== 0) {
        const rad = (-orig.rotationDegrees * Math.PI) / 180;
        const cos = Math.cos(rad);
        const sin = Math.sin(rad);
        dx = rawDx * cos - rawDy * sin;
        dy = rawDx * sin + rawDy * cos;
      }

      // Resize
      const handle = state.handle!;
      let newX = orig.x;
      let newY = orig.y;
      let newW = orig.width;
      let newH = orig.height;

      if (handle.includes('e')) {
        newW = Math.max(20, orig.width + dx);
      }
      if (handle.includes('w')) {
        newW = Math.max(20, orig.width - dx);
        newX = orig.x + (orig.width - newW);
      }
      if (handle.includes('s')) {
        newH = Math.max(20, orig.height + dy);
      }
      if (handle.includes('n')) {
        newH = Math.max(20, orig.height - dy);
        newY = orig.y + (orig.height - newH);
      }

      // Constrain resize to maintain aspect ratio for all layer types.
      if (orig.width > 0 && orig.height > 0) {
        const ar = orig.width / orig.height;
        const isCorner = handle.length === 2; // 'ne', 'nw', 'se', 'sw'

        if (isCorner) {
          // Corner handles: use the dominant axis (whichever changed more)
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
          // 'n' or 's'
          newW = newH * ar;
        }

        // Re-clamp
        newW = Math.max(20, newW);
        newH = Math.max(20, newH);

        // Adjust position for north/west handles to keep opposite edge fixed
        if (handle.includes('w')) {
          newX = orig.x + (orig.width - newW);
        }
        if (handle.includes('n')) {
          newY = orig.y + (orig.height - newH);
        }
      }

      return { ...orig, x: newX, y: newY, width: newW, height: newH };
    },
    [canvasWidth, canvasHeight]
  );

  /** Apply visual update to DOM element (no React state).
   *  When `sizeChanged` is false (pure drag) we skip width/height so that
   *  component-level height expansions (e.g. text wrapping) are preserved. */
  const applyVisualUpdate = useCallback((layer: LayerState, sizeChanged: boolean) => {
    const el = layerRefs.current.get(layer.id);
    if (!el) return;
    el.style.left = `${layer.x}px`;
    el.style.top = `${layer.y}px`;
    if (sizeChanged) {
      el.style.width = `${layer.width}px`;
      el.style.height = `${layer.height}px`;
    }
  }, []);

  // Global pointer move/up handlers (attached on drag start, removed on end)
  const handlePointerMove = useCallback(
    (e: PointerEvent) => {
      const state = dragStateRef.current;
      if (!state) return;

      state.currentX = e.clientX;
      state.currentY = e.clientY;

      if (state.rafId !== null) return;
      state.rafId = requestAnimationFrame(() => {
        const s = dragStateRef.current;
        if (!s) return;
        s.rafId = null;
        const updated = computeUpdatedLayer(s, s.currentX, s.currentY);
        applyVisualUpdate(updated, s.type === 'resize');
      });
    },
    [computeUpdatedLayer, applyVisualUpdate]
  );

  const handlePointerUp = useCallback(
    (e: PointerEvent) => {
      const state = dragStateRef.current;
      if (!state) return;

      if (state.rafId !== null) {
        cancelAnimationFrame(state.rafId);
      }

      const updated = computeUpdatedLayer(state, e.clientX, e.clientY);
      setIsDragging(false);

      if (state.layerKind === 'video') {
        // Commit video layer to React state
        setLayers((prev) => prev.map((l) => (l.id === updated.id ? updated : l)));
        // Send to server
        const newLayers = layersRef.current.map((l) => (l.id === updated.id ? updated : l));
        throttledConfigChange?.(newLayers);
      } else if (state.layerKind === 'text') {
        // Commit text overlay position/size (and scaled fontSize for resizes)
        const isResize = state.type === 'resize';
        const origFontSize = state.origFontSize;
        setTextOverlays((prev) => {
          const next = prev.map((o) => {
            if (o.id !== updated.id) return o;
            const patch: Partial<TextOverlayState> = {
              x: updated.x,
              y: updated.y,
              width: updated.width,
              height: updated.height,
            };
            // Scale fontSize proportionally to width change on resize
            if (isResize && origFontSize != null && state.origLayer.width > 0) {
              patch.fontSize = Math.max(
                8,
                Math.round(origFontSize * (updated.width / state.origLayer.width))
              );
            }
            return { ...o, ...patch };
          });
          commitOverlaysRef.current(next, imageOverlaysRef.current);
          return next;
        });
      } else if (state.layerKind === 'image') {
        // Commit image overlay position/size
        setImageOverlays((prev) => {
          const next = prev.map((o) =>
            o.id === updated.id
              ? { ...o, x: updated.x, y: updated.y, width: updated.width, height: updated.height }
              : o
          );
          commitOverlaysRef.current(textOverlaysRef.current, next);
          return next;
        });
      }

      dragStateRef.current = null;
      document.removeEventListener('pointermove', handlePointerMove);
      document.removeEventListener('pointerup', handlePointerUp);
    },
    [computeUpdatedLayer, throttledConfigChange, handlePointerMove]
  );

  /** Start dragging a layer (video, text overlay, or image overlay) */
  const handleLayerPointerDown = useCallback(
    (layerId: string, e: React.PointerEvent) => {
      e.stopPropagation();
      e.preventDefault();

      const found = findAnyLayer(layerId);
      if (!found) return;

      setSelectedLayerId(layerId);

      // Compute scale from container
      const el = layerRefs.current.get(layerId);
      const container = el?.parentElement;
      const scale = container
        ? container.getBoundingClientRect().width /
          (Number(container.dataset.canvasWidth) || canvasWidth)
        : 1;

      dragStateRef.current = {
        type: 'drag',
        layerId,
        layerKind: found.kind,
        startX: e.clientX,
        startY: e.clientY,
        origLayer: { ...found.state },
        scale,
        rafId: null,
        currentX: e.clientX,
        currentY: e.clientY,
      };

      setIsDragging(true);
      document.addEventListener('pointermove', handlePointerMove);
      document.addEventListener('pointerup', handlePointerUp);
    },
    [canvasWidth, findAnyLayer, handlePointerMove, handlePointerUp]
  );

  /** Start resizing a layer (video, text overlay, or image overlay) */
  const handleResizePointerDown = useCallback(
    (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => {
      e.stopPropagation();
      e.preventDefault();

      const found = findAnyLayer(layerId);
      if (!found) return;

      const el = layerRefs.current.get(layerId);
      const container = el?.parentElement;
      const scale = container
        ? container.getBoundingClientRect().width /
          (Number(container.dataset.canvasWidth) || canvasWidth)
        : 1;

      // Stash origFontSize for text overlays so we can scale font
      // proportionally during resize.
      const origFontSize =
        found.kind === 'text'
          ? textOverlaysRef.current.find((o) => o.id === layerId)?.fontSize
          : undefined;

      dragStateRef.current = {
        type: 'resize',
        layerId,
        layerKind: found.kind,
        handle,
        startX: e.clientX,
        startY: e.clientY,
        origLayer: { ...found.state },
        scale,
        rafId: null,
        currentX: e.clientX,
        currentY: e.clientY,
        origFontSize,
      };

      setIsDragging(true);
      document.addEventListener('pointermove', handlePointerMove);
      document.addEventListener('pointerup', handlePointerUp);
    },
    [canvasWidth, findAnyLayer, handlePointerMove, handlePointerUp]
  );

  const selectLayer = useCallback((id: string | null) => {
    setSelectedLayerId(id);
  }, []);

  const updateLayerOpacity = useCallback(
    (layerId: string, opacity: number) => {
      setLayers((prev) => {
        const next = prev.map((l) =>
          l.id === layerId ? { ...l, opacity: Math.max(0, Math.min(1, opacity)) } : l
        );
        throttledConfigChange?.(next);
        return next;
      });
    },
    [throttledConfigChange]
  );

  const updateLayerRotation = useCallback(
    (layerId: string, degrees: number) => {
      setLayers((prev) => {
        const next = prev.map((l) => (l.id === layerId ? { ...l, rotationDegrees: degrees } : l));
        throttledConfigChange?.(next);
        return next;
      });
    },
    [throttledConfigChange]
  );

  const updateLayerZIndex = useCallback(
    (layerId: string, zIndex: number) => {
      setLayers((prev) => {
        const next = prev
          .map((l) => (l.id === layerId ? { ...l, zIndex } : l))
          .sort((a, b) => a.zIndex - b.zIndex);
        throttledConfigChange?.(next);
        return next;
      });
    },
    [throttledConfigChange]
  );

  // ── Visibility toggle ──────────────────────────────────────────────────────

  const toggleLayerVisibility = useCallback(
    (layerId: string) => {
      // Check video layers
      const isVideoLayer = layersRef.current.some((l) => l.id === layerId);
      if (isVideoLayer) {
        setLayers((prev) => {
          const next = prev.map((l) => (l.id === layerId ? { ...l, visible: !l.visible } : l));
          throttledConfigChange?.(next);
          return next;
        });
        return;
      }

      // Check text overlays
      const isTextOverlay = textOverlaysRef.current.some((o) => o.id === layerId);
      if (isTextOverlay) {
        setTextOverlays((prev) => {
          const next = prev.map((o) => (o.id === layerId ? { ...o, visible: !o.visible } : o));
          commitOverlaysRef.current(next, imageOverlaysRef.current);
          return next;
        });
        return;
      }

      // Check image overlays
      const isImgOverlay = imageOverlaysRef.current.some((o) => o.id === layerId);
      if (isImgOverlay) {
        setImageOverlays((prev) => {
          const next = prev.map((o) => (o.id === layerId ? { ...o, visible: !o.visible } : o));
          commitOverlaysRef.current(textOverlaysRef.current, next);
          return next;
        });
      }
    },
    [throttledConfigChange]
  );

  // ── Mirror toggle ───────────────────────────────────────────────────────

  const updateLayerMirror = useCallback(
    (layerId: string, axis: 'horizontal' | 'vertical') => {
      const field = axis === 'horizontal' ? 'mirrorHorizontal' : 'mirrorVertical';

      // Check video layers
      const isVideoLayer = layersRef.current.some((l) => l.id === layerId);
      if (isVideoLayer) {
        setLayers((prev) => {
          const next = prev.map((l) => (l.id === layerId ? { ...l, [field]: !l[field] } : l));
          throttledConfigChange?.(next);
          return next;
        });
        return;
      }

      // Check text overlays
      const isTextOverlay = textOverlaysRef.current.some((o) => o.id === layerId);
      if (isTextOverlay) {
        setTextOverlays((prev) => {
          const next = prev.map((o) => (o.id === layerId ? { ...o, [field]: !o[field] } : o));
          commitOverlaysRef.current(next, imageOverlaysRef.current);
          return next;
        });
        return;
      }

      // Check image overlays
      const isImgOverlay = imageOverlaysRef.current.some((o) => o.id === layerId);
      if (isImgOverlay) {
        setImageOverlays((prev) => {
          const next = prev.map((o) => (o.id === layerId ? { ...o, [field]: !o[field] } : o));
          commitOverlaysRef.current(textOverlaysRef.current, next);
          return next;
        });
      }
    },
    [throttledConfigChange]
  );

  // ── Overlay commit helper ─────────────────────────────────────────────────

  const commitOverlays = useCallback(
    (nextText: TextOverlayState[], nextImg: ImageOverlayState[]) => {
      // Arm the guard so the sync effect won't overwrite overlays with
      // stale params before the backend echoes back this change.
      overlayCommitGuardRef.current = Date.now();

      if (onConfigChange) {
        const config = buildConfig(paramsRef.current, layersRef.current, nextText, nextImg);
        onConfigChange(nodeId, config);
      } else if (onParamChange) {
        onParamChange(nodeId, 'text_overlays', serializeTextOverlays(nextText));
        onParamChange(nodeId, 'image_overlays', serializeImageOverlays(nextImg));
      }
    },
    [nodeId, onConfigChange, onParamChange]
  );

  // Keep a stable ref so pointer-up can call the latest commitOverlays without
  // adding it to its dependency array (which would re-create event listeners).
  const commitOverlaysRef = useRef(commitOverlays);
  useEffect(() => {
    commitOverlaysRef.current = commitOverlays;
  }, [commitOverlays]);

  // ── Generic overlay CRUD helpers ───────────────────────────────────────────
  //
  // updateOverlay / removeOverlay eliminate the duplicated logic that was
  // previously copy-pasted between the text and image overlay callbacks.

  /** Update an overlay by id, apply partial updates, and commit.
   *  Shared by updateTextOverlay and updateImageOverlay. */
  const updateOverlay = useCallback(
    <T extends { id: string }>(
      id: string,
      updates: Partial<Omit<T, 'id'>>,
      setter: React.Dispatch<React.SetStateAction<T[]>>,
      buildCommitArgs: (next: T[]) => [TextOverlayState[], ImageOverlayState[]]
    ) => {
      // Arm the guard immediately so sync effect won't overwrite
      overlayCommitGuardRef.current = Date.now();
      setter((prev) => {
        const next = prev.map((o) => (o.id === id ? { ...o, ...updates } : o));
        const [text, img] = buildCommitArgs(next);
        // Use throttled commit to avoid flooding the server on slider drags
        if (throttledOverlayCommit) {
          throttledOverlayCommit(text, img);
        } else {
          commitOverlaysRef.current(text, img);
        }
        return next;
      });
    },
    [throttledOverlayCommit]
  );

  /** Remove an overlay by id, re-index remaining items, and commit.
   *  Shared by removeTextOverlay and removeImageOverlay. */
  const removeOverlay = useCallback(
    <T extends { id: string }>(
      id: string,
      idPrefix: string,
      setter: React.Dispatch<React.SetStateAction<T[]>>,
      buildCommitArgs: (next: T[]) => [TextOverlayState[], ImageOverlayState[]]
    ) => {
      setter((prev) => {
        const next = prev
          .filter((o) => o.id !== id)
          .map((o, i) => ({ ...o, id: `${idPrefix}_${i}` }));
        const [text, img] = buildCommitArgs(next);
        commitOverlays(text, img);
        return next;
      });
      // Clear selection — re-indexing makes the old selectedLayerId stale
      setSelectedLayerId(null);
    },
    [commitOverlays]
  );

  // ── Z-index helpers ──────────────────────────────────────────────────────

  /** Return the highest z-index currently in use across all layer types.
   *  Returns -1 when there are no layers so the first overlay gets z 0. */
  const maxZIndex = useCallback((): number => {
    let max = -1;
    for (const l of layersRef.current) if (l.zIndex > max) max = l.zIndex;
    for (const o of textOverlaysRef.current) if (o.zIndex > max) max = o.zIndex;
    for (const o of imageOverlaysRef.current) if (o.zIndex > max) max = o.zIndex;
    return max;
  }, []);

  // ── Text overlay CRUD ─────────────────────────────────────────────────────

  const addTextOverlay = useCallback(
    (text: string) => {
      setTextOverlays((prev) => {
        const newId = `text_${prev.length}`;
        const next: TextOverlayState[] = [
          ...prev,
          {
            id: newId,
            text,
            x: 40,
            y: 40 + prev.length * 50,
            width: 200,
            height: 40,
            color: [255, 255, 255, 255],
            fontSize: 24,
            fontName: 'dejavu-sans',
            opacity: 1.0,
            rotationDegrees: 0,
            zIndex: maxZIndex() + 1,
            mirrorHorizontal: false,
            mirrorVertical: false,
            visible: true,
          },
        ];
        commitOverlays(next, imageOverlaysRef.current);
        // Auto-select the newly added overlay so it's immediately interactive
        setSelectedLayerId(newId);
        return next;
      });
    },
    [commitOverlays, maxZIndex]
  );

  const updateTextOverlay = useCallback(
    (id: string, updates: Partial<Omit<TextOverlayState, 'id'>>) => {
      // Auto-expand the rect so the rendered text is never clipped.
      // The backend expands the bitmap to fit, but the UI should keep
      // the interactive rect in sync.
      const existing = textOverlaysRef.current.find((o) => o.id === id);
      if (existing) {
        const fontSize = updates.fontSize ?? existing.fontSize;
        const text = updates.text ?? existing.text;

        // Height: ~1.4× font size covers ascenders + descenders.
        const minHeight = Math.ceil(fontSize * 1.4);
        if (existing.height < minHeight && !('height' in updates)) {
          updates = { ...updates, height: minHeight };
        }

        // Width: ~0.6× font size per character is a reasonable estimate
        // for proportional fonts.  The backend will expand if still short.
        const minWidth = Math.ceil(text.length * fontSize * 0.6);
        if (existing.width < minWidth && !('width' in updates)) {
          updates = { ...updates, width: minWidth };
        }
      }
      updateOverlay(id, updates, setTextOverlays, (next) => [next, imageOverlaysRef.current]);
    },
    [updateOverlay]
  );

  const removeTextOverlay = useCallback(
    (id: string) =>
      removeOverlay(id, 'text', setTextOverlays, (next) => [
        next as unknown as TextOverlayState[],
        imageOverlaysRef.current,
      ]),
    [removeOverlay]
  );

  // ── Image overlay CRUD ────────────────────────────────────────────────────

  const addImageOverlay = useCallback(
    (dataBase64: string, naturalWidth?: number, naturalHeight?: number) => {
      setImageOverlays((prev) => {
        // Compute initial rect that preserves source aspect ratio.
        // Fit the image within a 200px box on its largest side.
        const maxDim = 200;
        let w = maxDim;
        let h = maxDim;
        if (naturalWidth && naturalHeight && naturalWidth > 0 && naturalHeight > 0) {
          const scale = Math.min(maxDim / naturalWidth, maxDim / naturalHeight, 1);
          w = Math.max(1, Math.round(naturalWidth * scale));
          h = Math.max(1, Math.round(naturalHeight * scale));
        }
        const newId = `img_${prev.length}`;
        const next: ImageOverlayState[] = [
          ...prev,
          {
            id: newId,
            dataBase64,
            x: 40,
            y: 40 + prev.length * 60,
            width: w,
            height: h,
            opacity: 1.0,
            rotationDegrees: 0,
            zIndex: maxZIndex() + 1,
            mirrorHorizontal: false,
            mirrorVertical: false,
            visible: true,
          },
        ];
        commitOverlays(textOverlaysRef.current, next);
        // Auto-select the newly added overlay so it's immediately interactive
        setSelectedLayerId(newId);
        return next;
      });
    },
    [commitOverlays, maxZIndex]
  );

  const updateImageOverlay = useCallback(
    (id: string, updates: Partial<Omit<ImageOverlayState, 'id'>>) =>
      updateOverlay(id, updates, setImageOverlays, (next) => [textOverlaysRef.current, next]),
    [updateOverlay]
  );

  const removeImageOverlay = useCallback(
    (id: string) =>
      removeOverlay(id, 'img', setImageOverlays, (next) => [
        textOverlaysRef.current,
        next as unknown as ImageOverlayState[],
      ]),
    [removeOverlay]
  );

  // ── Batch reorder ─────────────────────────────────────────────────────────
  //
  // Atomically update z-index for every layer in a single commit so that
  // drag-to-reorder doesn't fire N individual updates that race against
  // each other via stale refs.

  const reorderLayers = useCallback(
    (entries: Array<{ id: string; kind: LayerKind; zIndex: number }>) => {
      // Build lookup: id → new z-index
      const zMap = new Map<string, number>();
      for (const e of entries) zMap.set(e.id, e.zIndex);

      // Update video layers
      let nextLayers = layersRef.current;
      const hasVideoChanges = nextLayers.some((l) => {
        const z = zMap.get(l.id);
        return z !== undefined && z !== l.zIndex;
      });
      if (hasVideoChanges) {
        nextLayers = nextLayers
          .map((l) => {
            const z = zMap.get(l.id);
            return z !== undefined && z !== l.zIndex ? { ...l, zIndex: z } : l;
          })
          .sort((a, b) => a.zIndex - b.zIndex);
        setLayers(nextLayers);
      }

      // Update text overlays
      let nextText = textOverlaysRef.current;
      const hasTextChanges = nextText.some((o) => {
        const z = zMap.get(o.id);
        return z !== undefined && z !== o.zIndex;
      });
      if (hasTextChanges) {
        nextText = nextText.map((o) => {
          const z = zMap.get(o.id);
          return z !== undefined && z !== o.zIndex ? { ...o, zIndex: z } : o;
        });
        setTextOverlays(nextText);
      }

      // Update image overlays
      let nextImg = imageOverlaysRef.current;
      const hasImgChanges = nextImg.some((o) => {
        const z = zMap.get(o.id);
        return z !== undefined && z !== o.zIndex;
      });
      if (hasImgChanges) {
        nextImg = nextImg.map((o) => {
          const z = zMap.get(o.id);
          return z !== undefined && z !== o.zIndex ? { ...o, zIndex: z } : o;
        });
        setImageOverlays(nextImg);
      }

      // Single commit with all three updated arrays
      if (hasVideoChanges || hasTextChanges || hasImgChanges) {
        overlayCommitGuardRef.current = Date.now();
        if (onConfigChange) {
          const config = buildConfig(paramsRef.current, nextLayers, nextText, nextImg);
          onConfigChange(nodeId, config);
        } else if (onParamChange) {
          // Video layers — use immediate onParamChange (not throttled) so
          // all layer types commit atomically in the same tick.
          if (hasVideoChanges) {
            onParamChange(nodeId, 'layers', serializeLayers(nextLayers));
          }
          // Overlays
          if (hasTextChanges || hasImgChanges) {
            onParamChange(nodeId, 'text_overlays', serializeTextOverlays(nextText));
            onParamChange(nodeId, 'image_overlays', serializeImageOverlays(nextImg));
          }
        }
      }
    },
    [nodeId, onConfigChange, onParamChange]
  );

  return {
    layers,
    selectedLayerId,
    selectLayer,
    handleLayerPointerDown,
    handleResizePointerDown,
    updateLayerOpacity,
    updateLayerRotation,
    updateLayerZIndex,
    toggleLayerVisibility,
    updateLayerMirror,
    layerRefs,
    isDragging,
    textOverlays,
    imageOverlays,
    addTextOverlay,
    updateTextOverlay,
    removeTextOverlay,
    addImageOverlay,
    updateImageOverlay,
    removeImageOverlay,
    reorderLayers,
  };
};
