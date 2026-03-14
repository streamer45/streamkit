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
 *  - compositorCommit       – commit adapter, throttled persistence
 *  - compositorServerSync   – server-driven layout subscription
 *  - compositorOverlays     – overlay CRUD, layer property updates, reorder
 *  - compositorDragResize   – pointer drag / resize handlers
 *  - compositorLayerParsers – parsing, serialisation, pure helpers
 */

import { useState, useEffect, useCallback, useRef } from 'react';

import { useCompositorCommit } from './compositorCommit';
import type { LayerKind } from './compositorConstants';
import { DEFAULT_CROP_X, DEFAULT_CROP_Y, DEFAULT_CROP_ZOOM } from './compositorConstants';
import { useCompositorDragResize } from './compositorDragResize';
import type { DragState } from './compositorDragResize';
import type { CompositorKeyboardDeps } from './compositorKeyboard';
import {
  mergeOverlayState,
  parseLayers,
  parseImageOverlays,
  parseTextOverlays,
} from './compositorLayerParsers';
import type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
  ResizeHandle,
} from './compositorLayerParsers';
import { useCompositorOverlays } from './compositorOverlays';
import { useServerLayoutSync } from './compositorServerSync';

export type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
  ResizeHandle,
} from './compositorLayerParsers';
export type { LayerKind } from './compositorConstants';

export interface UseCompositorLayersOptions {
  nodeId: string;
  sessionId?: string;
  canvasWidth: number;
  canvasHeight: number;
  params: Record<string, unknown>;
  onConfigChange?: (nodeId: string, config: Record<string, unknown>) => void;
  onParamChange?: (nodeId: string, paramName: string, value: unknown) => void;
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
  updateLayerPositionSize: (
    layerId: string,
    patch: { x?: number; y?: number; width?: number; height?: number }
  ) => void;
  updateLayerZIndex: (layerId: string, zIndex: number) => void;
  toggleLayerVisibility: (layerId: string) => void;
  /** Toggle horizontal or vertical mirroring for a layer (video, text, or image). */
  updateLayerMirror: (layerId: string, axis: 'horizontal' | 'vertical') => void;
  /** Update crop/zoom on a video layer. */
  updateLayerCropZoom: (
    layerId: string,
    patch: { cropX?: number; cropY?: number; cropZoom?: number }
  ) => void;
  /** Ref map: layer elements register here for direct DOM manipulation during drag */
  layerRefs: React.MutableRefObject<Map<string, HTMLDivElement>>;
  /** Refs to the snap guide line DOM elements for direct show/hide during drag */
  snapGuideRefs: React.MutableRefObject<{
    vertical: HTMLDivElement | null;
    horizontal: HTMLDivElement | null;
  }>;
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
  /** Pre-assembled deps bag for useCompositorKeyboard. */
  keyboardDeps: CompositorKeyboardDeps;
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

  const layerRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const snapGuideRefs = useRef<{
    vertical: HTMLDivElement | null;
    horizontal: HTMLDivElement | null;
  }>({ vertical: null, horizontal: null });
  const dragStateRef = useRef<DragState | null>(null);
  const layersRef = useRef(layers);
  useEffect(() => {
    layersRef.current = layers;
  }, [layers]);

  // ── Sync from props ─────────────────────────────────────────────────────
  // In Monitor view (sessionId is set), the server's view data is the source
  // of truth for geometry (x, y, width, height).  The "sync from props" effect
  // must NOT overwrite server-resolved positions with config-parsed ones,
  // otherwise a params echo-back from the server would clobber the accurate
  // layout that useServerLayoutSync applied.
  const isMonitorView = !!sessionId;

  useEffect(() => {
    if (dragStateRef.current) return;
    const parsed = parseLayers(params, canvasWidth, canvasHeight);

    const merged = mergeOverlayState(
      layersRef.current,
      parsed,
      (a, b) => a.cropZoom !== b.cropZoom || a.cropX !== b.cropX || a.cropY !== b.cropY,
      isMonitorView
    );
    if (merged !== layersRef.current) setLayers(merged);

    setTextOverlays((cur) =>
      mergeOverlayState(
        cur,
        parseTextOverlays(params),
        (a, b) =>
          a.text !== b.text ||
          a.fontSize !== b.fontSize ||
          a.fontName !== b.fontName ||
          a.color.some((v, i) => v !== b.color[i]),
        isMonitorView
      )
    );
    setImageOverlays((cur) =>
      mergeOverlayState(
        cur,
        parseImageOverlays(params),
        (a, b) => a.dataBase64 !== b.dataBase64,
        isMonitorView
      )
    );
  }, [params, canvasWidth, canvasHeight, isMonitorView]);

  // ── Server-driven layout (Monitor view only) ───────────────────────────
  useServerLayoutSync(
    sessionId,
    nodeId,
    dragStateRef,
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
      if (t)
        return {
          state: {
            ...t,
            cropZoom: DEFAULT_CROP_ZOOM,
            cropX: DEFAULT_CROP_X,
            cropY: DEFAULT_CROP_Y,
          },
          kind: 'text',
        };
      const img = imageOverlaysRef.current.find((o) => o.id === layerId);
      if (img)
        return {
          state: {
            ...img,
            cropZoom: DEFAULT_CROP_ZOOM,
            cropX: DEFAULT_CROP_X,
            cropY: DEFAULT_CROP_Y,
          },
          kind: 'image',
        };
      return null;
    },
    []
  );

  // ── Commit / persistence ───────────────────────────────────────────────────
  const { commitAdapter, throttledConfigChange, throttledOverlayCommit } = useCompositorCommit({
    nodeId,
    onConfigChange,
    onParamChange,
    throttleMs,
    paramsRef,
    layersRef,
    textOverlaysRef,
    imageOverlaysRef,
  });

  // ── Overlay CRUD, property updates, reorder ─────────────────────────────
  const overlayOps = useCompositorOverlays({
    commitAdapter,
    setLayers,
    setTextOverlays,
    setImageOverlays,
    setSelectedLayerId,
    layersRef,
    textOverlaysRef,
    imageOverlaysRef,
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
    snapGuideRefs,
  });

  return {
    layers,
    selectedLayerId,
    selectLayer: overlayOps.selectLayer,
    handleLayerPointerDown,
    handleResizePointerDown,
    updateLayerOpacity: overlayOps.updateLayerOpacity,
    updateLayerRotation: overlayOps.updateLayerRotation,
    updateLayerPositionSize: overlayOps.updateLayerPositionSize,
    updateLayerZIndex: overlayOps.updateLayerZIndex,
    toggleLayerVisibility: overlayOps.toggleLayerVisibility,
    updateLayerMirror: overlayOps.updateLayerMirror,
    updateLayerCropZoom: overlayOps.updateLayerCropZoom,
    layerRefs,
    snapGuideRefs,
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
    keyboardDeps: {
      selectedLayerId,
      selectLayer: overlayOps.selectLayer,
      removeTextOverlay: overlayOps.removeTextOverlay,
      removeImageOverlay: overlayOps.removeImageOverlay,
      layersRef,
      textOverlaysRef,
      imageOverlaysRef,
      setLayers,
      throttledConfigChange,
      updateTextOverlay: overlayOps.updateTextOverlay,
      updateImageOverlay: overlayOps.updateImageOverlay,
    },
  };
};
