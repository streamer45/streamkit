// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Server-driven layout synchronisation for the compositor.
 *
 * When a live pipeline is running (Monitor view), the server is the source of
 * truth for layer geometry (positions/sizes from aspect-fit, auto-PiP, and
 * text measurement).  This module encapsulates the view-data subscription and
 * the diffing logic that keeps the Jotai atom state in sync without
 * unnecessary re-renders.
 *
 * View data carries ONLY server-computed fields (x, y, width, height, and
 * text measurements).  Config-driven fields (opacity, rotation, z_index,
 * mirror, crop) are never in the view-data payload, so there is no risk of
 * stale echo-backs overwriting the client's authoritative local state during
 * high-frequency slider interactions.
 *
 * During pointer drags the `dragStateRef` guard still prevents stale geometry
 * from overwriting in-flight DOM positions.
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
  setImageOverlaysInStore,
  setLayersInStore,
  setTextOverlaysInStore,
} from './compositorAtoms';
import {
  DEFAULT_OPACITY,
  DEFAULT_ROTATION_DEGREES,
  DEFAULT_Z_INDEX,
  DEFAULT_MIRROR_HORIZONTAL,
  DEFAULT_MIRROR_VERTICAL,
  DEFAULT_VISIBLE,
  DEFAULT_CROP_ZOOM,
  DEFAULT_CROP_X,
  DEFAULT_CROP_Y,
  DEFAULT_CROP_SHAPE,
} from './compositorConstants';
import type { LayerState, TextOverlayState, OverlayBase } from './compositorLayerParsers';
import { getLocalConfigRev, getClientNonce } from './useConfigRev';

/** Per-layer source frame dimensions, keyed by layer id.
 *  Populated from `ResolvedLayer.source_width`/`source_height` in server
 *  view data.  Kept separate from `LayerState` so prediction inputs
 *  (runtime server metadata) don't mix with config/geometry state. */
export type SourceDimsMap = Map<string, { width: number; height: number }>;

/** Map server geometry onto existing client LayerState[], preserving all
 *  config-driven fields (opacity, rotation, z_index, mirror, crop, visible).
 *
 *  For server-only layers (e.g. auto-PiP layers with no explicit config in
 *  params), a stub LayerState is created with server geometry + default config
 *  values.  This ensures the client has a LayerState entry for every layer the
 *  server reports, which is a precondition for client-side prediction. */
export function mapServerLayers(prev: LayerState[], serverLayers: ResolvedLayer[]): LayerState[] {
  const next: LayerState[] = serverLayers.map((sl) => {
    const existing = prev.find((l) => l.id === sl.id);
    if (!existing) {
      // Server-only layer (auto-PiP) with no local counterpart.
      // Materialize a stub with server geometry + default config values.
      return {
        id: sl.id,
        x: sl.x,
        y: sl.y,
        width: sl.width,
        height: sl.height,
        opacity: DEFAULT_OPACITY,
        zIndex: DEFAULT_Z_INDEX,
        rotationDegrees: DEFAULT_ROTATION_DEGREES,
        mirrorHorizontal: DEFAULT_MIRROR_HORIZONTAL,
        mirrorVertical: DEFAULT_MIRROR_VERTICAL,
        visible: DEFAULT_VISIBLE,
        cropZoom: DEFAULT_CROP_ZOOM,
        cropX: DEFAULT_CROP_X,
        cropY: DEFAULT_CROP_Y,
        cropShape: DEFAULT_CROP_SHAPE,
        aspectFit: true,
        serverOnly: true,
      } satisfies LayerState;
    }
    if (
      existing.x === sl.x &&
      existing.y === sl.y &&
      existing.width === sl.width &&
      existing.height === sl.height
    ) {
      return existing;
    }
    return { ...existing, x: sl.x, y: sl.y, width: sl.width, height: sl.height };
  });

  return next.length !== prev.length || next.some((s, i) => s !== prev[i]) ? next : prev;
}

/** Apply server-resolved geometry to a single overlay, preserving all
 *  config-driven fields.  Returns the original reference when unchanged. */
function resolveOverlay<T extends OverlayBase>(o: T, so: ResolvedOverlay): T {
  if (o.x === so.x && o.y === so.y && o.width === so.width && o.height === so.height) {
    return o;
  }
  return { ...o, x: so.x, y: so.y, width: so.width, height: so.height };
}

/** Apply server-resolved overlay geometry to local state.
 *  Matches by stable `id` instead of array index.
 *  Performs shallow equality to avoid unnecessary re-renders. */
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

/** True when the view-data tick was rendered from pre-commit config
 *  (rev older than our latest stamped commit).  Empty sender is the
 *  server's pre-stamp default (see compositor `mod.rs` view-data emit:
 *  any node that participates in the rev contract emits `_sender: ""`,
 *  `_rev: 0` until the first stamped UpdateParams lands) and is
 *  treated like "ours" for gating. */
export function isStaleViewData(vd: Record<string, unknown>, nodeId: string): boolean {
  const sender = typeof vd._sender === 'string' ? vd._sender : undefined;
  const rev = typeof vd._rev === 'number' ? vd._rev : undefined;
  if (rev === undefined) return false;
  if (sender === '' || sender === getClientNonce()) {
    return rev < getLocalConfigRev(nodeId);
  }
  return false;
}

/** Populate per-layer source dimensions from server view data.
 *  Only writes when a value actually changed to avoid unnecessary object churn. */
function updateSourceDims(
  sourceDimsRef: React.MutableRefObject<SourceDimsMap>,
  serverLayers: ResolvedLayer[]
): void {
  for (const sl of serverLayers) {
    if (sl.source_width != null && sl.source_height != null) {
      const prev = sourceDimsRef.current.get(sl.id);
      if (!prev || prev.width !== sl.source_width || prev.height !== sl.source_height) {
        sourceDimsRef.current.set(sl.id, {
          width: sl.source_width,
          height: sl.source_height,
        });
      }
    }
  }
}

/** Apply server overlay and layer updates to the compositor store. */
function applyServerLayoutToStore(store: CompositorStore, layout: CompositorLayout): void {
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
}

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
  dragStateRef: React.MutableRefObject<unknown>,
  sourceDimsRef: React.MutableRefObject<SourceDimsMap>,
  activeInteractionRef?: React.MutableRefObject<boolean>
): void {
  useEffect(() => {
    if (!sessionId) return;

    const applyServerLayout = (viewData: unknown) => {
      if (!viewData || typeof viewData !== 'object') return;
      // Skip during pointer drag/resize to avoid stale server geometry
      // overwriting in-flight DOM positions.
      if (dragStateRef.current) return;
      // Skip during any active live-mode interaction (slider drag, etc.)
      // to avoid stale server values overwriting in-flight client state.
      if (activeInteractionRef?.current) return;

      const vd = viewData as Record<string, unknown>;

      // Stale view-data gate: skip echoes of our own config changes
      // whose rev is strictly below our local counter.  We use `<`
      // (not `<=`) because the view data for the current rev carries
      // server-computed geometry that the client must accept.
      if (isStaleViewData(vd, nodeId)) return;

      const layout = viewData as CompositorLayout;
      if (!Array.isArray(layout.layers)) return;

      updateSourceDims(sourceDimsRef, layout.layers);
      applyServerLayoutToStore(store, layout);
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
  }, [sessionId, nodeId, store, dragStateRef, sourceDimsRef, activeInteractionRef]);
}
