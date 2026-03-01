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
  opacity: number;
  rotationDegrees: number;
  zIndex: number;
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
  /** Client-side visibility toggle (hidden overlays send opacity=0 to backend) */
  visible: boolean;
}

/** Which edge/corner is being resized */
export type ResizeHandle = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw';

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
  addImageOverlay: (dataBase64: string) => void;
  updateImageOverlay: (id: string, updates: Partial<Omit<ImageOverlayState, 'id'>>) => void;
  removeImageOverlay: (id: string) => void;
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
}

interface TextOverlayConfig {
  text: string;
  rect: Rect;
  color?: [number, number, number, number];
  font_size?: number;
  opacity?: number;
  rotation_degrees?: number;
  z_index?: number;
}

interface ImageOverlayConfig {
  data_base64: string;
  rect: Rect;
  opacity?: number;
  rotation_degrees?: number;
  z_index?: number;
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
      visible: true,
    }))
    .sort((a, b) => a.zIndex - b.zIndex);
}

/** Parse text overlays from compositor params */
function parseTextOverlays(params: Record<string, unknown>): TextOverlayState[] {
  const overlays = params.text_overlays as TextOverlayConfig[] | undefined;
  if (!Array.isArray(overlays)) return [];
  return overlays.map((o, i) => ({
    id: `text_${i}`,
    text: o.text ?? '',
    x: o.rect?.x ?? 0,
    y: o.rect?.y ?? 0,
    width: o.rect?.width ?? 200,
    height: o.rect?.height ?? 40,
    color: o.color ?? [255, 255, 255, 255],
    fontSize: o.font_size ?? 24,
    opacity: o.opacity ?? 1.0,
    rotationDegrees: o.rotation_degrees ?? 0,
    zIndex: o.z_index ?? 0,
    visible: true,
  }));
}

/** Parse image overlays from compositor params */
function parseImageOverlays(params: Record<string, unknown>): ImageOverlayState[] {
  const overlays = params.image_overlays as ImageOverlayConfig[] | undefined;
  if (!Array.isArray(overlays)) return [];
  return overlays.map((o, i) => ({
    id: `img_${i}`,
    dataBase64: o.data_base64 ?? '',
    x: o.rect?.x ?? 0,
    y: o.rect?.y ?? 0,
    width: o.rect?.width ?? 200,
    height: o.rect?.height ?? 200,
    opacity: o.opacity ?? 1.0,
    rotationDegrees: o.rotation_degrees ?? 0,
    zIndex: o.z_index ?? 0,
    visible: true,
  }));
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
    opacity: o.visible ? Math.round(o.opacity * 100) / 100 : 0,
    rotation_degrees: Math.round(o.rotationDegrees * 10) / 10,
    z_index: o.zIndex,
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
  }));
}

