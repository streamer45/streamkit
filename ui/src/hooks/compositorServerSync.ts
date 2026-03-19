// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Server-driven layout synchronisation for the compositor.
 *
 * When a live pipeline is running (Monitor view), the server is the source of
 * truth for layer positions, dimensions, and overlay measurements.  This module
 * encapsulates the view-data subscription and the diffing logic that keeps the
 * Jotai atom state in sync without unnecessary re-renders.
 *
 * With Jotai atoms, only the specific layer atoms that actually changed get new
 * values — other layers and their subscribed components are unaffected.  The
 * sliderActiveRef guard is no longer needed because atom-level writes during
 * slider drags are immediately overwritten by the next slider tick; any brief
 * echo-back regression is imperceptible.
 */

import { useEffect } from 'react';

import {
  sessionStore as defaultSessionStore,
  nodeViewDataAtom,
  nodeKey,
} from '@/stores/sessionAtoms';
import type {
  CompositorLayout,
  ResolvedLayer,
  ResolvedOverlay,
} from '@/types/generated/compositor-types';

import type { CompositorStore } from './compositorAtoms';
import {
  getImageOverlaysFromStore,
  getLayersFromStore,
  getTextOverlaysFromStore,
  isSliderActiveAtom,
  setImageOverlaysInStore,
  setLayersInStore,
  setTextOverlaysInStore,
} from './compositorAtoms';
import type { LayerState, TextOverlayState, OverlayBase } from './compositorLayerParsers';

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
 *  Subscribes to the per-node viewData Jotai atom in the default
 *  (provider-less) store.  This avoids the compositor Provider's scoped
 *  store and doesn't trigger React re-renders.  Writes go directly to
 *  the compositor Jotai store's per-layer atoms — only atoms whose
 *  values actually changed trigger subscriber re-renders. */
export function useServerLayoutSync(
  sessionId: string | undefined,
  nodeId: string,
  store: CompositorStore,
  dragStateRef: React.MutableRefObject<unknown>
): void {
  useEffect(() => {
    if (!sessionId) return;

    const applyServerLayout = (viewData: unknown) => {
      if (!viewData || typeof viewData !== 'object') return;
      // Skip during drag/resize or active slider interaction to avoid
      // server echo-backs overwriting in-flight local atom values.
      if (dragStateRef.current || store.get(isSliderActiveAtom)) return;

      const layout = viewData as CompositorLayout;
      if (!Array.isArray(layout.layers)) return;

      const prevLayers = getLayersFromStore(store);
      const newLayers = mapServerLayers(prevLayers, layout.layers);
      if (newLayers !== prevLayers) setLayersInStore(store, newLayers);

      if (Array.isArray(layout.text_overlays)) {
        const prevText = getTextOverlaysFromStore(store);
        const base = applyServerOverlays(prevText, layout.text_overlays);
        const next = mergeTextMeasurements(base, layout.text_overlays);
        if (next !== prevText) setTextOverlaysInStore(store, next);
      }

      if (Array.isArray(layout.image_overlays)) {
        const prevImg = getImageOverlaysFromStore(store);
        const next = applyServerOverlays(prevImg, layout.image_overlays);
        if (next !== prevImg) setImageOverlaysInStore(store, next);
      }
    };

    // Apply current value immediately (if any) from the default Jotai store.
    const viewDataAtom = nodeViewDataAtom(nodeKey(sessionId, nodeId));
    const current = defaultSessionStore.get(viewDataAtom);
    applyServerLayout(current);

    // Subscribe to the Jotai atom in the default (provider-less) store.
    // This runs outside the compositor Provider scope, so we use
    // defaultSessionStore.sub() directly instead of useAtomValue.
    const unsubscribe = defaultSessionStore.sub(viewDataAtom, () => {
      applyServerLayout(defaultSessionStore.get(viewDataAtom));
    });
    return unsubscribe;
  }, [sessionId, nodeId, store, dragStateRef]);
}
