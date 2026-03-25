// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Unit tests for mergeOverlayState in compositorLayerParsers.
 *
 * Focuses on the `preserveGeometry` (Monitor view) behaviour: when true,
 * ALL server-resolved OverlayBase fields (position, opacity, rotation, etc.)
 * plus runtime-only fields (measuredTextWidth, measuredTextHeight) must be
 * kept from `current`, with only type-specific config fields (text, fontSize,
 * fontName, color, dataBase64) taken from `parsed`.
 */

import { describe, it, expect } from 'vitest';

import type { LayerState, TextOverlayState, ImageOverlayState } from './compositorLayerParsers';
import {
  mergeOverlayState,
  fitRectPreservingAspect,
  detectSnapGuides,
  computeUpdatedLayer,
} from './compositorLayerParsers';

// ── Helpers ─────────────────────────────────────────────────────────────────

function makeTextOverlay(overrides: Partial<TextOverlayState> = {}): TextOverlayState {
  return {
    id: 'text_0',
    text: 'Hello',
    x: 0,
    y: 0,
    width: 200,
    height: 40,
    color: [255, 255, 255, 255],
    fontSize: 32,
    fontName: 'dejavu-sans',
    opacity: 1,
    rotationDegrees: 0,
    zIndex: 100,
    mirrorHorizontal: false,
    mirrorVertical: false,
    visible: true,
    ...overrides,
  };
}

function makeImageOverlay(overrides: Partial<ImageOverlayState> = {}): ImageOverlayState {
  return {
    id: 'img_0',
    dataBase64: 'abc123',
    x: 0,
    y: 0,
    width: 200,
    height: 200,
    opacity: 1,
    rotationDegrees: 0,
    zIndex: 200,
    mirrorHorizontal: false,
    mirrorVertical: false,
    visible: true,
    ...overrides,
  };
}

function makeLayer(overrides: Partial<LayerState> = {}): LayerState {
  return {
    id: 'in_0',
    x: 0,
    y: 0,
    width: 640,
    height: 480,
    opacity: 1,
    rotationDegrees: 0,
    zIndex: 0,
    mirrorHorizontal: false,
    mirrorVertical: false,
    visible: true,
    cropZoom: 1.0,
    cropX: 0.5,
    cropY: 0.5,
    cropShape: 'rect' as const,
    ...overrides,
  };
}

const textHasExtraChanges = (a: TextOverlayState, b: TextOverlayState) =>
  a.text !== b.text ||
  a.fontSize !== b.fontSize ||
  a.fontName !== b.fontName ||
  a.color.some((v, i) => v !== b.color[i]);

const layerHasExtraChanges = (a: LayerState, b: LayerState) =>
  a.cropZoom !== b.cropZoom ||
  a.cropX !== b.cropX ||
  a.cropY !== b.cropY ||
  a.cropShape !== b.cropShape;

// ── Tests ───────────────────────────────────────────────────────────────────

