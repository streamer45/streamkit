// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Keyboard shortcuts for the video compositor.
 *
 * - Delete / Backspace → remove selected text or image overlay
 *   (video layers cannot be removed).
 * - Arrow keys → nudge selected layer by SNAP_GRID (10 px).
 * - Shift+Arrow → nudge by 1 px for fine positioning.
 * - Escape → deselect the current layer.
 *
 * The hook attaches a `keydown` listener scoped to the compositor DOM
 * tree so it doesn't interfere with other keyboard handling in the app.
 */

import { useCallback, useEffect, useRef } from 'react';

import type { LayerKind } from './compositorConstants';
import { SNAP_GRID } from './compositorLayerParsers';
import type { LayerState, TextOverlayState, ImageOverlayState } from './compositorLayerParsers';

// ── Dependency bag ──────────────────────────────────────────────────────────

export interface CompositorKeyboardDeps {
  /** Currently selected layer id (null = nothing selected). */
  selectedLayerId: string | null;
  /** Deselect the current layer. */
  selectLayer: (id: string | null) => void;
  /** Remove a text overlay by id. */
  removeTextOverlay: (id: string) => void;
  /** Remove an image overlay by id. */
  removeImageOverlay: (id: string) => void;

  // ── Refs for reading current state without re-render deps ────────────
  layersRef: React.MutableRefObject<LayerState[]>;
  textOverlaysRef: React.MutableRefObject<TextOverlayState[]>;
  imageOverlaysRef: React.MutableRefObject<ImageOverlayState[]>;

  // ── Mutators (video layers) ──────────────────────────────────────────
  setLayers: React.Dispatch<React.SetStateAction<LayerState[]>>;
  throttledConfigChange: ((layers: LayerState[]) => void) | null;

  // ── Mutators (overlays) ──────────────────────────────────────────────
  updateTextOverlay: (id: string, updates: Partial<Omit<TextOverlayState, 'id'>>) => void;
  updateImageOverlay: (id: string, updates: Partial<Omit<ImageOverlayState, 'id'>>) => void;

  /** Whether the compositor is in a disabled / read-only state. */
  disabled?: boolean;
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/** Arrow key names that map to a nudge direction. */
const ARROW_KEYS = new Set(['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight']);

/** Compute the (dx, dy) nudge delta for an arrow key press. */
function arrowDelta(key: string, shift: boolean): [number, number] {
  const step = shift ? 1 : SNAP_GRID;
  const dx = key === 'ArrowLeft' ? -step : key === 'ArrowRight' ? step : 0;
  const dy = key === 'ArrowUp' ? -step : key === 'ArrowDown' ? step : 0;
  return [dx, dy];
}

// ── Hook ────────────────────────────────────────────────────────────────────

export function useCompositorKeyboard(
  wrapperRef: React.RefObject<HTMLDivElement | null>,
  deps: CompositorKeyboardDeps
) {
  const {
    selectedLayerId,
    selectLayer,
    removeTextOverlay,
    removeImageOverlay,
    layersRef,
    textOverlaysRef,
    imageOverlaysRef,
    setLayers,
    throttledConfigChange,
    updateTextOverlay,
    updateImageOverlay,
    disabled,
  } = deps;

  // Keep a stable ref to the selected id so the keydown handler never
  // goes stale (the handler itself is intentionally stable).
  const selectedRef = useRef(selectedLayerId);
  useEffect(() => {
    selectedRef.current = selectedLayerId;
  }, [selectedLayerId]);

  /** Determine the kind of a given layer id by checking each list. */
  const kindOf = useCallback(
    (id: string): LayerKind | null => {
      if (layersRef.current.some((l) => l.id === id)) return 'video';
      if (textOverlaysRef.current.some((o) => o.id === id)) return 'text';
      if (imageOverlaysRef.current.some((o) => o.id === id)) return 'image';
      return null;
    },
    [layersRef, textOverlaysRef, imageOverlaysRef]
  );

  const nudge = useCallback(
    (id: string, kind: LayerKind, dx: number, dy: number) => {
      if (kind === 'video') {
        setLayers((prev) => {
          const next = prev.map((l) => (l.id === id ? { ...l, x: l.x + dx, y: l.y + dy } : l));
          throttledConfigChange?.(next);
          return next;
        });
      } else if (kind === 'text') {
        const cur = textOverlaysRef.current.find((o) => o.id === id);
        if (cur) updateTextOverlay(id, { x: cur.x + dx, y: cur.y + dy });
      } else if (kind === 'image') {
        const cur = imageOverlaysRef.current.find((o) => o.id === id);
        if (cur) updateImageOverlay(id, { x: cur.x + dx, y: cur.y + dy });
      }
    },
    [
      setLayers,
      throttledConfigChange,
      textOverlaysRef,
      imageOverlaysRef,
      updateTextOverlay,
      updateImageOverlay,
    ]
  );

  /** Handle Delete / Backspace: remove text or image overlay. */
  const handleDelete = useCallback(
    (id: string) => {
      const kind = kindOf(id);
      if (kind === 'text') removeTextOverlay(id);
      else if (kind === 'image') removeImageOverlay(id);
      // Video layers cannot be removed — silently ignore.
    },
    [kindOf, removeTextOverlay, removeImageOverlay]
  );

  /** Handle arrow key nudge for the selected layer. */
  const handleArrow = useCallback(
    (id: string, key: string, shift: boolean) => {
      const kind = kindOf(id);
      if (!kind) return;
      const [dx, dy] = arrowDelta(key, shift);
      nudge(id, kind, dx, dy);
    },
    [kindOf, nudge]
  );

  useEffect(() => {
    const wrapper = wrapperRef.current;
    if (!wrapper || disabled) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      // Only act when the event originates inside the compositor tree.
      if (!wrapper.contains(e.target as Node)) return;

      // Don't intercept when the user is typing in an input / textarea.
      const tag = (e.target as HTMLElement).tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA') return;

      const id = selectedRef.current;

      if (e.key === 'Escape') {
        selectLayer(null);
        e.preventDefault();
        return;
      }

      if (!id) return;

      if (e.key === 'Delete' || e.key === 'Backspace') {
        handleDelete(id);
        e.preventDefault();
        return;
      }

      if (ARROW_KEYS.has(e.key)) {
        handleArrow(id, e.key, e.shiftKey);
        e.preventDefault();
      }
    };

    wrapper.addEventListener('keydown', handleKeyDown);
    return () => wrapper.removeEventListener('keydown', handleKeyDown);
  }, [wrapperRef, disabled, selectLayer, handleDelete, handleArrow]);
}
