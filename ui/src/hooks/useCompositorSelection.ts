// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Tiny external store for communicating compositor layer selection
 * from CompositorNode to YamlPane without prop drilling.
 *
 * Uses React 18's useSyncExternalStore for tear-free reads.
 *
 * State is keyed by node label so that multiple compositor nodes
 * (within the same pipeline or across sessions) never overwrite
 * each other's selection.
 */

import { useSyncExternalStore } from 'react';

interface CompositorSelection {
  /** Display label of the compositor node (e.g. "compositor_0") */
  nodeLabel: string | null;
  /** Selected layer/overlay ID within that compositor (e.g. "in_0", "text_0") */
  layerId: string | null;
}

/** Per-node selection state, keyed by node label.
 *  Entries accumulate over the module lifetime and are removed explicitly
 *  via clearCompositorSelection (called from useEffect cleanup).  If a node
 *  unmounts without cleanup (error boundary, HMR) its entry lingers until
 *  the next setCompositorSelection call for the same label overwrites it. */
const selectionMap = new Map<string, string | null>();

/** Derived snapshot exposed to consumers.  Updated whenever the map changes. */
let snapshot: CompositorSelection = { nodeLabel: null, layerId: null };
const listeners = new Set<() => void>();

function notify() {
  for (const fn of listeners) fn();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot() {
  return snapshot;
}

/** Rebuild the public snapshot from the map.
 *  When multiple compositors have non-null layerIds, the one visited last
 *  in Map iteration order wins (insertion order, NOT update-recency).
 *  In practice only one compositor is selected at a time, so order is moot. */
function rebuildSnapshot() {
  let activeLabel: string | null = null;
  let activeLayerId: string | null = null;

  for (const [label, layerId] of selectionMap) {
    if (layerId != null) {
      activeLabel = label;
      activeLayerId = layerId;
    }
  }

  const prev = snapshot;
  if (prev.nodeLabel === activeLabel && prev.layerId === activeLayerId) return;
  snapshot = { nodeLabel: activeLabel, layerId: activeLayerId };
  notify();
}

/** Called by CompositorNode when layer selection changes.
 *  Always pass the node label; pass null layerId to clear the selection for that node. */
export function setCompositorSelection(nodeLabel: string, layerId: string | null) {
  selectionMap.set(nodeLabel, layerId);
  rebuildSnapshot();
}

/** Remove the selection entry for a specific compositor node.
 *  Preferred in useEffect cleanup to avoid accidentally clearing another
 *  compositor's selection. */
export function clearCompositorSelection(nodeLabel: string) {
  selectionMap.delete(nodeLabel);
  rebuildSnapshot();
}

/** Read the current compositor selection (reactive) */
export function useCompositorSelection(): CompositorSelection {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
