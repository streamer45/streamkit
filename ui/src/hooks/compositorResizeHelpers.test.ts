// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Tests for the pure helpers in compositorResizeHelpers (snap math, drag/
// resize transforms, AR enforcement, rotation, boundary clamping).

import { describe, expect, it } from 'vitest';

import type { LayerState, ResizeHandle } from './compositorLayerParsers';
import {
  SNAP_GRID,
  SNAP_THRESHOLD,
  computeUpdatedLayer,
  detectSnapGuides,
} from './compositorResizeHelpers';

const CANVAS_W = 1280;
const CANVAS_H = 720;

function makeLayer(overrides: Partial<LayerState> = {}): LayerState {
  return {
    id: 'in_0',
    x: 100,
    y: 100,
    width: 200,
    height: 150,
    opacity: 1,
    zIndex: 0,
    rotationDegrees: 0,
    mirrorHorizontal: false,
    mirrorVertical: false,
    visible: true,
    cropZoom: 1,
    cropX: 0.5,
    cropY: 0.5,
    cropShape: 'rect',
    aspectFit: true,
    ...overrides,
  };
}

describe('snap constants', () => {
  it('exports a 10px grid and an 8px snap threshold', () => {
    expect(SNAP_GRID).toBe(10);
    expect(SNAP_THRESHOLD).toBe(8);
  });
});

describe('detectSnapGuides', () => {
  it('returns all-false when layer is in the interior', () => {
    const layer = makeLayer({ x: 200, y: 200, width: 200, height: 150 });
    expect(detectSnapGuides(layer, CANVAS_W, CANVAS_H)).toEqual({
      verticalCenter: false,
      horizontalCenter: false,
      leftEdge: false,
      rightEdge: false,
      topEdge: false,
      bottomEdge: false,
    });
  });

  it('flags vertical-centre when the layer midpoint matches canvas centre X', () => {
    const layer = makeLayer({ x: (CANVAS_W - 200) / 2, y: 100, width: 200, height: 150 });
    const guides = detectSnapGuides(layer, CANVAS_W, CANVAS_H);
    expect(guides.verticalCenter).toBe(true);
    expect(guides.horizontalCenter).toBe(false);
  });

  it('flags horizontal-centre when the layer midpoint matches canvas centre Y', () => {
    const layer = makeLayer({ x: 100, y: (CANVAS_H - 150) / 2, width: 200, height: 150 });
    expect(detectSnapGuides(layer, CANVAS_W, CANVAS_H).horizontalCenter).toBe(true);
  });

  it('flags edge alignment when layer touches each canvas edge', () => {
    const left = makeLayer({ x: 0, y: 100 });
    expect(detectSnapGuides(left, CANVAS_W, CANVAS_H).leftEdge).toBe(true);

    const right = makeLayer({ x: CANVAS_W - 200, y: 100, width: 200 });
    expect(detectSnapGuides(right, CANVAS_W, CANVAS_H).rightEdge).toBe(true);

    const top = makeLayer({ x: 100, y: 0 });
    expect(detectSnapGuides(top, CANVAS_W, CANVAS_H).topEdge).toBe(true);

    const bottom = makeLayer({ x: 100, y: CANVAS_H - 150, height: 150 });
    expect(detectSnapGuides(bottom, CANVAS_W, CANVAS_H).bottomEdge).toBe(true);
  });
});

describe('computeUpdatedLayer — drag', () => {
  it('returns the original layer untouched when the pointer did not move', () => {
    const layer = makeLayer({ x: 137, y: 213 });
    const r = computeUpdatedLayer(layer, 'drag', undefined, 0, 0, CANVAS_W, CANVAS_H);
    expect(r).toEqual({ ...layer, x: 137, y: 213 });
  });

  it('snaps the dragged position to the SNAP_GRID', () => {
    const layer = makeLayer({ x: 100, y: 100 });
    const r = computeUpdatedLayer(layer, 'drag', undefined, 23, 47, CANVAS_W, CANVAS_H);
    expect(r.x).toBe(120);
    expect(r.y).toBe(150);
  });

  it.each<[string, Partial<LayerState>, number, number, number, number]>([
    ['left edge', { x: 5, y: 100 }, -1, 0, 0, 100],
    ['right edge', { x: CANVAS_W - 200 - 5, y: 100, width: 200 }, 1, 0, CANVAS_W - 200, 100],
    ['top edge', { x: 100, y: 5 }, 0, -1, 100, 0],
    ['bottom edge', { x: 100, y: CANVAS_H - 150 - 5, height: 150 }, 0, 1, 100, CANVAS_H - 150],
  ])('snaps to canvas %s when within threshold', (_label, init, dx, dy, expX, expY) => {
    const layer = makeLayer(init);
    const r = computeUpdatedLayer(layer, 'drag', undefined, dx, dy, CANVAS_W, CANVAS_H);
    expect(r.x).toBe(expX);
    expect(r.y).toBe(expY);
  });

  it('snaps the centre when the layer midpoint approaches canvas centre', () => {
    const layer = makeLayer({ x: (CANVAS_W - 200) / 2 + 3, y: 100, width: 200, height: 150 });
    const r = computeUpdatedLayer(layer, 'drag', undefined, -1, 0, CANVAS_W, CANVAS_H);
    expect(r.x).toBe((CANVAS_W - 200) / 2);
  });
});

