// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Tiny external store for communicating compositor layer selection
 * from CompositorNode to YamlPane without prop drilling.
 *
 * Uses React 18's useSyncExternalStore for tear-free reads.
 */

import { useSyncExternalStore } from 'react';

interface CompositorSelection {
  /** Display label of the compositor node (e.g. "compositor_0") */
  nodeLabel: string | null;
  /** Selected layer/overlay ID within that compositor (e.g. "in_0", "text_0") */
  layerId: string | null;
}

let snapshot: CompositorSelection = { nodeLabel: null, layerId: null };
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot() {
  return snapshot;
}

/** Called by CompositorNode when layer selection changes */
export function setCompositorSelection(nodeLabel: string | null, layerId: string | null) {
  // Avoid unnecessary notifications
  if (snapshot.nodeLabel === nodeLabel && snapshot.layerId === layerId) return;
  snapshot = { nodeLabel, layerId };
  for (const fn of listeners) fn();
}

/** Read the current compositor selection (reactive) */
export function useCompositorSelection(): CompositorSelection {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
