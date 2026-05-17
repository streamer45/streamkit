// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Unit tests for compositorAtoms — per-compositor Jotai stores, derived
 * selectors, and the bulk setLayers/setTextOverlays/setImageOverlays
 * helpers that diff at field level to avoid spurious atom writes.
 *
 * IDs are unique per test to keep the atomFamily caches isolated (the
 * family caches are global by design — see the note at the bottom of
 * compositorAtoms.ts).
 */

import { createStore } from 'jotai';
import { describe, expect, it } from 'vitest';

import {
  allImageOverlaysAtom,
  allLayersAtom,
  allTextOverlaysAtom,
  getImageOverlaysFromStore,
  getLayersFromStore,
  getTextOverlaysFromStore,
  imageOverlayAtoms,
  imageOverlayIdsAtom,
  isDraggingAtom,
  layerAtoms,
  layerIdsAtom,
  layerOpacityAtom,
  layerRotationAtom,
  nullImageOverlayAtom,
  nullLayerAtom,
  nullOpacityAtom,
  nullRotationAtom,
  nullTextOverlayAtom,
  selectedLayerIdAtom,
  selectedLayerKindAtom,
  setImageOverlaysInStore,
  setLayersInStore,
  setTextOverlaysInStore,
  textOverlayAtoms,
  textOverlayIdsAtom,
} from './compositorAtoms';
import type { ImageOverlayState, LayerState, TextOverlayState } from './compositorLayerParsers';

// ── Test data factories ─────────────────────────────────────────────────────

let idCounter = 0;
function freshId(prefix: string): string {
  idCounter += 1;
  return `${prefix}-${process.pid}-${idCounter}`;
}

function makeLayer(id: string, overrides: Partial<LayerState> = {}): LayerState {
  return {
    id,
    x: 0,
    y: 0,
    width: 640,
    height: 480,
    opacity: 1,
    zIndex: 0,
    rotationDegrees: 0,
    mirrorHorizontal: false,
    mirrorVertical: false,
    visible: true,
    cropZoom: 1.0,
    cropX: 0.5,
    cropY: 0.5,
    cropShape: 'rect',
    aspectFit: true,
    ...overrides,
  };
}

function makeTextOverlay(id: string, overrides: Partial<TextOverlayState> = {}): TextOverlayState {
  return {
    id,
    text: 'hello',
    x: 0,
    y: 0,
    width: 200,
    height: 40,
    color: [255, 255, 255, 255],
    fontSize: 24,
    fontName: 'samples/fonts/system/DejaVuSans.ttf',
    opacity: 1,
    rotationDegrees: 0,
    zIndex: 100,
    mirrorHorizontal: false,
    mirrorVertical: false,
    visible: true,
    ...overrides,
  };
}