describe('mergeOverlayState', () => {
  describe('preserveGeometry=false (Design view)', () => {
    it('takes all fields from parsed, preserving only visible from existing', () => {
      const current = [makeTextOverlay({ x: 100, y: 200, visible: false, opacity: 0.5 })];
      const parsed = [makeTextOverlay({ x: 0, y: 0, opacity: 1 })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, false);

      expect(result).not.toBe(current);
      expect(result[0].x).toBe(0); // from parsed
      expect(result[0].y).toBe(0); // from parsed
      expect(result[0].visible).toBe(false); // from existing
      expect(result[0].opacity).toBe(0.5); // from existing (because visible=false)
    });
  });

  describe('preserveGeometry=true (Monitor view)', () => {
    it('preserves ALL OverlayBase fields from existing, not just x/y/w/h', () => {
      const current = [
        makeTextOverlay({
          x: 100,
          y: 200,
          width: 300,
          height: 50,
          opacity: 0.8,
          rotationDegrees: 45,
          zIndex: 150,
          mirrorHorizontal: true,
          mirrorVertical: true,
          visible: true,
        }),
      ];
      // Parsed has different values for all OverlayBase fields
      const parsed = [
        makeTextOverlay({
          x: 0,
          y: 0,
          width: 200,
          height: 40,
          opacity: 1,
          rotationDegrees: 0,
          zIndex: 100,
          mirrorHorizontal: false,
          mirrorVertical: false,
        }),
      ];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true);

      // All OverlayBase fields should come from existing
      expect(result[0].x).toBe(100);
      expect(result[0].y).toBe(200);
      expect(result[0].width).toBe(300);
      expect(result[0].height).toBe(50);
      expect(result[0].opacity).toBe(0.8);
      expect(result[0].rotationDegrees).toBe(45);
      expect(result[0].zIndex).toBe(150);
      expect(result[0].mirrorHorizontal).toBe(true);
      expect(result[0].mirrorVertical).toBe(true);
      expect(result[0].visible).toBe(true);
    });

    it('takes type-specific config fields from parsed', () => {
      const current = [makeTextOverlay({ text: 'Old', fontSize: 24, fontName: 'mono' })];
      const parsed = [makeTextOverlay({ text: 'New', fontSize: 48, fontName: 'dejavu-sans' })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true);

      expect(result[0].text).toBe('New');
      expect(result[0].fontSize).toBe(48);
      expect(result[0].fontName).toBe('dejavu-sans');
    });

    it('preserves runtime-only fields like measuredTextWidth/measuredTextHeight', () => {
      const current = [
        makeTextOverlay({
          x: 100,
          y: 200,
          width: 300,
          height: 50,
          measuredTextWidth: 280,
          measuredTextHeight: 45,
        }),
      ];
      // Parsed never has measuredTextWidth/measuredTextHeight
      const parsed = [makeTextOverlay({ text: 'Updated' })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true);

      // Runtime fields should be preserved from existing
      expect(result[0].measuredTextWidth).toBe(280);
      expect(result[0].measuredTextHeight).toBe(45);
    });

    it('returns same reference when nothing changed', () => {
      const overlay = makeTextOverlay({ x: 100, y: 200 });
      const current = [overlay];
      // Parsed has different OverlayBase fields (should be ignored) but same config
      const parsed = [makeTextOverlay({ x: 0, y: 0, text: overlay.text })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true);

      expect(result).toBe(current); // referential equality — no re-render
    });

    it('detects config-only changes via hasExtraChanges', () => {
      const current = [makeTextOverlay({ text: 'Old' })];
      const parsed = [makeTextOverlay({ text: 'New' })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true);

      expect(result).not.toBe(current); // new array due to text change
      expect(result[0].text).toBe('New');
    });

    it('works with image overlays (preserves OverlayBase, takes dataBase64)', () => {
      const current = [
        makeImageOverlay({
          x: 50,
          y: 50,
          width: 400,
          height: 300,
          rotationDegrees: 90,
          dataBase64: 'old-data',
        }),
      ];
      const parsed = [
        makeImageOverlay({
          x: 0,
          y: 0,
          width: 200,
          height: 200,
          rotationDegrees: 0,
          dataBase64: 'new-data',
        }),
      ];

      const imageHasExtraChanges = (a: ImageOverlayState, b: ImageOverlayState) =>
        a.dataBase64 !== b.dataBase64;

      const result = mergeOverlayState(current, parsed, imageHasExtraChanges, true);

      // OverlayBase from existing
      expect(result[0].x).toBe(50);
      expect(result[0].y).toBe(50);
      expect(result[0].width).toBe(400);
      expect(result[0].height).toBe(300);
      expect(result[0].rotationDegrees).toBe(90);
      // Config field from parsed
      expect(result[0].dataBase64).toBe('new-data');
    });

    it('handles new overlays (not in current) by using parsed values', () => {
      const current: TextOverlayState[] = [];
      const parsed = [makeTextOverlay({ id: 'text_new', text: 'Brand new' })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true);

      expect(result.length).toBe(1);
      expect(result[0].id).toBe('text_new');
      expect(result[0].text).toBe('Brand new');
      // Falls through to parsed since no existing match
      expect(result[0].x).toBe(0);
    });

    it('handles removed overlays (in current but not in parsed)', () => {
      const current = [
        makeTextOverlay({ id: 'text_0' }),
        makeTextOverlay({ id: 'text_1', text: 'Second' }),
      ];
      const parsed = [makeTextOverlay({ id: 'text_0' })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true);

      expect(result.length).toBe(1);
      expect(result[0].id).toBe('text_0');
    });
  });

  describe('previousParsed — stale topology rebuild protection', () => {
    // Simulates the bug: user edits a field via inspector, then a topology
    // rebuild fires sync-from-props with the same stale params.  Without
    // previousParsed, the stale value overwrites the user's local edit.

    it('does NOT overwrite text color when parsed params are unchanged', () => {
      // User changed color alpha to 128 via inspector → local state updated
      const current = [makeTextOverlay({ color: [255, 255, 255, 128] })];
      // Topology rebuild re-parses same stale params (alpha=255)
      const parsed = [makeTextOverlay({ color: [255, 255, 255, 255] })];
      // Previous parse had the same stale value
      const previousParsed = [makeTextOverlay({ color: [255, 255, 255, 255] })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true, previousParsed);

      // Color should be preserved from local state (not overwritten)
      expect(result[0].color).toEqual([255, 255, 255, 128]);
      // Should return same reference (no unnecessary re-render)
      expect(result).toBe(current);
    });

    it('does NOT overwrite text fontSize when parsed params are unchanged', () => {
      const current = [makeTextOverlay({ fontSize: 48 })];
      const parsed = [makeTextOverlay({ fontSize: 32 })];
      const previousParsed = [makeTextOverlay({ fontSize: 32 })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true, previousParsed);

      expect(result[0].fontSize).toBe(48);
      expect(result).toBe(current);
    });

    it('does NOT overwrite video layer cropZoom when parsed params are unchanged', () => {
      // User zoomed to 2.5× and panned via inspector
      const current = [makeLayer({ cropZoom: 2.5, cropX: 0.3, cropY: 0.7 })];
      // Topology rebuild re-parses same stale params (default zoom)
      const parsed = [makeLayer({ cropZoom: 1.0, cropX: 0.5, cropY: 0.5 })];
      const previousParsed = [makeLayer({ cropZoom: 1.0, cropX: 0.5, cropY: 0.5 })];

      const result = mergeOverlayState(current, parsed, layerHasExtraChanges, true, previousParsed);

      expect(result[0].cropZoom).toBe(2.5);
      expect(result[0].cropX).toBe(0.3);
      expect(result[0].cropY).toBe(0.7);
      expect(result).toBe(current);
    });

    it('does NOT overwrite video layer cropShape when parsed params are unchanged', () => {
      const current = [makeLayer({ cropShape: 'circle' })];
      const parsed = [makeLayer({ cropShape: 'rect' })];
      const previousParsed = [makeLayer({ cropShape: 'rect' })];

      const result = mergeOverlayState(current, parsed, layerHasExtraChanges, true, previousParsed);

      expect(result[0].cropShape).toBe('circle');
      expect(result).toBe(current);
    });

    it('DOES apply config fields when parsed params actually changed', () => {
      // User had local color alpha=128
      const current = [makeTextOverlay({ color: [255, 255, 255, 128] })];
      // New params have a genuinely different color (e.g. from another client)
      const parsed = [makeTextOverlay({ color: [255, 0, 0, 255] })];
      // Previous parse had the old default
      const previousParsed = [makeTextOverlay({ color: [255, 255, 255, 255] })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true, previousParsed);

      // Color should be updated because parsed actually changed vs previous
      expect(result[0].color).toEqual([255, 0, 0, 255]);
    });

    it('DOES apply config fields when previousParsed is empty (first sync)', () => {
      const current = [makeTextOverlay({ text: 'Local edit' })];
      const parsed = [makeTextOverlay({ text: 'From server' })];
      const previousParsed: TextOverlayState[] = [];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true, previousParsed);

      // No previous match → falls back to pickConfigFields (apply all)
      expect(result[0].text).toBe('From server');
    });

    it('preserves OverlayBase fields even when config fields change', () => {
      const current = [
        makeTextOverlay({
          x: 100,
          y: 200,
          opacity: 0.5,
          rotationDegrees: 45,
        }),
      ];
      const parsed = [
        makeTextOverlay({
          x: 0,
          y: 0,
          opacity: 1,
          rotationDegrees: 0,
          text: 'Updated text',
        }),
      ];
      const previousParsed = [makeTextOverlay({ text: 'Old text' })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true, previousParsed);

      // OverlayBase from existing
      expect(result[0].x).toBe(100);
      expect(result[0].y).toBe(200);
      expect(result[0].opacity).toBe(0.5);
      expect(result[0].rotationDegrees).toBe(45);
      // Config field applied because it actually changed
      expect(result[0].text).toBe('Updated text');
    });

    it('handles mix of changed and unchanged config fields', () => {
      const current = [makeTextOverlay({ text: 'Local', fontSize: 48, fontName: 'mono' })];
      // Only text changed in parsed, fontSize and fontName stayed the same
      const parsed = [makeTextOverlay({ text: 'New', fontSize: 32, fontName: 'dejavu-sans' })];
      const previousParsed = [
        makeTextOverlay({ text: 'Old', fontSize: 32, fontName: 'dejavu-sans' }),
      ];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true, previousParsed);

      // text changed in parsed → applied
      expect(result[0].text).toBe('New');
      // fontSize unchanged in parsed → local value preserved
      expect(result[0].fontSize).toBe(48);
      // fontName unchanged in parsed → local value preserved
      expect(result[0].fontName).toBe('mono');
    });

    it('does NOT overwrite image dataBase64 when parsed params are unchanged', () => {
      const current = [makeImageOverlay({ dataBase64: 'local-edit' })];
      const parsed = [makeImageOverlay({ dataBase64: 'stale-server' })];
      const previousParsed = [makeImageOverlay({ dataBase64: 'stale-server' })];

      const imageHasExtraChanges = (a: ImageOverlayState, b: ImageOverlayState) =>
        a.dataBase64 !== b.dataBase64;

      const result = mergeOverlayState(current, parsed, imageHasExtraChanges, true, previousParsed);

      expect(result[0].dataBase64).toBe('local-edit');
      expect(result).toBe(current);
    });

    it('is not used when preserveGeometry=false (Design view)', () => {
      // In Design view, previousParsed is ignored — parsed always wins
      const current = [makeTextOverlay({ color: [255, 255, 255, 128] })];
      const parsed = [makeTextOverlay({ color: [255, 255, 255, 255] })];
      const previousParsed = [makeTextOverlay({ color: [255, 255, 255, 255] })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, false, previousParsed);

      // Design view takes all fields from parsed
      expect(result[0].color).toEqual([255, 255, 255, 255]);
    });
  });
});

