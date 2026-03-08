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
 *
 * Heavy subsystems are extracted into companion modules:
 *  - compositorServerSync   – server-driven layout subscription
 *  - compositorOverlays     – overlay CRUD, layer property updates, reorder
 *  - compositorDragResize   – pointer drag / resize handlers
 *  - compositorLayerParsers – parsing, serialisation, pure helpers
 */

import { throttle } from 'lodash-es';
import { useState, useEffect, useCallback, useRef, useMemo } from 'react';

import { useCompositorDragResize } from './compositorDragResize';
import type { DragState } from './compositorDragResize';
import {
  buildConfig,
  mergeOverlayState,
  OVERLAY_COMMIT_GUARD_MS,
  parseLayers,
  parseImageOverlays,
  parseTextOverlays,
  serializeImageOverlays,
  serializeLayers,
  serializeTextOverlays,
} from './compositorLayerParsers';
import type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
  ResizeHandle,
  LayerKind,
} from './compositorLayerParsers';
import { useCompositorOverlays } from './compositorOverlays';
import { useServerLayoutSync } from './compositorServerSync';

export type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
  ResizeHandle,
  LayerKind,
} from './compositorLayerParsers';

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

  // Stable refs — let throttled / memoised callbacks read latest values
  // at call-time without triggering cascading dependency changes.
  const paramsRef = useRef(params);
  useEffect(() => {
    paramsRef.current = params;
  }, [params]);

  const textOverlaysRef = useRef(textOverlays);
  const imageOverlaysRef = useRef(imageOverlays);
  useEffect(() => {
    textOverlaysRef.current = textOverlays;
  }, [textOverlays]);
  useEffect(() => {
    imageOverlaysRef.current = imageOverlays;
  }, [imageOverlays]);

  const overlayCommitGuardRef = useRef<number>(0);

  const layerRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const dragStateRef = useRef<DragState | null>(null);
  const layersRef = useRef(layers);
  useEffect(() => {
    layersRef.current = layers;
  }, [layers]);

  // ── Sync from props ─────────────────────────────────────────────────────
  useEffect(() => {
    if (dragStateRef.current) return;
    const parsed = parseLayers(params, canvasWidth, canvasHeight);

    const merged = mergeOverlayState(layersRef.current, parsed);
    if (merged !== layersRef.current) setLayers(merged);

    const sinceCommit = Date.now() - overlayCommitGuardRef.current;
    if (sinceCommit < OVERLAY_COMMIT_GUARD_MS) return;

    setTextOverlays((cur) =>
      mergeOverlayState(
        cur,
        parseTextOverlays(params),
        (a, b) =>
          a.text !== b.text ||
          a.fontSize !== b.fontSize ||
          a.fontName !== b.fontName ||
          a.color.some((v, i) => v !== b.color[i])
      )
    );
    setImageOverlays((cur) => mergeOverlayState(cur, parseImageOverlays(params)));
  }, [params, canvasWidth, canvasHeight]);

  // ── Server-driven layout (Monitor view only) ───────────────────────────
  const isDraggingRef = useRef(false);
  useEffect(() => {
    isDraggingRef.current = !!dragStateRef.current;
  });
  useServerLayoutSync(
    sessionId,
    nodeId,
    isDraggingRef,
    setLayers,
    setTextOverlays,
    setImageOverlays
  );

  // ── Find layer across all types ─────────────────────────────────────────
  const findAnyLayer = useCallback(
    (layerId: string): { state: LayerState; kind: LayerKind } | null => {
      const v = layersRef.current.find((l) => l.id === layerId);
      if (v) return { state: v, kind: 'video' };
      const t = textOverlaysRef.current.find((o) => o.id === layerId);
      if (t) return { state: t, kind: 'text' };
      const img = imageOverlaysRef.current.find((o) => o.id === layerId);
      if (img) return { state: img, kind: 'image' };
      return null;
    },
    []
  );

  // ── Throttled commit helpers ────────────────────────────────────────────
  const throttledConfigChange = useMemo(() => {
    if (!onConfigChange && !onParamChange) return null;
    return throttle(
      (currentLayers: LayerState[]) => {
        if (onConfigChange) {
          const config = buildConfig(
            paramsRef.current,
            currentLayers,
            textOverlaysRef.current,
            imageOverlaysRef.current
          );
          onConfigChange(nodeId, config);
        } else if (onParamChange) {
          onParamChange(nodeId, 'layers', serializeLayers(currentLayers));
        }
      },
      throttleMs,
      { leading: true, trailing: true }
    );
  }, [nodeId, onConfigChange, onParamChange, throttleMs]);

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

  useEffect(
    () => () => {
      throttledConfigChange?.cancel();
      throttledOverlayCommit?.cancel();
    },
    [throttledConfigChange, throttledOverlayCommit]
  );

  // ── Overlay CRUD, property updates, reorder ─────────────────────────────
  const overlayOps = useCompositorOverlays({
    nodeId,
    onConfigChange,
    onParamChange,
    setLayers,
    setTextOverlays,
    setImageOverlays,
    setSelectedLayerId,
    layersRef,
    textOverlaysRef,
    imageOverlaysRef,
    paramsRef,
    overlayCommitGuardRef,
    throttledConfigChange,
    throttledOverlayCommit,
  });

  // ── Drag / resize handlers ──────────────────────────────────────────────
  const { handleLayerPointerDown, handleResizePointerDown } = useCompositorDragResize({
    canvasWidth,
    canvasHeight,
    dragStateRef,
    layerRefs,
    layersRef,
    textOverlaysRef,
    imageOverlaysRef,
    setLayers,
    setTextOverlays,
    setImageOverlays,
    setSelectedLayerId,
    setIsDragging,
    findAnyLayer,
    throttledConfigChange,
    commitOverlaysRef: overlayOps.commitOverlaysRef,
  });

  return {
    layers,
    selectedLayerId,
    selectLayer: overlayOps.selectLayer,
    handleLayerPointerDown,
    handleResizePointerDown,
    updateLayerOpacity: overlayOps.updateLayerOpacity,
    updateLayerRotation: overlayOps.updateLayerRotation,
    updateLayerZIndex: overlayOps.updateLayerZIndex,
    toggleLayerVisibility: overlayOps.toggleLayerVisibility,
    updateLayerMirror: overlayOps.updateLayerMirror,
    layerRefs,
    isDragging,
    textOverlays,
    imageOverlays,
    addTextOverlay: overlayOps.addTextOverlay,
    updateTextOverlay: overlayOps.updateTextOverlay,
    removeTextOverlay: overlayOps.removeTextOverlay,
    addImageOverlay: overlayOps.addImageOverlay,
    updateImageOverlay: overlayOps.updateImageOverlay,
    removeImageOverlay: overlayOps.removeImageOverlay,
    reorderLayers: overlayOps.reorderLayers,
  };
};