function makeImageOverlay(
  id: string,
  overrides: Partial<ImageOverlayState> = {}
): ImageOverlayState {
  return {
    id,
    assetPath: 'samples/images/user/test.png',
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

// ── Core atoms ──────────────────────────────────────────────────────────────

describe('compositorAtoms — initial atom values', () => {
  it('exposes empty ID arrays and null selection by default', () => {
    const store = createStore();
    expect(store.get(layerIdsAtom)).toEqual([]);
    expect(store.get(textOverlayIdsAtom)).toEqual([]);
    expect(store.get(imageOverlayIdsAtom)).toEqual([]);
    expect(store.get(selectedLayerIdAtom)).toBeNull();
    expect(store.get(isDraggingAtom)).toBe(false);
  });

  it('null sentinel atoms have stable default values', () => {
    const store = createStore();
    expect(store.get(nullLayerAtom)).toBeNull();
    expect(store.get(nullTextOverlayAtom)).toBeNull();
    expect(store.get(nullImageOverlayAtom)).toBeNull();
    expect(store.get(nullOpacityAtom)).toBe(1);
    expect(store.get(nullRotationAtom)).toBe(0);
  });

  it('layerOpacityAtom falls back to 1 for unknown ids', () => {
    const store = createStore();
    const id = freshId('layer-missing');
    expect(store.get(layerOpacityAtom(id))).toBe(1);
    expect(store.get(layerRotationAtom(id))).toBe(0);
  });

  it('per-compositor stores are independent', () => {
    const a = createStore();
    const b = createStore();
    const id = freshId('layer');
    setLayersInStore(a, [makeLayer(id, { opacity: 0.3 })]);
    expect(getLayersFromStore(a)).toHaveLength(1);
    expect(getLayersFromStore(b)).toHaveLength(0);
  });
});

// ── setLayersInStore ────────────────────────────────────────────────────────

describe('setLayersInStore', () => {
  it('writes ID list and per-layer atoms', () => {
    const store = createStore();
    const id0 = freshId('in');
    const id1 = freshId('in');
    const layers = [makeLayer(id0), makeLayer(id1, { opacity: 0.5 })];

    setLayersInStore(store, layers);

    expect(store.get(layerIdsAtom)).toEqual([id0, id1]);
    expect(store.get(layerAtoms(id0))).toEqual(layers[0]);
    expect(store.get(layerAtoms(id1))?.opacity).toBe(0.5);
    expect(store.get(layerOpacityAtom(id1))).toBe(0.5);
  });

  it('skips per-layer write when all fields are equal (field-level diff)', () => {
    const store = createStore();
    const id = freshId('in');
    const a = makeLayer(id, { x: 100 });
    setLayersInStore(store, [a]);
    const after = store.get(layerAtoms(id));

    // Same field values but a different object identity (e.g. after a
    // mergeOverlayState spread).  The atom must NOT be re-written, so the
    // reference stays equal.
    const aCopy = { ...a };
    setLayersInStore(store, [aCopy]);

    expect(store.get(layerAtoms(id))).toBe(after);
  });

  it('writes per-layer atom when any field differs', () => {
    const store = createStore();
    const id = freshId('in');
    setLayersInStore(store, [makeLayer(id, { opacity: 1 })]);
    const before = store.get(layerAtoms(id));

    setLayersInStore(store, [makeLayer(id, { opacity: 0.5 })]);

    const after = store.get(layerAtoms(id));
    expect(after).not.toBe(before);
    expect(after?.opacity).toBe(0.5);
  });

  it('skips ID list write when IDs are unchanged', () => {
    const store = createStore();
    const id0 = freshId('in');
    const id1 = freshId('in');
    setLayersInStore(store, [makeLayer(id0), makeLayer(id1)]);
    const idsBefore = store.get(layerIdsAtom);

    // Same IDs, but new layer objects (e.g. an opacity edit on one).
    setLayersInStore(store, [makeLayer(id0), makeLayer(id1, { opacity: 0.2 })]);

    expect(store.get(layerIdsAtom)).toBe(idsBefore);
  });

  it('writes new ID list when an ID is added or removed', () => {
    const store = createStore();
    const id0 = freshId('in');
    const id1 = freshId('in');
    setLayersInStore(store, [makeLayer(id0)]);

    setLayersInStore(store, [makeLayer(id0), makeLayer(id1)]);
    expect(store.get(layerIdsAtom)).toEqual([id0, id1]);

    setLayersInStore(store, [makeLayer(id1)]);
    expect(store.get(layerIdsAtom)).toEqual([id1]);
  });

  it('nulls atoms for removed layers (without touching the global family cache)', () => {
    const store = createStore();
    const id0 = freshId('in');
    const id1 = freshId('in');
    setLayersInStore(store, [makeLayer(id0), makeLayer(id1)]);

    setLayersInStore(store, [makeLayer(id1)]);

    expect(store.get(layerAtoms(id0))).toBeNull();
    expect(store.get(layerAtoms(id1))).not.toBeNull();
  });

  it('survives full state replacement', () => {
    const store = createStore();
    const id0 = freshId('in');
    setLayersInStore(store, [makeLayer(id0, { x: 0 })]);

    setLayersInStore(store, [makeLayer(id0, { x: 999 })]);
    expect(store.get(layerAtoms(id0))?.x).toBe(999);

    setLayersInStore(store, []);
    expect(store.get(layerAtoms(id0))).toBeNull();
    expect(store.get(layerIdsAtom)).toEqual([]);
  });
});

// ── setTextOverlaysInStore ──────────────────────────────────────────────────

describe('setTextOverlaysInStore', () => {
  it('writes IDs and per-overlay atoms', () => {
    const store = createStore();
    const id = freshId('text');
    setTextOverlaysInStore(store, [makeTextOverlay(id, { fontSize: 48 })]);

    expect(store.get(textOverlayIdsAtom)).toEqual([id]);
    expect(store.get(textOverlayAtoms(id))?.fontSize).toBe(48);
  });

  it('skips per-overlay write when text/font/color all match', () => {
    const store = createStore();
    const id = freshId('text');
    const a = makeTextOverlay(id, { text: 'same' });
    setTextOverlaysInStore(store, [a]);
    const before = store.get(textOverlayAtoms(id));

    setTextOverlaysInStore(store, [{ ...a }]);
    expect(store.get(textOverlayAtoms(id))).toBe(before);
  });

  it('detects text-specific field changes (color array element)', () => {
    const store = createStore();
    const id = freshId('text');
    setTextOverlaysInStore(store, [makeTextOverlay(id, { color: [255, 0, 0, 255] })]);
    const before = store.get(textOverlayAtoms(id));

    setTextOverlaysInStore(store, [makeTextOverlay(id, { color: [255, 0, 0, 128] })]);

    expect(store.get(textOverlayAtoms(id))).not.toBe(before);
    expect(store.get(textOverlayAtoms(id))?.color[3]).toBe(128);
  });

  it('detects measuredTextWidth / measuredTextHeight changes', () => {
    const store = createStore();
    const id = freshId('text');
    setTextOverlaysInStore(store, [makeTextOverlay(id)]);
    const before = store.get(textOverlayAtoms(id));

    setTextOverlaysInStore(store, [
      makeTextOverlay(id, { measuredTextWidth: 123, measuredTextHeight: 45 }),
    ]);

    expect(store.get(textOverlayAtoms(id))).not.toBe(before);
    expect(store.get(textOverlayAtoms(id))?.measuredTextWidth).toBe(123);
  });

  it('nulls removed text overlay atoms', () => {
    const store = createStore();
    const id0 = freshId('text');
    const id1 = freshId('text');
    setTextOverlaysInStore(store, [makeTextOverlay(id0), makeTextOverlay(id1)]);

    setTextOverlaysInStore(store, [makeTextOverlay(id1)]);

    expect(store.get(textOverlayAtoms(id0))).toBeNull();
    expect(store.get(textOverlayAtoms(id1))).not.toBeNull();
  });
});

// ── setImageOverlaysInStore ─────────────────────────────────────────────────

describe('setImageOverlaysInStore', () => {
  it('writes IDs and per-overlay atoms', () => {
    const store = createStore();
    const id = freshId('img');
    setImageOverlaysInStore(store, [makeImageOverlay(id)]);
    expect(store.get(imageOverlayIdsAtom)).toEqual([id]);
    expect(store.get(imageOverlayAtoms(id))?.assetPath).toMatch(/test\.png$/);
  });

  it('detects assetPath changes', () => {
    const store = createStore();
    const id = freshId('img');
    setImageOverlaysInStore(store, [makeImageOverlay(id, { assetPath: 'a.png' })]);
    const before = store.get(imageOverlayAtoms(id));

    setImageOverlaysInStore(store, [makeImageOverlay(id, { assetPath: 'b.png' })]);

    expect(store.get(imageOverlayAtoms(id))).not.toBe(before);
    expect(store.get(imageOverlayAtoms(id))?.assetPath).toBe('b.png');
  });

  it('skips write when fields all match (object copy)', () => {
    const store = createStore();
    const id = freshId('img');
    const a = makeImageOverlay(id);
    setImageOverlaysInStore(store, [a]);
    const before = store.get(imageOverlayAtoms(id));

    setImageOverlaysInStore(store, [{ ...a }]);
    expect(store.get(imageOverlayAtoms(id))).toBe(before);
  });

  it('nulls removed image overlay atoms', () => {
    const store = createStore();
    const id0 = freshId('img');
    const id1 = freshId('img');
    setImageOverlaysInStore(store, [makeImageOverlay(id0), makeImageOverlay(id1)]);

    setImageOverlaysInStore(store, [makeImageOverlay(id1)]);

    expect(store.get(imageOverlayAtoms(id0))).toBeNull();
    expect(store.get(imageOverlayAtoms(id1))).not.toBeNull();
  });
});

// ── Derived atoms ───────────────────────────────────────────────────────────

describe('allLayersAtom / allTextOverlaysAtom / allImageOverlaysAtom', () => {
  it('returns layers in ID order, filtering nulled entries', () => {
    const store = createStore();
    const id0 = freshId('in');
    const id1 = freshId('in');
    setLayersInStore(store, [makeLayer(id0, { x: 1 }), makeLayer(id1, { x: 2 })]);

    const layers = store.get(allLayersAtom);
    expect(layers.map((l) => l.x)).toEqual([1, 2]);

    setLayersInStore(store, [makeLayer(id1, { x: 2 })]);
    expect(store.get(allLayersAtom).map((l) => l.id)).toEqual([id1]);
  });

  it('getLayersFromStore matches the atom view', () => {
    const store = createStore();
    const id = freshId('in');
    setLayersInStore(store, [makeLayer(id)]);
    expect(getLayersFromStore(store)).toEqual(store.get(allLayersAtom));
  });

  it('reflects text + image overlay arrays through helpers', () => {
    const store = createStore();
    const tid = freshId('text');
    const iid = freshId('img');
    setTextOverlaysInStore(store, [makeTextOverlay(tid)]);
    setImageOverlaysInStore(store, [makeImageOverlay(iid)]);

    expect(getTextOverlaysFromStore(store)).toEqual(store.get(allTextOverlaysAtom));
    expect(getImageOverlaysFromStore(store)).toEqual(store.get(allImageOverlaysAtom));
    expect(getTextOverlaysFromStore(store)).toHaveLength(1);
    expect(getImageOverlaysFromStore(store)).toHaveLength(1);
  });
});

// ── layerOpacityAtom / layerRotationAtom ────────────────────────────────────

describe('layerOpacityAtom / layerRotationAtom (derived families)', () => {
  it('tracks the parent layer atom', () => {
    const store = createStore();
    const id = freshId('in');
    setLayersInStore(store, [makeLayer(id, { opacity: 0.4, rotationDegrees: 90 })]);

    expect(store.get(layerOpacityAtom(id))).toBe(0.4);
    expect(store.get(layerRotationAtom(id))).toBe(90);

    setLayersInStore(store, [makeLayer(id, { opacity: 0.9, rotationDegrees: 180 })]);
    expect(store.get(layerOpacityAtom(id))).toBe(0.9);
    expect(store.get(layerRotationAtom(id))).toBe(180);
  });

  it('returns sentinel defaults when the layer is missing', () => {
    const store = createStore();
    const id = freshId('in-missing');
    expect(store.get(layerOpacityAtom(id))).toBe(1);
    expect(store.get(layerRotationAtom(id))).toBe(0);
  });
});

// ── selectedLayerKindAtom ───────────────────────────────────────────────────

describe('selectedLayerKindAtom', () => {
  it('returns null when nothing is selected', () => {
    const store = createStore();
    expect(store.get(selectedLayerKindAtom)).toBeNull();
  });

  it('classifies the selection by which ID list contains it', () => {
    const store = createStore();
    const vid = freshId('in');
    const tid = freshId('text');
    const iid = freshId('img');
    setLayersInStore(store, [makeLayer(vid)]);
    setTextOverlaysInStore(store, [makeTextOverlay(tid)]);
    setImageOverlaysInStore(store, [makeImageOverlay(iid)]);

    store.set(selectedLayerIdAtom, vid);
    expect(store.get(selectedLayerKindAtom)).toBe('video');

    store.set(selectedLayerIdAtom, tid);
    expect(store.get(selectedLayerKindAtom)).toBe('text');

    store.set(selectedLayerIdAtom, iid);
    expect(store.get(selectedLayerKindAtom)).toBe('image');
  });

  it('returns null when the selected ID is in no list (avoiding phantom cache lookups)', () => {
    const store = createStore();
    store.set(selectedLayerIdAtom, freshId('unknown'));
    expect(store.get(selectedLayerKindAtom)).toBeNull();
  });
});