/** Build the full compositor config from current params + updated layers */
function buildConfig(
  params: Record<string, unknown>,
  layers: LayerState[],
  textOverlays?: TextOverlayState[],
  imageOverlays?: ImageOverlayState[]
): Record<string, unknown> {
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
    };
  }

  return {
    width: params.width ?? 1280,
    height: params.height ?? 720,
    output_pixel_format: params.output_pixel_format ?? 'rgba8',
    layers: layersMap,
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
    // Preserve client-side `visible` state from existing layers
    const current = layersRef.current;
    const merged = parsed.map((p) => {
      const existing = current.find((l) => l.id === p.id);
      return existing
        ? {
            ...p,
            visible: existing.visible,
            opacity: existing.visible ? p.opacity : existing.opacity,
          }
        : p;
    });

    // Issue #4 fix: only update layers state if the merged result actually
    // differs from current state.  This prevents unnecessary re-renders that
    // cause positions to briefly revert when switching selected layers.
    const layersChanged =
      merged.length !== current.length ||
      merged.some(
        (m, i) =>
          m.id !== current[i].id ||
          m.x !== current[i].x ||
          m.y !== current[i].y ||
          m.width !== current[i].width ||
          m.height !== current[i].height ||
          m.opacity !== current[i].opacity ||
          m.zIndex !== current[i].zIndex ||
          m.rotationDegrees !== current[i].rotationDegrees ||
          m.visible !== current[i].visible
      );
    if (layersChanged) {
      setLayers(merged);
    }

    // Skip overlay re-parse if we just committed a local overlay change.
    // This prevents stale params from overwriting the local removal/add.
    const sinceCommit = Date.now() - overlayCommitGuardRef.current;
    if (sinceCommit < 3000) return;

    setTextOverlays((currentText) => {
      const parsed = parseTextOverlays(params);
      const merged = parsed.map((p) => {
        const existing = currentText.find((o) => o.id === p.id);
        if (existing) {
          return {
            ...p,
            visible: existing.visible,
            opacity: existing.visible ? p.opacity : existing.opacity,
          };
        }
        return p;
      });
      // Skip update if nothing changed
      const changed =
        merged.length !== currentText.length ||
        merged.some(
          (m, i) =>
            m.id !== currentText[i].id ||
            m.x !== currentText[i].x ||
            m.y !== currentText[i].y ||
            m.width !== currentText[i].width ||
            m.height !== currentText[i].height ||
            m.opacity !== currentText[i].opacity ||
            m.rotationDegrees !== currentText[i].rotationDegrees ||
            m.text !== currentText[i].text ||
            m.fontSize !== currentText[i].fontSize ||
            m.visible !== currentText[i].visible
        );
      return changed ? merged : currentText;
    });
    setImageOverlays((currentImg) => {
      const parsed = parseImageOverlays(params);
      const merged = parsed.map((p) => {
        const existing = currentImg.find((o) => o.id === p.id);
        if (existing) {
          return {
            ...p,
            visible: existing.visible,
            opacity: existing.visible ? p.opacity : existing.opacity,
          };
        }
        return p;
      });
      // Skip update if nothing changed
      const changed =
        merged.length !== currentImg.length ||
        merged.some(
          (m, i) =>
            m.id !== currentImg[i].id ||
            m.x !== currentImg[i].x ||
            m.y !== currentImg[i].y ||
            m.width !== currentImg[i].width ||
            m.height !== currentImg[i].height ||
            m.opacity !== currentImg[i].opacity ||
            m.visible !== currentImg[i].visible
        );
      return changed ? merged : currentImg;
    });
  }, [params, canvasWidth, canvasHeight]);

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
            zIndex: 0,
            rotationDegrees: textOverlay.rotationDegrees,
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
            zIndex: 0,
            rotationDegrees: 0,
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
          const config = buildConfig(params, currentLayers);
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
            };
          }
          onParamChange(nodeId, 'layers', layersMap);
        }
      },
      throttleMs,
      { leading: true, trailing: true }
    );
  }, [nodeId, onConfigChange, onParamChange, params, throttleMs]);

  // Throttled overlay commit for continuous updates (sliders, drag, etc.)
  // Prevents flooding the server with config changes on every slider tick.
  const throttledOverlayCommit = useMemo(() => {
    if (!onConfigChange && !onParamChange) return null;
    return throttle(
      (nextText: TextOverlayState[], nextImg: ImageOverlayState[]) => {
        if (onConfigChange) {
          const config = buildConfig(params, layersRef.current, nextText, nextImg);
          onConfigChange(nodeId, config);
        } else if (onParamChange) {
          onParamChange(nodeId, 'text_overlays', serializeTextOverlays(nextText));
          onParamChange(nodeId, 'image_overlays', serializeImageOverlays(nextImg));
        }
      },
      throttleMs,
      { leading: true, trailing: true }
    );
  }, [nodeId, onConfigChange, onParamChange, params, throttleMs]);

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
        return { ...orig, x: orig.x + rawDx, y: orig.y + rawDy };
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

      // Constrain video layer resize to maintain aspect ratio.
      // Text/image overlays (layerKind !== 'video') are not constrained here
      // because text overlays no longer have resize handles and image overlays
      // may intentionally use different dimensions.
      if (state.layerKind === 'video' && orig.width > 0 && orig.height > 0) {
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
    []
  );

  /** Apply visual update to DOM element (no React state) */
  const applyVisualUpdate = useCallback((layer: LayerState) => {
    const el = layerRefs.current.get(layer.id);
    if (!el) return;
    el.style.left = `${layer.x}px`;
    el.style.top = `${layer.y}px`;
    el.style.width = `${layer.width}px`;
    el.style.height = `${layer.height}px`;
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
        applyVisualUpdate(updated);
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
        // Commit text overlay position/size
        setTextOverlays((prev) => {
          const next = prev.map((o) =>
            o.id === updated.id
              ? { ...o, x: updated.x, y: updated.y, width: updated.width, height: updated.height }
              : o
          );
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

  // ── Overlay commit helper ─────────────────────────────────────────────────

  const commitOverlays = useCallback(
    (nextText: TextOverlayState[], nextImg: ImageOverlayState[]) => {
      // Arm the guard so the sync effect won't overwrite overlays with
      // stale params before the backend echoes back this change.
      overlayCommitGuardRef.current = Date.now();

      if (onConfigChange) {
        const config = buildConfig(params, layersRef.current, nextText, nextImg);
        onConfigChange(nodeId, config);
      } else if (onParamChange) {
        onParamChange(nodeId, 'text_overlays', serializeTextOverlays(nextText));
        onParamChange(nodeId, 'image_overlays', serializeImageOverlays(nextImg));
      }
    },
    [nodeId, onConfigChange, onParamChange, params]
  );

  // Keep a stable ref so pointer-up can call the latest commitOverlays without
  // adding it to its dependency array (which would re-create event listeners).
  const commitOverlaysRef = useRef(commitOverlays);
  useEffect(() => {
    commitOverlaysRef.current = commitOverlays;
  }, [commitOverlays]);

  // ── Text overlay CRUD ─────────────────────────────────────────────────────

  const addTextOverlay = useCallback(
    (text: string) => {
      setTextOverlays((prev) => {
        const next: TextOverlayState[] = [
          ...prev,
          {
            id: `text_${prev.length}`,
            text,
            x: 40,
            y: 40 + prev.length * 50,
            width: 200,
            height: 40,
            color: [255, 255, 255, 255],
            fontSize: 24,
            opacity: 1.0,
            rotationDegrees: 0,
            zIndex: 0,
            visible: true,
          },
        ];
        commitOverlays(next, imageOverlaysRef.current);
        return next;
      });
    },
    [commitOverlays]
  );

  const updateTextOverlay = useCallback(
    (id: string, updates: Partial<Omit<TextOverlayState, 'id'>>) => {
      // Arm the guard immediately so sync effect won't overwrite
      overlayCommitGuardRef.current = Date.now();
      setTextOverlays((prev) => {
        const next = prev.map((o) => (o.id === id ? { ...o, ...updates } : o));
        // Use throttled commit to avoid flooding the server on slider drags
        if (throttledOverlayCommit) {
          throttledOverlayCommit(next, imageOverlaysRef.current);
        } else {
          commitOverlaysRef.current(next, imageOverlaysRef.current);
        }
        return next;
      });
    },
    [throttledOverlayCommit]
  );

  const removeTextOverlay = useCallback(
    (id: string) => {
      setTextOverlays((prev) => {
        const next = prev.filter((o) => o.id !== id).map((o, i) => ({ ...o, id: `text_${i}` }));
        commitOverlays(next, imageOverlaysRef.current);
        return next;
      });
      // Clear selection — re-indexing makes the old selectedLayerId stale
      setSelectedLayerId(null);
    },
    [commitOverlays]
  );

  // ── Image overlay CRUD ────────────────────────────────────────────────────

  const addImageOverlay = useCallback(
    (dataBase64: string) => {
      setImageOverlays((prev) => {
        const next: ImageOverlayState[] = [
          ...prev,
          {
            id: `img_${prev.length}`,
            dataBase64,
            x: 40,
            y: 40 + prev.length * 60,
            width: 200,
            height: 200,
            opacity: 1.0,
            rotationDegrees: 0,
            zIndex: 0,
            visible: true,
          },
        ];
        commitOverlays(textOverlaysRef.current, next);
        return next;
      });
    },
    [commitOverlays]
  );

  const updateImageOverlay = useCallback(
    (id: string, updates: Partial<Omit<ImageOverlayState, 'id'>>) => {
      // Arm the guard immediately so sync effect won't overwrite
      overlayCommitGuardRef.current = Date.now();
      setImageOverlays((prev) => {
        const next = prev.map((o) => (o.id === id ? { ...o, ...updates } : o));
        // Use throttled commit to avoid flooding the server on slider drags
        if (throttledOverlayCommit) {
          throttledOverlayCommit(textOverlaysRef.current, next);
        } else {
          commitOverlaysRef.current(textOverlaysRef.current, next);
        }
        return next;
      });
    },
    [throttledOverlayCommit]
  );

  const removeImageOverlay = useCallback(
    (id: string) => {
      setImageOverlays((prev) => {
        const next = prev.filter((o) => o.id !== id).map((o, i) => ({ ...o, id: `img_${i}` }));
        commitOverlays(textOverlaysRef.current, next);
        return next;
      });
      // Clear selection — re-indexing makes the old selectedLayerId stale
      setSelectedLayerId(null);
    },
    [commitOverlays]
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
  };
};