// ── fitRectPreservingAspect ─────────────────────────────────────────────────
// Test vectors mirror Rust tests in compositor/tests.rs:test_fit_rect_preserving_aspect

describe('fitRectPreservingAspect', () => {
  it('4:3 source into 16:9 bounds → pillarboxed', () => {
    // Scale = min(426/640, 240/480) = min(0.666, 0.5) = 0.5
    // Fitted: 320×240, centred within 426×240
    const result = fitRectPreservingAspect(640, 480, { x: 100, y: 50, width: 426, height: 240 });
    expect(result.width).toBe(320);
    expect(result.height).toBe(240);
    expect(result.x).toBe(100 + Math.floor((426 - 320) / 2));
    expect(result.y).toBe(50);
  });

  it('16:9 source into 4:3 bounds → letterboxed', () => {
    // Scale = min(400/1280, 400/720) = min(0.3125, 0.555) = 0.3125
    // Fitted: 400×225, centred within 400×400
    const result = fitRectPreservingAspect(1280, 720, { x: 0, y: 0, width: 400, height: 400 });
    expect(result.width).toBe(400);
    expect(result.height).toBe(225);
    expect(result.x).toBe(0);
    expect(result.y).toBe(Math.floor((400 - 225) / 2));
  });

  it('exact match → no change', () => {
    const result = fitRectPreservingAspect(640, 480, { x: 10, y: 20, width: 640, height: 480 });
    expect(result.width).toBe(640);
    expect(result.height).toBe(480);
    expect(result.x).toBe(10);
    expect(result.y).toBe(20);
  });

  it('zero source width → returns bounds unchanged', () => {
    const bounds = { x: 5, y: 10, width: 200, height: 100 };
    expect(fitRectPreservingAspect(0, 480, bounds)).toEqual(bounds);
  });

  it('zero source height → returns bounds unchanged', () => {
    const bounds = { x: 5, y: 10, width: 200, height: 100 };
    expect(fitRectPreservingAspect(640, 0, bounds)).toEqual(bounds);
  });

  it('zero bounds width → returns bounds unchanged', () => {
    const bounds = { x: 5, y: 10, width: 0, height: 100 };
    expect(fitRectPreservingAspect(640, 480, bounds)).toEqual(bounds);
  });

  it('zero bounds height → returns bounds unchanged', () => {
    const bounds = { x: 5, y: 10, width: 200, height: 0 };
    expect(fitRectPreservingAspect(640, 480, bounds)).toEqual(bounds);
  });
});

