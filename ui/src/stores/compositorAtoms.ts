// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Jotai atoms for compositor layer state.
 *
 * Each compositor node instance (keyed by nodeId) owns its own set of
 * layer / text-overlay / image-overlay atoms.  Per-layer appearance atoms
 * (opacity, rotation) are stored separately so that a slider drag only
 * re-renders the slider component and the affected canvas layer — not
 * the entire compositor tree.
 *
 * The atom families are organised as:
 *
 *   compositorLayersAtom(nodeId)            → LayerState[]
 *   compositorTextOverlaysAtom(nodeId)      → TextOverlayState[]
 *   compositorImageOverlaysAtom(nodeId)     → ImageOverlayState[]
 *   compositorSelectedLayerAtom(nodeId)     → string | null
 *   compositorIsDraggingAtom(nodeId)        → boolean
 *
 * Per-layer appearance atoms (keyed by `${nodeId}:${layerId}`):
 *
 *   compositorLayerOpacityAtom(key)         → number
 *   compositorLayerRotationAtom(key)        → number
 */

import { atom } from 'jotai';
import { atomFamily } from 'jotai/utils';

import type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
} from '@/hooks/compositorLayerParsers';

import { jotaiStore } from './jotaiStore';

// ── Per-node atom families ──────────────────────────────────────────────────

/** Video layers for a given compositor node. */
export const compositorLayersAtom = atomFamily((_nodeId: string) => {
  void _nodeId;
  return atom<LayerState[]>([]);
});

/** Text overlays for a given compositor node. */
export const compositorTextOverlaysAtom = atomFamily((_nodeId: string) => {
  void _nodeId;
  return atom<TextOverlayState[]>([]);
});

/** Image overlays for a given compositor node. */
export const compositorImageOverlaysAtom = atomFamily((_nodeId: string) => {
  void _nodeId;
  return atom<ImageOverlayState[]>([]);
});

/** Currently selected layer ID within a compositor node. */
export const compositorSelectedLayerAtom = atomFamily((_nodeId: string) => {
  void _nodeId;
  return atom<string | null>(null);
});

/** Whether a drag/resize is in progress for a compositor node. */
export const compositorIsDraggingAtom = atomFamily((_nodeId: string) => {
  void _nodeId;
  return atom<boolean>(false);
});

// ── Per-layer appearance atoms ──────────────────────────────────────────────
//
// Keyed by `${nodeId}:${layerId}`.  These are the source of truth for
// opacity and rotation during slider drags, so the full layers-array
// atom is NOT updated during high-frequency slider ticks.  Only the
// per-layer atom changes → only OpacityControl / RotationControl and
// the affected VideoLayer re-render.

/** Per-layer opacity (0–1). Default matches DEFAULT_OPACITY = 1. */
export const compositorLayerOpacityAtom = atomFamily((_key: string) => {
  void _key;
  return atom<number>(1);
});

/** Per-layer rotation in degrees. Default matches DEFAULT_ROTATION_DEGREES = 0. */
export const compositorLayerRotationAtom = atomFamily((_key: string) => {
  void _key;
  return atom<number>(0);
});

// ── Per-layer atom key tracking ─────────────────────────────────────────────
//
// We need to track which per-layer atom keys belong to each node so that
// cleanupCompositorAtoms can remove them when the node unmounts.

const activeLayerKeys = new Map<string, Set<string>>();

function trackKey(nodeId: string, key: string): void {
  let keys = activeLayerKeys.get(nodeId);
  if (!keys) {
    keys = new Set();
    activeLayerKeys.set(nodeId, keys);
  }
  keys.add(key);
}

// ── Sync helper ─────────────────────────────────────────────────────────────

/** Synchronise per-layer appearance atoms from a merged layer list.
 *
 *  Call this in sync-from-props and server-sync so that the per-layer atoms
 *  reflect the authoritative data.  Uses the vanilla Jotai store so it can
 *  be called from effects without going through React state. */
export function syncLayerAppearanceAtoms(
  nodeId: string,
  items: ReadonlyArray<{ id: string; opacity: number; rotationDegrees: number }>
): void {
  for (const item of items) {
    const key = `${nodeId}:${item.id}`;
    trackKey(nodeId, key);
    jotaiStore.set(compositorLayerOpacityAtom(key), item.opacity);
    jotaiStore.set(compositorLayerRotationAtom(key), item.rotationDegrees);
  }
}

// ── Cleanup ─────────────────────────────────────────────────────────────────

/** Remove all atoms for a compositor node. Call on unmount. */
export function cleanupCompositorAtoms(nodeId: string): void {
  compositorLayersAtom.remove(nodeId);
  compositorTextOverlaysAtom.remove(nodeId);
  compositorImageOverlaysAtom.remove(nodeId);
  compositorSelectedLayerAtom.remove(nodeId);
  compositorIsDraggingAtom.remove(nodeId);

  // Clean up per-layer appearance atoms
  const keys = activeLayerKeys.get(nodeId);
  if (keys) {
    for (const key of keys) {
      compositorLayerOpacityAtom.remove(key);
      compositorLayerRotationAtom.remove(key);
    }
    activeLayerKeys.delete(nodeId);
  }
}