describe('computeUpdatedLayer — resize basics', () => {
  it.each<['e' | 'w' | 's' | 'n', 'width' | 'height', number]>([
    ['e', 'width', 50],
    ['w', 'width', -50],
    ['s', 'height', 50],
    ['n', 'height', -50],
  ])('grows %s handle', (handle, dim, delta) => {
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 150 });
    const r = computeUpdatedLayer(
      layer,
      'resize',
      handle,
      handle === 'e' ? delta : handle === 'w' ? delta : 0,
      handle === 's' ? delta : handle === 'n' ? delta : 0,
      CANVAS_W,
      CANVAS_H,
      'text'
    );
    if (dim === 'width') {
      expect(r.width).toBe(layer.width + Math.abs(delta));
    } else {
      expect(r.height).toBe(layer.height + Math.abs(delta));
    }
  });

  it('enforces a 20px minimum dimension', () => {
    const layer = makeLayer({ x: 100, y: 100, width: 30, height: 30 });
    const r = computeUpdatedLayer(layer, 'resize', 'e', -100, 0, CANVAS_W, CANVAS_H, 'text');
    expect(r.width).toBeGreaterThanOrEqual(20);
  });

  it('moves the origin when shrinking from a west handle', () => {
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 150 });
    const r = computeUpdatedLayer(layer, 'resize', 'w', 50, 0, CANVAS_W, CANVAS_H, 'text');
    expect(r.x).toBe(150);
    expect(r.width).toBe(150);
  });

  it('moves the origin when shrinking from a north handle', () => {
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 150 });
    const r = computeUpdatedLayer(layer, 'resize', 'n', 0, 30, CANVAS_W, CANVAS_H, 'text');
    expect(r.y).toBe(130);
    expect(r.height).toBe(120);
  });
});

describe('computeUpdatedLayer — aspect ratio (video / image layers)', () => {
  it('preserves AR when dragging a corner', () => {
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 100 }); // AR = 2
    const r = computeUpdatedLayer(layer, 'resize', 'se', 100, 0, CANVAS_W, CANVAS_H, 'video');
    expect(r.width / r.height).toBeCloseTo(2, 5);
  });

  it('preserves AR for E/W single-edge handles by adjusting height', () => {
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 100 });
    const r = computeUpdatedLayer(layer, 'resize', 'e', 100, 0, CANVAS_W, CANVAS_H, 'video');
    expect(r.width / r.height).toBeCloseTo(2, 5);
  });

  it('preserves AR for N/S single-edge handles by adjusting width', () => {
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 100 });
    const r = computeUpdatedLayer(layer, 'resize', 's', 0, 50, CANVAS_W, CANVAS_H, 'video');
    expect(r.width / r.height).toBeCloseTo(2, 5);
  });

  it('does NOT enforce AR for text overlays', () => {
    // Text starts as 300×40 (AR 7.5); growing east by 100 must NOT scale
    // height by AR — text height stays free.
    const layer = makeLayer({ x: 100, y: 100, width: 300, height: 40 });
    const r = computeUpdatedLayer(layer, 'resize', 'e', 100, 0, CANVAS_W, CANVAS_H, 'text');
    expect(r.width).toBe(400);
    expect(r.height).toBe(40);
  });
});

describe('computeUpdatedLayer — edge snapping during resize', () => {
  it('snaps the east edge to the right canvas boundary when within threshold', () => {
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 100 }); // AR 2
    const remaining = CANVAS_W - (layer.x + layer.width); // 980
    const r = computeUpdatedLayer(
      layer,
      'resize',
      'e',
      remaining - 1,
      0,
      CANVAS_W,
      CANVAS_H,
      'video'
    );
    expect(r.x + r.width).toBe(CANVAS_W);
  });

  it('snaps the west edge to x=0', () => {
    const layer = makeLayer({ x: 50, y: 100, width: 200, height: 100 });
    const r = computeUpdatedLayer(layer, 'resize', 'w', -47, 0, CANVAS_W, CANVAS_H, 'video');
    expect(r.x).toBe(0);
  });
});

