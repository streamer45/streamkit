// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Jotai atom definitions for compositor layer state.
 *
 * Each layer (video, text overlay, image overlay) gets its own atom via
 * atomFamily, keyed by ID.  Components subscribe to individual atoms for
 * fine-grained reactivity — an opacity change on one layer only re-renders
 * that layer's VideoLayer component and the slider, not the entire canvas.
 *
 * A per-compositor-instance Jotai store (created via `createStore()`) scopes
 * the atoms so multiple compositor nodes don't share state.  The store is
 * provided to child components via Jotai's own `<Provider store={store}>`.
 */

import { atom, createStore } from 'jotai';
import { atomFamily } from 'jotai-family';

import type { LayerKind } from './compositorConstants';
import type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
  OverlayBase,
} from './compositorLayerParsers';

// ── Store type ──────────────────────────────────────────────────────────────

export type CompositorStore = ReturnType<typeof createStore>;

// ── Core atoms ──────────────────────────────────────────────────────────────

/** Ordered list of video layer IDs. */
export const layerIdsAtom = atom<string[]>([]);
/** Ordered list of text overlay IDs. */
export const textOverlayIdsAtom = atom<string[]>([]);
/** Ordered list of image overlay IDs. */
export const imageOverlayIdsAtom = atom<string[]>([]);

/** Currently selected layer/overlay ID. */
export const selectedLayerIdAtom = atom<string | null>(null);
/** Whether a drag/resize is in progress. */
export const isDraggingAtom = atom(false);

/** Per-layer atom family — each layer has its own atom. */
export const layerAtoms = atomFamily(
  (_id: string) => atom<LayerState | null>(null) // eslint-disable-line @typescript-eslint/no-unused-vars
);
/** Per-text-overlay atom family. */
export const textOverlayAtoms = atomFamily(
  (_id: string) => atom<TextOverlayState | null>(null) // eslint-disable-line @typescript-eslint/no-unused-vars
);
/** Per-image-overlay atom family. */
export const imageOverlayAtoms = atomFamily(
  (_id: string) => atom<ImageOverlayState | null>(null) // eslint-disable-line @typescript-eslint/no-unused-vars
);

// ── Derived atoms ───────────────────────────────────────────────────────────

/** All video layers as an array, derived from per-layer atoms. */
export const allLayersAtom = atom<LayerState[]>((get) => {
  const ids = get(layerIdsAtom);
  return ids.map((id) => get(layerAtoms(id))).filter((l): l is LayerState => l !== null);
});

/** All text overlays as an array. */
export const allTextOverlaysAtom = atom<TextOverlayState[]>((get) => {
  const ids = get(textOverlayIdsAtom);
  return ids
    .map((id) => get(textOverlayAtoms(id)))
    .filter((o): o is TextOverlayState => o !== null);
});

/** All image overlays as an array. */
export const allImageOverlaysAtom = atom<ImageOverlayState[]>((get) => {
  const ids = get(imageOverlayIdsAtom);
  return ids
    .map((id) => get(imageOverlayAtoms(id)))
    .filter((o): o is ImageOverlayState => o !== null);
});

/** The kind of the currently selected layer, or null. */
export const selectedLayerKindAtom = atom<LayerKind | null>((get) => {
  const id = get(selectedLayerIdAtom);
  if (!id) return null;
  if (get(layerAtoms(id))) return 'video';
  if (get(textOverlayAtoms(id))) return 'text';
  if (get(imageOverlayAtoms(id))) return 'image';
  return null;
});

// ── Equality helpers ────────────────────────────────────────────────────────

