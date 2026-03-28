// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Zero-render drag / resize logic for compositor layers.
 *
 * During pointer-driven interactions, visual updates are applied directly to
 * DOM elements via refs and requestAnimationFrame.  React state is only
 * committed on pointer-up, keeping the experience butter-smooth.
 */

import { useCallback, useEffect, useRef } from 'react';

import { computeUpdatedLayer, detectSnapGuides } from './compositorLayerParsers';
import type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
  ResizeHandle,
  LayerKind,
} from './compositorLayerParsers';

// ── Drag state ref type ──────────────────────────────────────────────────

export interface DragState {
  type: 'drag' | 'resize';
  layerId: string;
  layerKind: LayerKind;
  handle?: ResizeHandle;
  startX: number;
  startY: number;
  origLayer: LayerState;
  scale: number;
  rafId: number | null;
  currentX: number;
  currentY: number;
  origFontSize?: number;
}

// ── Dependency bag ───────────────────────────────────────────────────────

export interface DragResizeDeps {
  canvasWidth: number;
  canvasHeight: number;
  dragStateRef: React.MutableRefObject<DragState | null>;
  layerRefs: React.MutableRefObject<Map<string, HTMLDivElement>>;
  layersRef: React.MutableRefObject<LayerState[]>;
  textOverlaysRef: React.MutableRefObject<TextOverlayState[]>;
  imageOverlaysRef: React.MutableRefObject<ImageOverlayState[]>;
  setLayers: React.Dispatch<React.SetStateAction<LayerState[]>>;
  setTextOverlays: React.Dispatch<React.SetStateAction<TextOverlayState[]>>;
  setImageOverlays: React.Dispatch<React.SetStateAction<ImageOverlayState[]>>;
  setSelectedLayerId: React.Dispatch<React.SetStateAction<string | null>>;
  setIsDragging: React.Dispatch<React.SetStateAction<boolean>>;
  findAnyLayer: (id: string) => { state: LayerState; kind: LayerKind } | null;
  throttledConfigChange: ((layers: LayerState[]) => void) | null;
  commitOverlaysRef: React.MutableRefObject<
    (text: TextOverlayState[], img: ImageOverlayState[]) => void
  >;
  snapGuideRefs: React.MutableRefObject<{
    vertical: HTMLDivElement | null;
    horizontal: HTMLDivElement | null;
    left: HTMLDivElement | null;
    right: HTMLDivElement | null;
    top: HTMLDivElement | null;
    bottom: HTMLDivElement | null;
  }>;
}

// ── Hook ─────────────────────────────────────────────────────────────────

