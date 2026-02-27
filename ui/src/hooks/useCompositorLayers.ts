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

export interface UseCompositorLayersResult {
  layers: LayerState[];
  selectedLayerId: string | null;
  selectLayer: (id: string | null) => void;
  handleLayerPointerDown: (layerId: string, e: React.PointerEvent) => void;
  handleResizePointerDown: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  updateLayerOpacity: (layerId: string, opacity: number) => void;
  updateLayerRotation: (layerId: string, degrees: number) => void;
  updateLayerZIndex: (layerId: string, zIndex: number) => void;
  /** Ref map: layer elements register here for direct DOM manipulation during drag */
  layerRefs: React.MutableRefObject<Map<string, HTMLDivElement>>;
  /** Whether a drag/resize is currently in progress */
  isDragging: boolean;
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
    }))
    .sort((a, b) => a.zIndex - b.zIndex);
}

/** Build the full compositor config from current params + updated layers */
function buildConfig(
  params: Record<string, unknown>,
  layers: LayerState[]
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
      opacity: Math.round(layer.opacity * 100) / 100,
      z_index: layer.zIndex,
      rotation_degrees: Math.round(layer.rotationDegrees * 10) / 10,
    };
  }

  return {
    width: params.width ?? 1280,
    height: params.height ?? 720,
    output_pixel_format: params.output_pixel_format ?? 'rgba8',
    layers: layersMap,
    image_overlays: params.image_overlays ?? [],
    text_overlays: params.text_overlays ?? [],
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
  const [selectedLayerId, setSelectedLayerId] = useState<string | null>(null);
  const [isDragging, setIsDragging] = useState(false);

  // Refs for zero-render drag/resize
  const layerRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const dragStateRef = useRef<{
    type: 'drag' | 'resize';
    layerId: string;
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
    setLayers(parsed);
  }, [params, canvasWidth, canvasHeight]);

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
              opacity: Math.round(layer.opacity * 100) / 100,
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

  // Cleanup throttle on unmount
  useEffect(
    () => () => {
      throttledConfigChange?.cancel();
    },
    [throttledConfigChange]
  );

  /** Compute updated layer from current pointer position */
  const computeUpdatedLayer = useCallback(
    (
      state: NonNullable<typeof dragStateRef.current>,
      clientX: number,
      clientY: number
    ): LayerState => {
      const dx = (clientX - state.startX) / state.scale;
      const dy = (clientY - state.startY) / state.scale;
      const orig = state.origLayer;

      if (state.type === 'drag') {
        return { ...orig, x: orig.x + dx, y: orig.y + dy };
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

      // Commit to React state
      setLayers((prev) => prev.map((l) => (l.id === updated.id ? updated : l)));
      setIsDragging(false);

      // Send to server
      const newLayers = layersRef.current.map((l) => (l.id === updated.id ? updated : l));
      throttledConfigChange?.(newLayers);

      dragStateRef.current = null;
      document.removeEventListener('pointermove', handlePointerMove);
      document.removeEventListener('pointerup', handlePointerUp);
    },
    [computeUpdatedLayer, throttledConfigChange, handlePointerMove]
  );

  /** Start dragging a layer */
  const handleLayerPointerDown = useCallback(
    (layerId: string, e: React.PointerEvent) => {
      e.stopPropagation();
      e.preventDefault();

      const layer = layersRef.current.find((l) => l.id === layerId);
      if (!layer) return;

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
        startX: e.clientX,
        startY: e.clientY,
        origLayer: { ...layer },
        scale,
        rafId: null,
        currentX: e.clientX,
        currentY: e.clientY,
      };

      setIsDragging(true);
      document.addEventListener('pointermove', handlePointerMove);
      document.addEventListener('pointerup', handlePointerUp);
    },
    [canvasWidth, handlePointerMove, handlePointerUp]
  );

  /** Start resizing a layer */
  const handleResizePointerDown = useCallback(
    (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => {
      e.stopPropagation();
      e.preventDefault();

      const layer = layersRef.current.find((l) => l.id === layerId);
      if (!layer) return;

      const el = layerRefs.current.get(layerId);
      const container = el?.parentElement;
      const scale = container
        ? container.getBoundingClientRect().width /
          (Number(container.dataset.canvasWidth) || canvasWidth)
        : 1;

      dragStateRef.current = {
        type: 'resize',
        layerId,
        handle,
        startX: e.clientX,
        startY: e.clientY,
        origLayer: { ...layer },
        scale,
        rafId: null,
        currentX: e.clientX,
        currentY: e.clientY,
      };

      setIsDragging(true);
      document.addEventListener('pointermove', handlePointerMove);
      document.addEventListener('pointerup', handlePointerUp);
    },
    [canvasWidth, handlePointerMove, handlePointerUp]
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

  return {
    layers,
    selectedLayerId,
    selectLayer,
    handleLayerPointerDown,
    handleResizePointerDown,
    updateLayerOpacity,
    updateLayerRotation,
    updateLayerZIndex,
    layerRefs,
    isDragging,
  };
};
