// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Unit tests for `promoteEditedServerOnly` — the per-layer scoping
 * that decides which `serverOnly` auto-stubs flip into "explicit
 * config" mode when the user mutates layers.
 *
 * The contract being protected:
 *   1. A layer the user actually edits gets promoted (serverOnly cleared).
 *   2. Other auto-stubs in the same commit stay serverOnly so the
 *      server keeps aspect-fitting sources the user never touched.
 *   3. Already-explicit layers (no serverOnly flag) pass through
 *      unchanged regardless of identity churn.
 */

import { describe, it, expect } from 'vitest';

import type { LayerState } from './compositorLayerParsers';
import { promoteEditedServerOnly } from './useCompositorLayers';

function makeLayer(id: string, x: number, opts: Partial<LayerState> = {}): LayerState {
  return {
    id,
    x,
    y: 0,
    width: 320,
    height: 240,
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
    ...opts,
  };
}

describe('promoteEditedServerOnly', () => {
  it('promotes only the layer whose identity changed', () => {
    const a = makeLayer('in_0', 0, { serverOnly: true });
    const b = makeLayer('in_1', 320, { serverOnly: true });
    const c = makeLayer('in_2', 640, { serverOnly: true });
    const current = [a, b, c];

    // Caller mutates only `b` via map identity preservation.
    const next = current.map((l) => (l.id === 'in_1' ? { ...l, x: 400 } : l));

    const result = promoteEditedServerOnly(current, next);

    // Edited layer: serverOnly cleared, geometry applied.
    expect(result[1].id).toBe('in_1');
    expect(result[1].x).toBe(400);
    expect(result[1].serverOnly).toBeUndefined();

    // Untouched layers retain serverOnly so the server keeps
    // resolving their geometry.
    expect(result[0].serverOnly).toBe(true);
    expect(result[2].serverOnly).toBe(true);

    // Untouched layer references are preserved.
    expect(result[0]).toBe(a);
    expect(result[2]).toBe(c);
  });

  it('passes through already-explicit layers untouched', () => {
    const a = makeLayer('in_0', 0); // no serverOnly
    const b = makeLayer('in_1', 320, { serverOnly: true });
    const current = [a, b];

    const next = current.map((l) => (l.id === 'in_0' ? { ...l, x: 100 } : l));
    const result = promoteEditedServerOnly(current, next);

    // Layer that was never serverOnly stays as-is, with edits applied.
    expect(result[0].x).toBe(100);
    expect(result[0].serverOnly).toBeUndefined();
    // Server-stub passes through untouched (its identity didn't change).
    expect(result[1]).toBe(b);
    expect(result[1].serverOnly).toBe(true);
  });

  it('promotes a freshly-added serverOnly layer (identity not in current)', () => {
    const a = makeLayer('in_0', 0);
    const current = [a];

    const fresh = makeLayer('in_1', 320, { serverOnly: true });
    const next = [a, fresh];

    const result = promoteEditedServerOnly(current, next);

    // The fresh entry is treated as "user-introduced": serverOnly cleared.
    // (In practice this path isn't hit because materialization writes
    // directly via setLayersInStore, but the rule is consistent: any
    // serverOnly entry whose identity isn't in `current` is promoted.)
    expect(result[1].serverOnly).toBeUndefined();
  });

  it('returns the same number of entries as next', () => {
    const a = makeLayer('in_0', 0, { serverOnly: true });
    const b = makeLayer('in_1', 320, { serverOnly: true });
    const current = [a, b];
    // Caller removes one and edits the other.
    const next = [{ ...a, x: 50 }];

    const result = promoteEditedServerOnly(current, next);

    expect(result).toHaveLength(1);
    expect(result[0].x).toBe(50);
    expect(result[0].serverOnly).toBeUndefined();
  });
});
