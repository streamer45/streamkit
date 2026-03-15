// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Server-driven layout synchronisation for the compositor.
 *
 * When a live pipeline is running (Monitor view), the server is the source of
 * truth for layer positions, dimensions, and overlay measurements.  This module
 * encapsulates the view-data subscription and the diffing logic that keeps the
 * React state in sync without unnecessary re-renders.
 */

import { useEffect } from 'react';

import { useSessionStore, selectNodeViewData } from '@/stores/sessionStore';
import type {
  CompositorLayout,
  ResolvedLayer,
  ResolvedOverlay,
} from '@/types/generated/compositor-types';

import type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
  OverlayBase,
} from './compositorLayerParsers';

// ── Pure helpers ────────────────────────────────────────────────────────────

/** Map server layer data to client LayerState[], preserving client-only fields. */
export function mapServerLayers(prev: LayerState[], serverLayers: ResolvedLayer[]): LayerState[] {
  const next: LayerState[] = serverLayers.map((sl) => {
    const existing = prev.find((l) => l.id === sl.id);
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
      mirrorHorizontal: sl.mirror_horizontal,
      mirrorVertical: sl.mirror_vertical,
      visible: existing?.visible ?? true,
      cropZoom: sl.crop_zoom,
      cropX: sl.crop_x,
      cropY: sl.crop_y,
    };
  });
  const changed =
    next.length !== prev.length ||
    next.some(
      (s, i) =>
        s.id !== prev[i].id ||
        s.x !== prev[i].x ||
        s.y !== prev[i].y ||
        s.width !== prev[i].width ||
        s.height !== prev[i].height ||
        s.opacity !== prev[i].opacity ||
        s.zIndex !== prev[i].zIndex ||
        s.rotationDegrees !== prev[i].rotationDegrees ||
        s.mirrorHorizontal !== prev[i].mirrorHorizontal ||
        s.mirrorVertical !== prev[i].mirrorVertical ||
        s.visible !== prev[i].visible ||
        s.cropZoom !== prev[i].cropZoom ||
        s.cropX !== prev[i].cropX ||
        s.cropY !== prev[i].cropY
    );
  return changed ? next : prev;
}

/** Resolve a single overlay against its server counterpart.
 *  Returns the original object when nothing changed (referential equality). */
function resolveOverlay<T extends OverlayBase>(o: T, so: ResolvedOverlay): T {
  const opacity = !o.visible ? o.opacity : so.opacity;
  const mh = so.mirror_horizontal;
  const mv = so.mirror_vertical;

  if (
    o.x === so.x &&
    o.y === so.y &&
    o.width === so.width &&
    o.height === so.height &&
    o.opacity === opacity &&
    o.zIndex === so.z_index &&
    o.rotationDegrees === so.rotation_degrees &&
    o.mirrorHorizontal === mh &&
    o.mirrorVertical === mv
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
    mirrorHorizontal: mh,
    mirrorVertical: mv,
  };
}

/** Apply server-resolved overlay positions to local state.
 *  Matches by stable `id` instead of array index.
 *  Preserves original opacity for hidden overlays and performs
 *  shallow equality to avoid unnecessary re-renders. */
export function applyServerOverlays<T extends OverlayBase>(
  prev: T[],
  serverItems: ResolvedOverlay[]
): T[] {
  const next = prev.map((o) => {
    const so = serverItems.find((s) => s.id === o.id);
    if (!so) return o;
    return resolveOverlay(o, so);
  });
  return next.some((n, i) => n !== prev[i]) ? next : prev;
}

/** Merge server text overlay measurements into local state. */
export function mergeTextMeasurements(
  base: TextOverlayState[],
  serverTextOverlays: ResolvedOverlay[]
): TextOverlayState[] {
  let changed = false;
  const next = base.map((o) => {
    const so = serverTextOverlays.find((s) => s.id === o.id);
    if (!so) return o;
    const mtw = so.measured_text_width ?? undefined;
    const mth = so.measured_text_height ?? undefined;
    if (o.measuredTextWidth === mtw && o.measuredTextHeight === mth) return o;
    changed = true;
    return { ...o, measuredTextWidth: mtw, measuredTextHeight: mth };
  });
  return changed ? next : base;
}

// ── Hook ────────────────────────────────────────────────────────────────────

/** Subscribe to server-driven layout updates for a compositor node.
 *
 *  Uses an external Zustand subscription (useSessionStore.subscribe)
 *  instead of a store selector hook to avoid triggering a React re-render
 *  on every view-data arrival.  The setters below already perform shallow
 *  comparison and only produce a new state reference when something
 *  actually changed, so the component only re-renders when needed. */
export function useServerLayoutSync(
  sessionId: string | undefined,
  nodeId: string,
  dragStateRef: React.MutableRefObject<unknown>,
  setLayers: React.Dispatch<React.SetStateAction<LayerState[]>>,
  setTextOverlays: React.Dispatch<React.SetStateAction<TextOverlayState[]>>,
  setImageOverlays: React.Dispatch<React.SetStateAction<ImageOverlayState[]>>,
  /** When provided, set to `true` once server layout data has been applied.
   *  Callers can use this to gate config-parsed geometry from the "sync from
   *  props" effect so that `useServerLayoutSync` becomes the exclusive
   *  geometry source in Monitor view. */
  serverLayoutAppliedRef?: React.MutableRefObject<boolean>
): void {
  useEffect(() => {
    if (!sessionId) return;

    const applyServerLayout = (viewData: unknown) => {
      if (!viewData || typeof viewData !== 'object') return;
      if (dragStateRef.current) return;

      const layout = viewData as CompositorLayout;
      if (!Array.isArray(layout.layers)) return;

      if (serverLayoutAppliedRef) {
        serverLayoutAppliedRef.current = true;
      }

      setLayers((prev) => mapServerLayers(prev, layout.layers));

      if (Array.isArray(layout.text_overlays)) {
        setTextOverlays((prev) => {
          const base = applyServerOverlays(prev, layout.text_overlays);
          return mergeTextMeasurements(base, layout.text_overlays);
        });
      }

      if (Array.isArray(layout.image_overlays)) {
        setImageOverlays((prev) => applyServerOverlays(prev, layout.image_overlays));
      }
    };

    // Apply current value immediately (if any)
    const current = selectNodeViewData(sessionId, nodeId)(useSessionStore.getState());
    applyServerLayout(current);

    // Subscribe externally — does NOT cause React re-renders.
    const selector = selectNodeViewData(sessionId, nodeId);
    const unsubscribe = useSessionStore.subscribe((state, prevState) => {
      const viewData = selector(state);
      const prevViewData = selector(prevState);
      if (viewData !== prevViewData) {
        applyServerLayout(viewData);
      }
    });
    return unsubscribe;
  }, [sessionId, nodeId, dragStateRef, setLayers, setTextOverlays, setImageOverlays]);
}
