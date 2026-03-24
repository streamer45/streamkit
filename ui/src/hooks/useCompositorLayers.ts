// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook for managing compositor layer state with Jotai atoms.
 *
 * Each layer (video input, text overlay, image overlay) has its own Jotai
 * atom.  Components subscribe to individual atoms for fine-grained
 * reactivity — an opacity change on one layer only re-renders that layer's
 * canvas element and the slider control, not the entire tree.
 *
 * A per-compositor-instance Jotai store (createStore()) scopes atoms so
 * multiple compositor nodes don't share state.  The store is returned in the
 * result for wrapping children in a Provider.
 *
 * Heavy subsystems are extracted into companion modules:
 *  - compositorCommit       – commit adapter, throttled persistence
 *  - compositorServerSync   – server-driven layout subscription
 *  - compositorOverlays     – overlay CRUD, layer property updates, reorder
 *  - compositorDragResize   – pointer drag / resize handlers
 *  - compositorLayerParsers – parsing, serialisation, pure helpers
 *  - compositorAtoms        – Jotai atom definitions and bulk helpers
 */

import { createStore } from 'jotai';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { PARAM_THROTTLE_MS } from '@/constants/timing';

import type { CompositorStore } from './compositorAtoms';
import {
  allImageOverlaysAtom,
  allLayersAtom,
  allTextOverlaysAtom,
  getImageOverlaysFromStore,
  getLayersFromStore,
  getTextOverlaysFromStore,
  isDraggingAtom,
  selectedLayerIdAtom,
  setImageOverlaysInStore,
  setLayersInStore,
  setTextOverlaysInStore,
} from './compositorAtoms';
import { useCompositorCommit } from './compositorCommit';
import type { LayerKind } from './compositorConstants';
import {
  DEFAULT_CROP_SHAPE,
  DEFAULT_CROP_X,
  DEFAULT_CROP_Y,
  DEFAULT_CROP_ZOOM,
} from './compositorConstants';
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
  /** Per-instance Jotai store — wrap children in <Provider store={store}>. */
  store: CompositorStore;
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
    patch: { cropX?: number; cropY?: number; cropZoom?: number; cropShape?: 'rect' | 'circle' }
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
  addTextOverlay: (text: string) => void;
  updateTextOverlay: (id: string, updates: Partial<Omit<TextOverlayState, 'id'>>) => void;
  removeTextOverlay: (id: string) => void;
  addImageOverlay: (dataBase64: string, naturalWidth?: number, naturalHeight?: number) => void;
  updateImageOverlay: (id: string, updates: Partial<Omit<ImageOverlayState, 'id'>>) => void;
  removeImageOverlay: (id: string) => void;
  /** Atomically reassign z-index values for all layer types in one commit.
   *  Each entry maps a layer id + kind to its new z-index. */
  reorderLayers: (entries: Array<{ id: string; kind: LayerKind; zIndex: number }>) => void;
  /** Ref flag: true while a live-mode interaction (slider drag, etc.) is in
   *  progress.  Set by consumers to suppress stale server echo-backs. */
  activeInteractionRef: React.MutableRefObject<boolean>;
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
    throttleMs = PARAM_THROTTLE_MS,
  } = options;

  // ── Per-instance Jotai store ────────────────────────────────────────────
  const store = useMemo(() => {
    const s = createStore();
    // Initialize atoms from params
    const parsed = parseLayers(params, canvasWidth, canvasHeight);
    setLayersInStore(s, parsed);
    setTextOverlaysInStore(s, parseTextOverlays(params));
    setImageOverlaysInStore(s, parseImageOverlays(params));
    return s;
    // Store is created once per compositor instance. Params changes are
    // handled by the sync-from-props effect below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ── Atom-backed setters ─────────────────────────────────────────────────
  // These are drop-in replacements for React.Dispatch<SetStateAction<T>>
  // so sub-hooks (compositorOverlays, compositorDragResize) don't need
  // interface changes.

  const setLayers = useCallback(
    (action: React.SetStateAction<LayerState[]>) => {
      const current = getLayersFromStore(store);
      const next = typeof action === 'function' ? action(current) : action;
      setLayersInStore(store, next);
    },
    [store]
  );

  const setTextOverlays = useCallback(
    (action: React.SetStateAction<TextOverlayState[]>) => {
      const current = getTextOverlaysFromStore(store);
      const next = typeof action === 'function' ? action(current) : action;
      setTextOverlaysInStore(store, next);
    },
    [store]
  );

  const setImageOverlays = useCallback(
    (action: React.SetStateAction<ImageOverlayState[]>) => {
      const current = getImageOverlaysFromStore(store);
      const next = typeof action === 'function' ? action(current) : action;
      setImageOverlaysInStore(store, next);
    },
    [store]
  );

  const setSelectedLayerId = useCallback(
    (action: React.SetStateAction<string | null>) => {
      const current = store.get(selectedLayerIdAtom);
      const next = typeof action === 'function' ? action(current) : action;
      if (next !== current) store.set(selectedLayerIdAtom, next);
    },
    [store]
  );

  const setIsDragging = useCallback(
    (action: React.SetStateAction<boolean>) => {
      const current = store.get(isDraggingAtom);
      const next = typeof action === 'function' ? action(current) : action;
      if (next !== current) store.set(isDraggingAtom, next);
    },
    [store]
  );

  // ── Reactive primitives (no array subscriptions!) ────────────────────────
  // Only subscribe to lightweight primitive atoms to avoid re-rendering
  // CompositorNode on every layer property change.
  const [selectedLayerId, setSelectedLayerIdState] = useState(() => store.get(selectedLayerIdAtom));
  const [isDragging, setIsDraggingState] = useState(() => store.get(isDraggingAtom));

  useEffect(() => {
    const unsub1 = store.sub(selectedLayerIdAtom, () => {
      setSelectedLayerIdState(store.get(selectedLayerIdAtom));
    });
    const unsub2 = store.sub(isDraggingAtom, () => {
      setIsDraggingState(store.get(isDraggingAtom));
    });
    return () => {
      unsub1();
      unsub2();
    };
  }, [store]);

  // ── Stable refs ─────────────────────────────────────────────────────────
  // Sub-hooks read these in callbacks to get the latest values without
  // triggering dependency changes.  Synced from atom subscriptions.
  const paramsRef = useRef(params);
  useEffect(() => {
    paramsRef.current = params;
  }, [params]);

  const layersRef = useRef<LayerState[]>(getLayersFromStore(store));
  const textOverlaysRef = useRef<TextOverlayState[]>(getTextOverlaysFromStore(store));
  const imageOverlaysRef = useRef<ImageOverlayState[]>(getImageOverlaysFromStore(store));

  useEffect(() => {
    const unsub1 = store.sub(allLayersAtom, () => {
      layersRef.current = getLayersFromStore(store);
    });
    const unsub2 = store.sub(allTextOverlaysAtom, () => {
      textOverlaysRef.current = getTextOverlaysFromStore(store);
    });
    const unsub3 = store.sub(allImageOverlaysAtom, () => {
      imageOverlaysRef.current = getImageOverlaysFromStore(store);
    });
    return () => {
      unsub1();
      unsub2();
      unsub3();
    };
  }, [store]);

  const layerRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const snapGuideRefs = useRef<{
    vertical: HTMLDivElement | null;
    horizontal: HTMLDivElement | null;
  }>({ vertical: null, horizontal: null });
  const dragStateRef = useRef<DragState | null>(null);

  // Per-node flag: true while any live-mode interaction (slider drag, etc.)
  // is in progress.  Guards useServerLayoutSync so stale server geometry
  // doesn't overwrite in-flight client state.
  const activeInteractionRef = useRef(false);

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

  // ── Sync from props ─────────────────────────────────────────────────────
  // In Monitor view (sessionId is set), the server's view data is the source
  // of truth for geometry.  The "sync from props" effect must NOT overwrite
  // server-resolved positions with config-parsed ones.
  const isMonitorView = !!sessionId;

  // Track previous parsed results so sync-from-props can detect which
  // config fields ACTUALLY changed in the new params vs the previous parse.
  // Without this, topology rebuilds with stale params overwrite local
  // inspector edits (crop/zoom on video layers, color alpha on text, etc.).
  const prevParsedLayersRef = useRef<LayerState[]>([]);
  const prevParsedTextRef = useRef<TextOverlayState[]>([]);
  const prevParsedImgRef = useRef<ImageOverlayState[]>([]);

  useEffect(() => {
    // Skip during pointer drag/resize — atoms already have the latest local
    // value and the sync would be a no-op in Monitor view (preserveGeometry
    // keeps OverlayBase fields from existing state).
    if (dragStateRef.current) return;

    const parsed = parseLayers(params, canvasWidth, canvasHeight);
    const currentLayers = getLayersFromStore(store);

    const merged = mergeOverlayState(
      currentLayers,
      parsed,
      (a, b) =>
        a.cropZoom !== b.cropZoom ||
        a.cropX !== b.cropX ||
        a.cropY !== b.cropY ||
        a.cropShape !== b.cropShape,
      isMonitorView,
      isMonitorView ? prevParsedLayersRef.current : undefined
    );
    if (merged !== currentLayers) setLayersInStore(store, merged);
    prevParsedLayersRef.current = parsed;

    const parsedText = parseTextOverlays(params);
    const currentText = getTextOverlaysFromStore(store);
    const mergedText = mergeOverlayState(
      currentText,
      parsedText,
      (a, b) =>
        a.text !== b.text ||
        a.fontSize !== b.fontSize ||
        a.fontName !== b.fontName ||
        a.color.some((v, i) => v !== b.color[i]),
      isMonitorView,
      isMonitorView ? prevParsedTextRef.current : undefined
    );
    if (mergedText !== currentText) setTextOverlaysInStore(store, mergedText);
    prevParsedTextRef.current = parsedText;

    const parsedImg = parseImageOverlays(params);
    const currentImg = getImageOverlaysFromStore(store);
    const mergedImg = mergeOverlayState(
      currentImg,
      parsedImg,
      (a, b) => a.dataBase64 !== b.dataBase64,
      isMonitorView,
      isMonitorView ? prevParsedImgRef.current : undefined
    );
    if (mergedImg !== currentImg) setImageOverlaysInStore(store, mergedImg);
    prevParsedImgRef.current = parsedImg;
  }, [params, canvasWidth, canvasHeight, isMonitorView, store]);

  // ── Server-driven layout (Monitor view only) ───────────────────────────
  useServerLayoutSync(sessionId, nodeId, store, dragStateRef, activeInteractionRef);

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
            cropShape: DEFAULT_CROP_SHAPE,
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
            cropShape: DEFAULT_CROP_SHAPE,
          },
          kind: 'image',
        };
      return null;
    },
    []
  );

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
    store,
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
    addTextOverlay: overlayOps.addTextOverlay,
    updateTextOverlay: overlayOps.updateTextOverlay,
    removeTextOverlay: overlayOps.removeTextOverlay,
    addImageOverlay: overlayOps.addImageOverlay,
    updateImageOverlay: overlayOps.updateImageOverlay,
    removeImageOverlay: overlayOps.removeImageOverlay,
    reorderLayers: overlayOps.reorderLayers,
    activeInteractionRef,
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
