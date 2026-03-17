// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Jotai atoms for compositor layer state.
 *
 * Each compositor node instance (keyed by nodeId) owns its own set of
 * layer / text-overlay / image-overlay atoms.  Individual layer properties
 * are stored per-layer so that an opacity slider drag only re-renders the
 * slider component — not the entire compositor tree.
 *
 * The atom families are organised as:
 *
 *   compositorLayersAtom(nodeId)       → LayerState[]
 *   compositorTextOverlaysAtom(nodeId) → TextOverlayState[]
 *   compositorImageOverlaysAtom(nodeId)→ ImageOverlayState[]
 *   compositorSelectedLayerAtom(nodeId)→ string | null
 *   compositorIsDraggingAtom(nodeId)   → boolean
 *
 * Derived atoms for individual fields can be added later; for Phase 1 we
 * keep per-node granularity which already eliminates the zero-render DOM
 * hack and the custom memo comparators.
 */

import { atom } from 'jotai';
import { atomFamily } from 'jotai/utils';

import type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
} from '@/hooks/compositorLayerParsers';

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

// ── Cleanup ─────────────────────────────────────────────────────────────────

/** Remove all atoms for a compositor node. Call on unmount. */
export function cleanupCompositorAtoms(nodeId: string): void {
  compositorLayersAtom.remove(nodeId);
  compositorTextOverlaysAtom.remove(nodeId);
  compositorImageOverlaysAtom.remove(nodeId);
  compositorSelectedLayerAtom.remove(nodeId);
  compositorIsDraggingAtom.remove(nodeId);
}
