// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useMemo, useState } from 'react';

import type { TextOverlayState, ImageOverlayState } from '@/hooks/useCompositorLayers';

import { friendlyLabel } from './compositorNodeParts';
import type { CompositorEntry } from './compositorNodeParts';

function entriesEqual(a: CompositorEntry[], b: CompositorEntry[]): boolean {
  return (
    a.length === b.length &&
    a.every(
      (p, i) =>
        p.id === b[i].id &&
        p.kind === b[i].kind &&
        p.zIndex === b[i].zIndex &&
        p.visible === b[i].visible
    )
  );
}

// Returns a referentially stable entry list so React.memo consumers bail out
// during opacity/rotation drags (those fields are not in entries).
export function useStableEntries(
  layers: { id: string; zIndex: number; visible: boolean }[],
  textOverlays: TextOverlayState[],
  imageOverlays: ImageOverlayState[]
): CompositorEntry[] {
  const next = useMemo(() => {
    const all: CompositorEntry[] = [];
    for (const l of layers) {
      all.push({
        id: l.id,
        kind: 'video',
        label: friendlyLabel(l.id, 'video'),
        zIndex: l.zIndex,
        visible: l.visible,
      });
    }
    textOverlays.forEach((o, i) => {
      all.push({
        id: o.id,
        kind: 'text',
        label: friendlyLabel(o.id, 'text', i),
        zIndex: o.zIndex,
        visible: o.visible,
      });
    });
    imageOverlays.forEach((o, i) => {
      all.push({
        id: o.id,
        kind: 'image',
        label: friendlyLabel(o.id, 'image', i),
        zIndex: o.zIndex,
        visible: o.visible,
      });
    });
    all.sort((a, b) => b.zIndex - a.zIndex);
    return all;
  }, [layers, textOverlays, imageOverlays]);

  // Render-time state adjustment (instead of a ref cache) so the React
  // Compiler can optimize this hook while keeping referential stability.
  const [stable, setStable] = useState(next);
  if (!entriesEqual(stable, next)) {
    setStable(next);
    return next;
  }
  return stable;
}
