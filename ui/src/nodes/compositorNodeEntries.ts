// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useMemo, useRef } from 'react';

import type { TextOverlayState, ImageOverlayState } from '@/hooks/useCompositorLayers';

import { friendlyLabel } from './compositorNodeParts';
import type { CompositorEntry } from './compositorNodeParts';

export function useStableEntries(
  layers: { id: string; zIndex: number; visible: boolean }[],
  textOverlays: TextOverlayState[],
  imageOverlays: ImageOverlayState[]
): CompositorEntry[] {
  const prevRef = useRef<CompositorEntry[]>([]);
  return useMemo(() => {
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

    const prev = prevRef.current;
    if (
      prev.length === all.length &&
      prev.every(
        (p, i) =>
          p.id === all[i].id &&
          p.kind === all[i].kind &&
          p.zIndex === all[i].zIndex &&
          p.visible === all[i].visible
      )
    ) {
      return prev;
    }
    prevRef.current = all;
    return all;
  }, [layers, textOverlays, imageOverlays]);
}