export function useCompositorDragResize(deps: DragResizeDeps) {
  const {
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
    commitOverlaysRef,
    snapGuideRefs,
  } = deps;

  // Track active document listeners so the cleanup effect can remove them on unmount.
  const activeListenersRef = useRef<{
    move: (e: PointerEvent) => void;
    up: (e: PointerEvent) => void;
  } | null>(null);

  const computeLayerFromPointer = useCallback(
    (state: DragState, clientX: number, clientY: number): LayerState => {
      const rawDx = (clientX - state.startX) / state.scale;
      const rawDy = (clientY - state.startY) / state.scale;
      return computeUpdatedLayer(
        state.origLayer,
        state.type,
        state.handle,
        rawDx,
        rawDy,
        canvasWidth,
        canvasHeight,
        state.layerKind
      );
    },
    [canvasWidth, canvasHeight]
  );

  const applyVisualUpdate = useCallback(
    (layer: LayerState, sizeChanged: boolean) => {
      const el = layerRefs.current.get(layer.id);
      if (!el) return;
      el.style.left = `${layer.x}px`;
      el.style.top = `${layer.y}px`;
      if (sizeChanged) {
        el.style.width = `${layer.width}px`;
        el.style.height = `${layer.height}px`;
        // Keep circle clip-path in sync during resize so the crop
        // shape tracks the handle instead of showing a clipped rectangle.
        if (layer.cropShape === 'circle') {
          const r = Math.min(layer.width, layer.height) / 2;
          el.style.clipPath = `circle(${r}px at 50% 50%)`;
        }
      }
    },
    [layerRefs]
  );

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
        const updated = computeLayerFromPointer(s, s.currentX, s.currentY);
        applyVisualUpdate(updated, s.type === 'resize');

        // Live font-size scaling for text overlays during resize so the
        // text visually tracks the handle and there's no snap-back on drop.
        if (
          s.type === 'resize' &&
          s.layerKind === 'text' &&
          s.origFontSize != null &&
          s.origLayer.width > 0
        ) {
          const el = layerRefs.current.get(s.layerId);
          if (el) {
            const newFs = Math.max(
              8,
              Math.round(s.origFontSize * (updated.width / s.origLayer.width))
            );
            const spans = el.querySelectorAll<HTMLSpanElement>('span');
            for (const span of spans) {
              if (span.style.fontSize) span.style.fontSize = `${newFs}px`;
            }
          }
        }

        // Show/hide snap guide lines (ref-only, no React state).
        // Guides are shown for both drag and resize — during resize the
        // relevant edge guides fire when a handle snaps to a canvas boundary.
        const guides = detectSnapGuides(updated, canvasWidth, canvasHeight);
        const refs = snapGuideRefs.current;
        const ON = '0.8';
        if (refs.vertical) refs.vertical.style.opacity = guides.verticalCenter ? ON : '0';
        if (refs.horizontal) refs.horizontal.style.opacity = guides.horizontalCenter ? ON : '0';
        if (refs.left) refs.left.style.opacity = guides.leftEdge ? ON : '0';
        if (refs.right) refs.right.style.opacity = guides.rightEdge ? ON : '0';
        if (refs.top) refs.top.style.opacity = guides.topEdge ? ON : '0';
        if (refs.bottom) refs.bottom.style.opacity = guides.bottomEdge ? ON : '0';
      });
    },
    [
      dragStateRef,
      computeLayerFromPointer,
      applyVisualUpdate,
      canvasWidth,
      canvasHeight,
      snapGuideRefs,
    ]
  );

  const handlePointerUp = useCallback(
    (e: PointerEvent) => {
      const state = dragStateRef.current;
      if (!state) return;

      if (state.rafId !== null) cancelAnimationFrame(state.rafId);

      // Hide all snap guides on drop.
      const refs = snapGuideRefs.current;
      for (const key of ['vertical', 'horizontal', 'left', 'right', 'top', 'bottom'] as const) {
        const el = refs[key];
        if (el) el.style.opacity = '0';
      }

      const updated = computeLayerFromPointer(state, e.clientX, e.clientY);
      setIsDragging(false);

      // Pure click-to-select (zero movement) — don't commit to server.
      // Committing here would send potentially stale config-parsed positions
      // for ALL layers, overwriting the server's resolved layout.
      const dx = e.clientX - state.startX;
      const dy = e.clientY - state.startY;
      const isZeroDelta = dx === 0 && dy === 0;

      if (state.layerKind === 'video') {
        if (!isZeroDelta) {
          setLayers((prev) => prev.map((l) => (l.id === updated.id ? updated : l)));
          const newLayers = layersRef.current.map((l) => (l.id === updated.id ? updated : l));
          throttledConfigChange?.(newLayers);
        }
      } else if (state.layerKind === 'text') {
        if (!isZeroDelta) {
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
        }
      } else if (state.layerKind === 'image') {
        if (!isZeroDelta) {
          setImageOverlays((prev) => {
            const next = prev.map((o) =>
              o.id === updated.id
                ? {
                    ...o,
                    x: updated.x,
                    y: updated.y,
                    width: updated.width,
                    height: updated.height,
                  }
                : o
            );
            commitOverlaysRef.current(textOverlaysRef.current, next);
            return next;
          });
        }
      }

      dragStateRef.current = null;
      document.removeEventListener('pointermove', handlePointerMove);
      document.removeEventListener('pointerup', handlePointerUp);
      activeListenersRef.current = null;
    },
    [
      dragStateRef,
      computeLayerFromPointer,
      setIsDragging,
      setLayers,
      setTextOverlays,
      setImageOverlays,
      layersRef,
      textOverlaysRef,
      imageOverlaysRef,
      throttledConfigChange,
      commitOverlaysRef,
      handlePointerMove,
      snapGuideRefs,
    ]
  );

  const handleLayerPointerDown = useCallback(
    (layerId: string, e: React.PointerEvent) => {
      if (e.button !== 0) return; // only primary (left) button starts drag
      e.stopPropagation();
      e.preventDefault();

      const found = findAnyLayer(layerId);
      if (!found) return;

      setSelectedLayerId(layerId);

      const el = layerRefs.current.get(layerId);
      const container = el?.parentElement;
      const scale = container
        ? container.getBoundingClientRect().width /
          (Number(container.dataset.canvasWidth) || canvasWidth)
        : 1;

      // For text/image overlays the displayed dimensions may differ from
      // the stored state due to auto-sizing.  Use the DOM element's actual
      // dimensions so edge-snap calculations align with what the user sees.
      const origLayer = { ...found.state };
      if (el && (found.kind === 'text' || found.kind === 'image')) {
        origLayer.width = el.offsetWidth;
        origLayer.height = el.offsetHeight;
      }

      dragStateRef.current = {
        type: 'drag',
        layerId,
        layerKind: found.kind,
        startX: e.clientX,
        startY: e.clientY,
        origLayer,
        scale,
        rafId: null,
        currentX: e.clientX,
        currentY: e.clientY,
      };

      setIsDragging(true);
      document.addEventListener('pointermove', handlePointerMove);
      document.addEventListener('pointerup', handlePointerUp);
      activeListenersRef.current = { move: handlePointerMove, up: handlePointerUp };
    },
    [
      canvasWidth,
      findAnyLayer,
      setSelectedLayerId,
      layerRefs,
      dragStateRef,
      setIsDragging,
      handlePointerMove,
      handlePointerUp,
    ]
  );

  const handleResizePointerDown = useCallback(
    (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => {
      if (e.button !== 0) return; // only primary (left) button starts resize
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

      const origFontSize =
        found.kind === 'text'
          ? textOverlaysRef.current.find((o) => o.id === layerId)?.fontSize
          : undefined;

      // For text/image overlays the displayed dimensions may differ from the
      // stored state because of auto-sizing (measuredTextWidth/Height or
      // browser text measurement).  Use the DOM element's actual dimensions
      // so the resize handle stays under the cursor from the first frame.
      const origLayer = { ...found.state };
      if (el && (found.kind === 'text' || found.kind === 'image')) {
        origLayer.width = el.offsetWidth;
        origLayer.height = el.offsetHeight;
      }

      dragStateRef.current = {
        type: 'resize',
        layerId,
        layerKind: found.kind,
        handle,
        startX: e.clientX,
        startY: e.clientY,
        origLayer,
        scale,
        rafId: null,
        currentX: e.clientX,
        currentY: e.clientY,
        origFontSize,
      };

      setIsDragging(true);
      document.addEventListener('pointermove', handlePointerMove);
      document.addEventListener('pointerup', handlePointerUp);
      activeListenersRef.current = { move: handlePointerMove, up: handlePointerUp };
    },
    [
      canvasWidth,
      findAnyLayer,
      layerRefs,
      textOverlaysRef,
      dragStateRef,
      setIsDragging,
      handlePointerMove,
      handlePointerUp,
    ]
  );

  // Clean up document listeners if the component unmounts mid-drag.
  useEffect(() => {
    return () => {
      if (activeListenersRef.current) {
        document.removeEventListener('pointermove', activeListenersRef.current.move);
        document.removeEventListener('pointerup', activeListenersRef.current.up);
      }
      if (dragStateRef.current?.rafId != null) {
        cancelAnimationFrame(dragStateRef.current.rafId);
      }
      dragStateRef.current = null;
      setIsDragging(false);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- mount-only: all deps are stable refs or React setters that never change
  }, []);

  return {
    handleLayerPointerDown,
    handleResizePointerDown,
  };
}