/** Equality check for string arrays (avoids spurious atom writes). */
function idsEqual(a: readonly string[], b: readonly string[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

/** Shared OverlayBase field equality (id, position, appearance, visibility). */
function baseFieldsEqual(a: OverlayBase, b: OverlayBase): boolean {
  return (
    a.id === b.id &&
    a.x === b.x &&
    a.y === b.y &&
    a.width === b.width &&
    a.height === b.height &&
    a.opacity === b.opacity &&
    a.zIndex === b.zIndex &&
    a.rotationDegrees === b.rotationDegrees &&
    a.mirrorHorizontal === b.mirrorHorizontal &&
    a.mirrorVertical === b.mirrorVertical &&
    a.visible === b.visible
  );
}

/** Field-level equality for LayerState.  Returns true when all fields match,
 *  even if the object references differ (e.g. after a mergeOverlayState
 *  spread from a server echo-back).  This prevents spurious atom writes that
 *  would cascade re-renders to VideoLayers whose data hasn't actually changed. */
function layerEqual(a: LayerState, b: LayerState): boolean {
  return (
    baseFieldsEqual(a, b) && a.cropZoom === b.cropZoom && a.cropX === b.cropX && a.cropY === b.cropY
  );
}

/** Field-level equality for TextOverlayState. */
function textOverlayEqual(a: TextOverlayState, b: TextOverlayState): boolean {
  return (
    baseFieldsEqual(a, b) &&
    a.text === b.text &&
    a.fontSize === b.fontSize &&
    a.fontName === b.fontName &&
    a.color[0] === b.color[0] &&
    a.color[1] === b.color[1] &&
    a.color[2] === b.color[2] &&
    a.color[3] === b.color[3] &&
    a.measuredTextWidth === b.measuredTextWidth &&
    a.measuredTextHeight === b.measuredTextHeight
  );
}

/** Field-level equality for ImageOverlayState. */
function imageOverlayEqual(a: ImageOverlayState, b: ImageOverlayState): boolean {
  return baseFieldsEqual(a, b) && a.dataBase64 === b.dataBase64;
}

// ── Bulk helpers ────────────────────────────────────────────────────────────

/** Set all video layers in the store, skipping unchanged atoms (by value). */
export function setLayersInStore(store: CompositorStore, layers: LayerState[]): void {
  const prevIds = store.get(layerIdsAtom);
  const newIds = layers.map((l) => l.id);

  if (!idsEqual(prevIds, newIds)) {
    store.set(layerIdsAtom, newIds);
  }

  const newIdSet = new Set(newIds);
  for (const layer of layers) {
    const prev = store.get(layerAtoms(layer.id));
    // Field-level comparison: skip write when all values match, even if
    // the object reference differs (common after mergeOverlayState spreads).
    if (prev !== layer && (!prev || !layerEqual(prev, layer))) {
      store.set(layerAtoms(layer.id), layer);
    }
  }

  for (const prevId of prevIds) {
    if (!newIdSet.has(prevId)) {
      store.set(layerAtoms(prevId), null);
      layerAtoms.remove(prevId);
    }
  }
}

/** Set all text overlays in the store, skipping unchanged atoms (by value). */
export function setTextOverlaysInStore(store: CompositorStore, overlays: TextOverlayState[]): void {
  const prevIds = store.get(textOverlayIdsAtom);
  const newIds = overlays.map((o) => o.id);

  if (!idsEqual(prevIds, newIds)) {
    store.set(textOverlayIdsAtom, newIds);
  }

  const newIdSet = new Set(newIds);
  for (const overlay of overlays) {
    const prev = store.get(textOverlayAtoms(overlay.id));
    if (prev !== overlay && (!prev || !textOverlayEqual(prev, overlay))) {
      store.set(textOverlayAtoms(overlay.id), overlay);
    }
  }

  for (const prevId of prevIds) {
    if (!newIdSet.has(prevId)) {
      store.set(textOverlayAtoms(prevId), null);
      textOverlayAtoms.remove(prevId);
    }
  }
}

/** Set all image overlays in the store, skipping unchanged atoms (by value). */
export function setImageOverlaysInStore(
  store: CompositorStore,
  overlays: ImageOverlayState[]
): void {
  const prevIds = store.get(imageOverlayIdsAtom);
  const newIds = overlays.map((o) => o.id);

  if (!idsEqual(prevIds, newIds)) {
    store.set(imageOverlayIdsAtom, newIds);
  }

  const newIdSet = new Set(newIds);
  for (const overlay of overlays) {
    const prev = store.get(imageOverlayAtoms(overlay.id));
    if (prev !== overlay && (!prev || !imageOverlayEqual(prev, overlay))) {
      store.set(imageOverlayAtoms(overlay.id), overlay);
    }
  }

  for (const prevId of prevIds) {
    if (!newIdSet.has(prevId)) {
      store.set(imageOverlayAtoms(prevId), null);
      imageOverlayAtoms.remove(prevId);
    }
  }
}

/** Read all video layers from the store as an array. */
export function getLayersFromStore(store: CompositorStore): LayerState[] {
  return store.get(allLayersAtom);
}

/** Read all text overlays from the store. */
export function getTextOverlaysFromStore(store: CompositorStore): TextOverlayState[] {
  return store.get(allTextOverlaysAtom);
}

/** Read all image overlays from the store. */
export function getImageOverlaysFromStore(store: CompositorStore): ImageOverlayState[] {
  return store.get(allImageOverlaysAtom);
}