// ── detectSnapGuides — edge-snap detection ──────────────────────────────────

describe('detectSnapGuides', () => {
  const CW = 1920;
  const CH = 1080;

  it('detects vertical-centre snap when layer midpoint is at canvas midpoint', () => {
    const layer = makeLayer({ x: (CW - 640) / 2, y: 100, width: 640, height: 480 });
    const guides = detectSnapGuides(layer, CW, CH);
    expect(guides.verticalCenter).toBe(true);
    expect(guides.horizontalCenter).toBe(false);
  });

  it('detects horizontal-centre snap when layer midpoint is at canvas midpoint', () => {
    const layer = makeLayer({ x: 100, y: (CH - 480) / 2, width: 640, height: 480 });
    const guides = detectSnapGuides(layer, CW, CH);
    expect(guides.horizontalCenter).toBe(true);
    expect(guides.verticalCenter).toBe(false);
  });

  it('detects left-edge snap when layer x ≈ 0', () => {
    const layer = makeLayer({ x: 0, y: 100 });
    const guides = detectSnapGuides(layer, CW, CH);
    expect(guides.leftEdge).toBe(true);
    expect(guides.rightEdge).toBe(false);
  });

  it('detects right-edge snap when layer right edge ≈ canvas width', () => {
    const layer = makeLayer({ x: CW - 640, y: 100, width: 640 });
    const guides = detectSnapGuides(layer, CW, CH);
    expect(guides.rightEdge).toBe(true);
    expect(guides.leftEdge).toBe(false);
  });

  it('detects top-edge snap when layer y ≈ 0', () => {
    const layer = makeLayer({ x: 100, y: 0 });
    const guides = detectSnapGuides(layer, CW, CH);
    expect(guides.topEdge).toBe(true);
    expect(guides.bottomEdge).toBe(false);
  });

  it('detects bottom-edge snap when layer bottom edge ≈ canvas height', () => {
    const layer = makeLayer({ x: 100, y: CH - 480, height: 480 });
    const guides = detectSnapGuides(layer, CW, CH);
    expect(guides.bottomEdge).toBe(true);
    expect(guides.topEdge).toBe(false);
  });

  it('returns all false when layer is in the middle with no alignment', () => {
    const layer = makeLayer({ x: 200, y: 200, width: 100, height: 100 });
    const guides = detectSnapGuides(layer, CW, CH);
    expect(guides.verticalCenter).toBe(false);
    expect(guides.horizontalCenter).toBe(false);
    expect(guides.leftEdge).toBe(false);
    expect(guides.rightEdge).toBe(false);
    expect(guides.topEdge).toBe(false);
    expect(guides.bottomEdge).toBe(false);
  });
});