describe('computeUpdatedLayer — clamp to canvas bounds', () => {
  it('does not let the layer extend past the right edge', () => {
    const layer = makeLayer({ x: CANVAS_W - 200, y: 100, width: 200, height: 100 });
    const r = computeUpdatedLayer(layer, 'resize', 'e', 9999, 0, CANVAS_W, CANVAS_H, 'video');
    expect(r.x + r.width).toBeLessThanOrEqual(CANVAS_W + 0.001);
  });

  it('does not let the layer extend past the bottom edge', () => {
    const layer = makeLayer({ x: 100, y: CANVAS_H - 100, width: 200, height: 100 });
    const r = computeUpdatedLayer(layer, 'resize', 's', 0, 9999, CANVAS_W, CANVAS_H, 'video');
    expect(r.y + r.height).toBeLessThanOrEqual(CANVAS_H + 0.001);
  });

  it('does not let a west-handle resize pull past x=0', () => {
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 100 });
    const r = computeUpdatedLayer(layer, 'resize', 'w', -9999, 0, CANVAS_W, CANVAS_H, 'video');
    expect(r.x).toBeGreaterThanOrEqual(0);
  });

  it('does not let a north-handle resize pull past y=0', () => {
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 100 });
    const r = computeUpdatedLayer(layer, 'resize', 'n', 0, -9999, CANVAS_W, CANVAS_H, 'video');
    expect(r.y).toBeGreaterThanOrEqual(0);
  });
});

describe('computeUpdatedLayer — rotated layers', () => {
  it('passes through (no extra deltas) when rotationDegrees=0', () => {
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 100, rotationDegrees: 0 });
    const r = computeUpdatedLayer(layer, 'resize', 'e', 30, 0, CANVAS_W, CANVAS_H, 'text');
    expect(r.width).toBe(230);
  });

  it('east-handle screen-X on a 90°-rotated layer does NOT change width', () => {
    // rotateToLocalCoords with rotationDegrees=90 maps (rawDx=50, rawDy=0)
    // → (dx=0, dy=-50).  East consumes only `dx`, so width and x do not
    // move.  A sign flip in the rotation matrix would change width here.
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 100, rotationDegrees: 90 });
    const r = computeUpdatedLayer(layer, 'resize', 'e', 50, 0, CANVAS_W, CANVAS_H, 'text');
    expect(r.width).toBe(layer.width);
    expect(r.x).toBe(layer.x);
  });

  it('east-handle screen-Y on a 90°-rotated layer DOES grow width', () => {
    // (rawDx=0, rawDy=50) at 90° → (dx=50, dy=0).  Perpendicular screen
    // movement maps onto the local east-axis, so width grows by exactly 50.
    // Pairs with the previous test: together they pin down both that the
    // identity-transform path is gone and that the rotation is correct.
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 100, rotationDegrees: 90 });
    const r = computeUpdatedLayer(layer, 'resize', 'e', 0, 50, CANVAS_W, CANVAS_H, 'text');
    expect(r.width).toBe(layer.width + 50);
    expect(r.height).toBe(layer.height);
  });
});

describe('computeUpdatedLayer — degenerate sizes', () => {
  it('skips AR enforcement when original layer has zero width or height', () => {
    const layer = makeLayer({ x: 100, y: 100, width: 0, height: 100 });
    const r = computeUpdatedLayer(layer, 'resize', 'e', 200, 0, CANVAS_W, CANVAS_H, 'video');
    // Width grows but height is unchanged (AR enforcement skipped).
    expect(r.width).toBeGreaterThanOrEqual(20);
    expect(r.height).toBe(100);
  });

  it('handles negative raw deltas without producing negative dimensions', () => {
    const layer = makeLayer({ x: 100, y: 100, width: 200, height: 100 });
    const handles: ResizeHandle[] = ['e', 'w', 'n', 's', 'ne', 'nw', 'se', 'sw'];
    for (const h of handles) {
      const r = computeUpdatedLayer(layer, 'resize', h, -9999, -9999, CANVAS_W, CANVAS_H, 'text');
      expect(r.width).toBeGreaterThanOrEqual(20);
      expect(r.height).toBeGreaterThanOrEqual(20);
    }
  });
});
