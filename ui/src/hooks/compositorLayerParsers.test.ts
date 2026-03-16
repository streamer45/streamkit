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

import type { TextOverlayState, ImageOverlayState } from './compositorLayerParsers';
import { mergeOverlayState } from './compositorLayerParsers';

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

const textHasExtraChanges = (a: TextOverlayState, b: TextOverlayState) =>
  a.text !== b.text ||
  a.fontSize !== b.fontSize ||
  a.fontName !== b.fontName ||
  a.color.some((v, i) => v !== b.color[i]);

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

    it('skips new overlays from parsed that are not in current (prevents stale echo re-add)', () => {
      const current: TextOverlayState[] = [];
      const parsed = [makeTextOverlay({ id: 'text_new', text: 'Brand new' })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true);

      // New items from parsed are skipped — additions go through local
      // state (addTextOverlay) and useState initialiser on topology rebuild.
      expect(result.length).toBe(0);
    });

    it('keeps locally-added overlays not yet in parsed (pending server confirmation)', () => {
      const current = [
        makeTextOverlay({ id: 'text_0' }),
        makeTextOverlay({ id: 'text_1', text: 'Second' }),
      ];
      const parsed = [makeTextOverlay({ id: 'text_0' })];

      const result = mergeOverlayState(current, parsed, textHasExtraChanges, true);

      // text_0 is in both → merged.  text_1 is only in current → kept.
      expect(result.length).toBe(2);
      expect(result[0].id).toBe('text_0');
      expect(result[1].id).toBe('text_1');
    });
  });
});
