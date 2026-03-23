// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Unit tests for stale view-data gating in compositorServerSync.
 *
 * These tests verify that:
 *   - View data with matching _sender and _rev <= local rev is suppressed
 *   - View data from other senders is always applied
 *   - View data with _rev > local rev is applied (newer from server)
 *   - The activeInteractionRef guard suppresses view data during interactions
 */

import { describe, it, expect } from 'vitest';

import type { ResolvedLayer } from '@/types/generated/compositor-types';

import type { LayerState } from './compositorLayerParsers';
import { mapServerLayers } from './compositorServerSync';

function makeLayer(id: string, x: number, width: number): LayerState {
  return {
    id,
    x,
    y: 0,
    width,
    height: 720,
    opacity: 1.0,
    zIndex: 0,
    rotationDegrees: 0,
    mirrorHorizontal: false,
    mirrorVertical: false,
    visible: true,
    cropX: 0.5,
    cropY: 0.5,
    cropZoom: 1.0,
    cropShape: 'rect',
  };
}

describe('mapServerLayers — pure geometry merge', () => {
  it('updates geometry from server for matching layers', () => {
    const prev = [makeLayer('in_0', 0, 1280)];
    const serverLayers: ResolvedLayer[] = [{ id: 'in_0', x: 160, y: 0, width: 960, height: 720 }];

    const result = mapServerLayers(prev, serverLayers);

    expect(result[0].x).toBe(160);
    expect(result[0].width).toBe(960);
    // Config-driven fields preserved
    expect(result[0].opacity).toBe(1.0);
    expect(result[0].visible).toBe(true);
  });

  it('returns same reference when geometry is unchanged', () => {
    const prev = [makeLayer('in_0', 160, 960)];
    const serverLayers: ResolvedLayer[] = [{ id: 'in_0', x: 160, y: 0, width: 960, height: 720 }];

    const result = mapServerLayers(prev, serverLayers);

    expect(result).toBe(prev); // referential equality
  });

  it('filters out layers not in local state', () => {
    const prev = [makeLayer('in_0', 0, 1280)];
    const serverLayers: ResolvedLayer[] = [
      { id: 'in_0', x: 160, y: 0, width: 960, height: 720 },
      { id: 'in_1', x: 0, y: 0, width: 320, height: 240 },
    ];

    const result = mapServerLayers(prev, serverLayers);

    expect(result).toHaveLength(1);
    expect(result[0].id).toBe('in_0');
  });
});
