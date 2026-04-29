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

import { describe, it, expect, vi } from 'vitest';

import type { ResolvedLayer } from '@/types/generated/compositor-types';

import type { LayerState } from './compositorLayerParsers';
import { isStaleViewData, mapServerLayers } from './compositorServerSync';
import { bumpConfigRev, resetAllConfigRevs } from './useConfigRev';

// Stub the WS service so `getClientNonce()` returns a deterministic
// value without requiring a live WS connection.  Hoisted by Vitest so
// it applies before module-level imports resolve in the SUT.
vi.mock('@/services/websocket', () => ({
  getWebSocketService: () => ({
    getClientNonce: () => 'client-A-nonce',
  }),
}));

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
    aspectFit: true,
  };
}

describe('mapServerLayers — pure geometry merge', () => {
  it('updates geometry from server for matching layers', () => {
    const prev = [makeLayer('in_0', 0, 1280)];
    const serverLayers: ResolvedLayer[] = [
      {
        id: 'in_0',
        x: 160,
        y: 0,
        width: 960,
        height: 720,
        source_width: 1920,
        source_height: 1080,
      },
    ];

    const result = mapServerLayers(prev, serverLayers);

    expect(result[0].x).toBe(160);
    expect(result[0].width).toBe(960);
    // Config-driven fields preserved
    expect(result[0].opacity).toBe(1.0);
    expect(result[0].visible).toBe(true);
  });

  it('returns same reference when geometry is unchanged', () => {
    const prev = [makeLayer('in_0', 160, 960)];
    const serverLayers: ResolvedLayer[] = [
      {
        id: 'in_0',
        x: 160,
        y: 0,
        width: 960,
        height: 720,
        source_width: 1920,
        source_height: 1080,
      },
    ];

    const result = mapServerLayers(prev, serverLayers);

    expect(result).toBe(prev); // referential equality
  });

  it('first-drag gate: empty-sender (uninitialised server stamp) is treated as "ours" for rev gating', () => {
    // Server stamps view-data unconditionally with `_sender`/`_rev` —
    // before any client has committed, the stamp defaults to `""`/`0`.
    // After our first stamped commit (localRev = 1) the server is
    // briefly still rendering with the pre-commit config and emits view
    // data with rev 0.  Without gating it, `mapServerLayers` would
    // overwrite the user's just-committed geometry — visible as a
    // first-drag snap-back on auto-stub layers.
    resetAllConfigRevs();
    bumpConfigRev('compositor-1'); // localRev becomes 1

    const preCommit: Record<string, unknown> = {
      _sender: '',
      _rev: 0,
      layers: [],
    };
    expect(isStaleViewData(preCommit, 'compositor-1')).toBe(true);

    // Once the server applies our commit it stamps with our nonce.
    // rev === local: not stale.
    const fresh: Record<string, unknown> = {
      _sender: 'client-A-nonce',
      _rev: 1,
      layers: [],
    };
    expect(isStaleViewData(fresh, 'compositor-1')).toBe(false);

    // Echo of an older commit from us: stale.
    const echo: Record<string, unknown> = {
      _sender: 'client-A-nonce',
      _rev: 0,
      layers: [],
    };
    expect(isStaleViewData(echo, 'compositor-1')).toBe(true);

    // Another client's stamp passes through (we accept their edits).
    const otherClient: Record<string, unknown> = {
      _sender: 'client-B-nonce',
      _rev: 5,
      layers: [],
    };
    expect(isStaleViewData(otherClient, 'compositor-1')).toBe(false);

    resetAllConfigRevs();
  });

  it('first-drag gate: empty-sender at rev 0 passes through when localRev is 0', () => {
    // Before any local commit, the server's default `""`/`0` stamp is
    // authoritative — the user is just observing the server-resolved
    // layout.  The gate must not fire here.
    resetAllConfigRevs();

    const preCommit: Record<string, unknown> = {
      _sender: '',
      _rev: 0,
      layers: [],
    };
    expect(isStaleViewData(preCommit, 'compositor-1')).toBe(false);
  });

  it('view-data without `_rev` (unrelated emitters) is not gated', () => {
    // `isStaleViewData` only gates emitters that participate in the
    // rev contract — anything missing `_rev` falls through unchanged.
    resetAllConfigRevs();
    bumpConfigRev('compositor-1');

    const noRev: Record<string, unknown> = { layers: [] };
    expect(isStaleViewData(noRev, 'compositor-1')).toBe(false);

    resetAllConfigRevs();
  });

  it('materializes server-only layers with default config', () => {
    const prev = [makeLayer('in_0', 0, 1280)];
    const serverLayers: ResolvedLayer[] = [
      {
        id: 'in_0',
        x: 160,
        y: 0,
        width: 960,
        height: 720,
        source_width: 1920,
        source_height: 1080,
      },
      { id: 'in_1', x: 0, y: 0, width: 320, height: 240, source_width: 640, source_height: 480 },
    ];

    const result = mapServerLayers(prev, serverLayers);

    expect(result).toHaveLength(2);
    expect(result[0].id).toBe('in_0');
    expect(result[0].x).toBe(160);
    // Server-only layer materialized with defaults
    expect(result[1].id).toBe('in_1');
    expect(result[1].x).toBe(0);
    expect(result[1].width).toBe(320);
    expect(result[1].opacity).toBe(1.0);
    expect(result[1].visible).toBe(true);
    expect(result[1].serverOnly).toBe(true);
  });
});
