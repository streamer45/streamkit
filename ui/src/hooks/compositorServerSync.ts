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
import type { LayerState, TextOverlayState, OverlayBase } from './compositorLayerParsers';
import { getLocalConfigRev, getClientNonce } from './useConfigRev';

// ── Pure helpers ────────────────────────────────────────────────────────────

/** Map server geometry onto existing client LayerState[], preserving all
 *  config-driven fields (opacity, rotation, z_index, mirror, crop, visible). */
export function mapServerLayers(prev: LayerState[], serverLayers: ResolvedLayer[]): LayerState[] {
  const next: LayerState[] = serverLayers
    .map((sl) => {
      const existing = prev.find((l) => l.id === sl.id);
      if (!existing) {
        // New layer from server with no local counterpart — should be rare.
        // Return a stub; sync-from-props will fill in the config fields.
        return undefined;
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
    })
    .filter((l): l is LayerState => l !== undefined);

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
  dragStateRef: React.MutableRefObject<unknown>,
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

      // Stale view-data gate: if this view data originated from our own
      // config change and the rev is <= our local counter, skip it.
      const sender = typeof vd._sender === 'string' ? vd._sender : undefined;
      const rev = typeof vd._rev === 'number' ? vd._rev : undefined;
      if (sender && sender === getClientNonce() && rev !== undefined) {
        const localRev = getLocalConfigRev(nodeId);
        if (rev <= localRev) {
          return;
        }
      }

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
  }, [sessionId, nodeId, store, dragStateRef, activeInteractionRef]);
}
