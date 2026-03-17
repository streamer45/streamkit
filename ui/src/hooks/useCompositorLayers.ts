// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook for managing compositor layer state.
 *
 * Drag/resize still uses the zero-render ref + requestAnimationFrame path for
 * pointer-move performance, but layer appearance updates (opacity / rotation)
 * now go through normal React state. Jotai keeps that state scoped per
 * compositor node instance and removes the need for the old sliderActiveRef /
 * direct-DOM mutation hack.
 */

import { useAtom } from 'jotai';
import { useEffect, useLayoutEffect, useCallback, useRef } from 'react';

import { PARAM_THROTTLE_MS } from '@/constants/timing';
import {
  cleanupCompositorAtoms,
  compositorImageOverlaysAtom,
  compositorIsDraggingAtom,
  compositorLayersAtom,
  compositorSelectedLayerAtom,
  compositorTextOverlaysAtom,
} from '@/stores/compositorAtoms';

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
  ImageOverlayState,
  LayerState,
  ResizeHandle,
  TextOverlayState,
} from './compositorLayerParsers';
import { useCompositorOverlays } from './compositorOverlays';
import { useServerLayoutSync } from './compositorServerSync';

export type {
  ImageOverlayState,
  LayerState,
  ResizeHandle,
  TextOverlayState,
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
  updateLayerMirror: (layerId: string, axis: 'horizontal' | 'vertical') => void;
  updateLayerCropZoom: (
    layerId: string,
    patch: { cropX?: number; cropY?: number; cropZoom?: number }
  ) => void;
  layerRefs: React.MutableRefObject<Map<string, HTMLDivElement>>;
  snapGuideRefs: React.MutableRefObject<{
    vertical: HTMLDivElement | null;
    horizontal: HTMLDivElement | null;
  }>;
  isDragging: boolean;
  textOverlays: TextOverlayState[];
  imageOverlays: ImageOverlayState[];
  addTextOverlay: (text: string) => void;
  updateTextOverlay: (id: string, updates: Partial<Omit<TextOverlayState, 'id'>>) => void;
  removeTextOverlay: (id: string) => void;
  addImageOverlay: (dataBase64: string, naturalWidth?: number, naturalHeight?: number) => void;
  updateImageOverlay: (id: string, updates: Partial<Omit<ImageOverlayState, 'id'>>) => void;
  removeImageOverlay: (id: string) => void;
  reorderLayers: (entries: Array<{ id: string; kind: LayerKind; zIndex: number }>) => void;
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
    throttleMs = PARAM_THROTTLE_MS,
  } = options;

  const [layers, setLayers] = useAtom(compositorLayersAtom(nodeId));
  const [textOverlays, setTextOverlays] = useAtom(compositorTextOverlaysAtom(nodeId));
  const [imageOverlays, setImageOverlays] = useAtom(compositorImageOverlaysAtom(nodeId));
  const [selectedLayerId, setSelectedLayerId] = useAtom(compositorSelectedLayerAtom(nodeId));
  const [isDragging, setIsDragging] = useAtom(compositorIsDraggingAtom(nodeId));

  const layerRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const snapGuideRefs = useRef<{
    vertical: HTMLDivElement | null;
    horizontal: HTMLDivElement | null;
  }>({ vertical: null, horizontal: null });
  const dragStateRef = useRef<DragState | null>(null);

  // Stable refs — let throttled / memoised callbacks read latest values
  // at call-time without triggering dependency churn.
  const paramsRef = useRef(params);
  const layersRef = useRef(layers);
  const textOverlaysRef = useRef(textOverlays);
  const imageOverlaysRef = useRef(imageOverlays);

  useEffect(() => {
    paramsRef.current = params;
  }, [params]);
  useEffect(() => {
    layersRef.current = layers;
  }, [layers]);
  useEffect(() => {
    textOverlaysRef.current = textOverlays;
  }, [textOverlays]);
  useEffect(() => {
    imageOverlaysRef.current = imageOverlays;
  }, [imageOverlays]);

  // Clean up atom family entries when this compositor node unmounts or
  // nodeId changes, so stale atoms don't accumulate.
  useEffect(() => {
    return () => {
      cleanupCompositorAtoms(nodeId);
    };
  }, [nodeId]);

  // ── Sync from props ─────────────────────────────────────────────────────
  // In Monitor view (sessionId is set), the server's view data is the source
  // of truth for geometry (x, y, width, height). The sync-from-props effect
  // must NOT overwrite server-resolved positions with config-parsed ones.
  const isMonitorView = !!sessionId;

  // useLayoutEffect so atoms are populated before the first paint, avoiding a
  // flash of empty state (atoms default to []).
  useLayoutEffect(() => {
    if (dragStateRef.current) return;

    const parsedLayers = parseLayers(params, canvasWidth, canvasHeight);
    const mergedLayers = mergeOverlayState(
      layersRef.current,
      parsedLayers,
      (a, b) => a.cropZoom !== b.cropZoom || a.cropX !== b.cropX || a.cropY !== b.cropY,
      isMonitorView
    );
    if (mergedLayers !== layersRef.current) {
      setLayers(mergedLayers);
    }

    setTextOverlays((current) =>
      mergeOverlayState(
        current,
        parseTextOverlays(params),
        (a, b) =>
          a.text !== b.text ||
          a.fontSize !== b.fontSize ||
          a.fontName !== b.fontName ||
          a.color.some((value, index) => value !== b.color[index]),
        isMonitorView
      )
    );

    setImageOverlays((current) =>
      mergeOverlayState(
        current,
        parseImageOverlays(params),
        (a, b) => a.dataBase64 !== b.dataBase64,
        isMonitorView
      )
    );
  }, [
    params,
    canvasWidth,
    canvasHeight,
    isMonitorView,
    setLayers,
    setTextOverlays,
    setImageOverlays,
  ]);

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
      const videoLayer = layersRef.current.find((layer) => layer.id === layerId);
      if (videoLayer) {
        return { state: videoLayer, kind: 'video' };
      }

      const textOverlay = textOverlaysRef.current.find((overlay) => overlay.id === layerId);
      if (textOverlay) {
        return {
          state: {
            ...textOverlay,
            cropZoom: DEFAULT_CROP_ZOOM,
            cropX: DEFAULT_CROP_X,
            cropY: DEFAULT_CROP_Y,
          },
          kind: 'text',
        };
      }

      const imageOverlay = imageOverlaysRef.current.find((overlay) => overlay.id === layerId);
      if (imageOverlay) {
        return {
          state: {
            ...imageOverlay,
            cropZoom: DEFAULT_CROP_ZOOM,
            cropX: DEFAULT_CROP_X,
            cropY: DEFAULT_CROP_Y,
          },
          kind: 'image',
        };
      }

      return null;
    },
    []
  );

  // ── Commit / persistence ────────────────────────────────────────────────
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