// ── computeUpdatedLayer — edge-snap in drag ─────────────────────────────────

describe('computeUpdatedLayer edge snapping', () => {
  const CW = 1920;
  const CH = 1080;

  it('snaps layer left edge to x=0 when dragged near canvas left', () => {
    // Layer starts at x=100; drag left by ~97px so it lands within SNAP_THRESHOLD of 0
    const orig = makeLayer({ x: 100, y: 300, width: 640, height: 480 });
    const result = computeUpdatedLayer(orig, 'drag', undefined, -97, 0, CW, CH);
    expect(result.x).toBe(0);
  });

  it('snaps layer right edge to canvas width when dragged near right boundary', () => {
    const orig = makeLayer({ x: 1200, y: 300, width: 640, height: 480 });
    // dx = 78 → raw x = 1278 → grid-snap to 1280 → right edge 1920 = CW → edge snap fires
    const result = computeUpdatedLayer(orig, 'drag', undefined, 78, 0, CW, CH);
    expect(result.x).toBe(CW - orig.width);
  });

  it('snaps layer top edge to y=0 when dragged near canvas top', () => {
    const orig = makeLayer({ x: 300, y: 100, width: 640, height: 480 });
    const result = computeUpdatedLayer(orig, 'drag', undefined, 0, -97, CW, CH);
    expect(result.y).toBe(0);
  });

  it('snaps layer bottom edge to canvas height when dragged near bottom boundary', () => {
    const orig = makeLayer({ x: 300, y: 500, width: 640, height: 480 });
    // dy = 98 → raw y = 598 → grid-snap to 600 → bottom edge 1080 = CH → edge snap fires
    const result = computeUpdatedLayer(orig, 'drag', undefined, 0, 98, CW, CH);
    expect(result.y).toBe(CH - orig.height);
  });

  it('centre snap takes priority over edge snap when both are close', () => {
    // Place the layer so that centre-snap and edge-snap would both fire:
    // If the layer is exactly half the canvas width, x=0 means left-edge AND centre at the same time.
    const w = CW; // layer as wide as canvas
    const orig = makeLayer({ x: 100, y: 300, width: w, height: 480 });
    // Drag so x ≈ 0 (left edge snap) — but also midX = 960 = CW/2 (centre snap)
    // Centre snap should lock x to (CW - w) / 2 = 0
    const result = computeUpdatedLayer(orig, 'drag', undefined, -97, 0, CW, CH);
    expect(result.x).toBe((CW - w) / 2); // 0 — both agree
  });
});

// ── computeUpdatedLayer — edge-snap in resize ───────────────────────────────

describe('computeUpdatedLayer resize edge snapping', () => {
  const CW = 1920;
  const CH = 1080;

  it('snaps right edge to canvas width when resizing east handle near boundary', () => {
    // Layer at x=1200, w=640, h=480 (AR=4:3). Stretch east by 75px → right edge 1915,
    // within SNAP_THRESHOLD of 1920 → snaps to canvas width.
    const orig = makeLayer({ x: 1200, y: 100, width: 640, height: 480 });
    const ar = orig.width / orig.height;
    const result = computeUpdatedLayer(orig, 'resize', 'e', 75, 0, CW, CH);
    expect(result.x + result.width).toBe(CW);
    // Aspect ratio must be preserved after snapping
    expect(result.width / result.height).toBeCloseTo(ar, 5);
  });

  it('snaps left edge to x=0 when resizing west handle near canvas left', () => {
    const orig = makeLayer({ x: 5, y: 100, width: 640, height: 480 });
    const ar = orig.width / orig.height;
    const result = computeUpdatedLayer(orig, 'resize', 'w', -3, 0, CW, CH);
    expect(result.x).toBe(0);
    // Aspect ratio must be preserved after snapping
    expect(result.width / result.height).toBeCloseTo(ar, 5);
  });

  it('snaps bottom edge to canvas height when resizing south handle near boundary', () => {
    const orig = makeLayer({ x: 100, y: 500, width: 640, height: 480 });
    const ar = orig.width / orig.height;
    // bottom = 500 + 480 = 980, stretch south by 96 → 1076, within threshold of 1080
    const result = computeUpdatedLayer(orig, 'resize', 's', 0, 96, CW, CH);
    expect(result.y + result.height).toBe(CH);
    // Aspect ratio must be preserved after snapping
    expect(result.width / result.height).toBeCloseTo(ar, 5);
  });

  it('snaps top edge to y=0 when resizing north handle near canvas top', () => {
    const orig = makeLayer({ x: 100, y: 5, width: 640, height: 480 });
    const ar = orig.width / orig.height;
    const result = computeUpdatedLayer(orig, 'resize', 'n', 0, -3, CW, CH);
    expect(result.y).toBe(0);
    // Aspect ratio must be preserved after snapping
    expect(result.width / result.height).toBeCloseTo(ar, 5);
  });

  it('does NOT snap edges that are far from canvas boundaries', () => {
    const orig = makeLayer({ x: 100, y: 100, width: 200, height: 200 });
    // Resize east by 50px → right edge = 350, far from 1920
    const result = computeUpdatedLayer(orig, 'resize', 'se', 50, 50, CW, CH);
    expect(result.x + result.width).not.toBe(CW);
    expect(result.y + result.height).not.toBe(CH);
  });

  it('corner handle nw near both edges snaps only the closer axis', () => {
    // Layer at x=5, y=5 — both within SNAP_THRESHOLD of 0.
    // The closer axis (equal distance → horizontal wins by order) should snap
    // without the second axis undoing it.
    const orig = makeLayer({ x: 5, y: 5, width: 640, height: 480 });
    const ar = orig.width / orig.height;
    const result = computeUpdatedLayer(orig, 'resize', 'nw', -3, -3, CW, CH);
    // At least one edge should be snapped to 0
    const snappedLeft = result.x === 0;
    const snappedTop = result.y === 0;
    expect(snappedLeft || snappedTop).toBe(true);
    // Aspect ratio must be preserved
    expect(result.width / result.height).toBeCloseTo(ar, 5);
    // The snapped edge must not be pushed past the canvas boundary
    expect(result.x).toBeGreaterThanOrEqual(0);
    expect(result.y).toBeGreaterThanOrEqual(0);
  });
});
