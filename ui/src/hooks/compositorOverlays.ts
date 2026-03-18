// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Overlay CRUD, layer property updates (opacity / rotation / z-index /
 * visibility / mirror), and batch reorder logic extracted from
 * useCompositorLayers to keep the main hook under the max-lines limit.
 *
 * Opacity and rotation updates write to Jotai atoms via the atom-backed
 * setLayers setter.  Only the affected layer's atom changes, so only
 * components subscribed to that atom re-render — no zero-render DOM hack
 * or sliderActiveRef needed.
 */

import { useCallback, useEffect, useRef } from 'react';

import type { CommitAdapter } from './compositorCommit';
import {
  DEFAULT_OPACITY,
  DEFAULT_ROTATION_DEGREES,
  DEFAULT_MIRROR_HORIZONTAL,
  DEFAULT_MIRROR_VERTICAL,
  DEFAULT_VISIBLE,
  DEFAULT_FONT_SIZE,
  DEFAULT_FONT_NAME,
  DEFAULT_TEXT_COLOR,
  DEFAULT_OVERLAY_X,
  DEFAULT_OVERLAY_Y_BASE,
  DEFAULT_OVERLAY_Y_STEP,
  DEFAULT_TEXT_WIDTH,
  DEFAULT_TEXT_HEIGHT,
  DEFAULT_CROP_X,
  DEFAULT_CROP_Y,
} from './compositorConstants';
import type { LayerKind } from './compositorConstants';
import type { LayerState, TextOverlayState, ImageOverlayState } from './compositorLayerParsers';

// ── Shared dependency bag ────────────────────────────────────────────────

export interface OverlayDeps {
  commitAdapter: CommitAdapter | null;
  setLayers: React.Dispatch<React.SetStateAction<LayerState[]>>;
  setTextOverlays: React.Dispatch<React.SetStateAction<TextOverlayState[]>>;
  setImageOverlays: React.Dispatch<React.SetStateAction<ImageOverlayState[]>>;
  setSelectedLayerId: React.Dispatch<React.SetStateAction<string | null>>;
  layersRef: React.MutableRefObject<LayerState[]>;
  textOverlaysRef: React.MutableRefObject<TextOverlayState[]>;
  imageOverlaysRef: React.MutableRefObject<ImageOverlayState[]>;
  throttledConfigChange: ((layers: LayerState[]) => void) | null;
  throttledOverlayCommit: ((text: TextOverlayState[], img: ImageOverlayState[]) => void) | null;
}

// ── Hook ─────────────────────────────────────────────────────────────────

export function useCompositorOverlays(deps: OverlayDeps) {
  const {
    commitAdapter,
    setLayers,
    setTextOverlays,
    setImageOverlays,
    setSelectedLayerId,
    layersRef,
    textOverlaysRef,
    imageOverlaysRef,
    throttledConfigChange,
    throttledOverlayCommit,
  } = deps;

  // ── selectLayer ──────────────────────────────────────────────────────

  const selectLayer = useCallback(
    (id: string | null) => setSelectedLayerId(id),
    [setSelectedLayerId]
  );

  // ── Layer property updates ───────────────────────────────────────────
  //
  // Opacity and rotation write to atoms via the atom-backed setLayers.
  // Only the affected layer's atom changes — fine-grained reactivity
  // means just the slider control and that one VideoLayer re-render.

  const updateLayerOpacity = useCallback(
    (layerId: string, opacity: number) => {
      const clamped = Math.max(0, Math.min(1, opacity));
      setLayers((prev) => {
        const idx = prev.findIndex((l) => l.id === layerId);
        if (idx === -1) return prev;
        return prev.map((l, i) => (i === idx ? { ...l, opacity: clamped } : l));
      });
      throttledConfigChange?.(layersRef.current);
    },
    [setLayers, layersRef, throttledConfigChange]
  );

  const updateLayerRotation = useCallback(
    (layerId: string, degrees: number) => {
      setLayers((prev) => {
        const idx = prev.findIndex((l) => l.id === layerId);
        if (idx === -1) return prev;
        return prev.map((l, i) => (i === idx ? { ...l, rotationDegrees: degrees } : l));
      });
      throttledConfigChange?.(layersRef.current);
    },
    [setLayers, layersRef, throttledConfigChange]
  );

  const updateLayerPositionSize = useCallback(
    (layerId: string, patch: { x?: number; y?: number; width?: number; height?: number }) => {
      setLayers((prev) => {
        return prev.map((l) => {
          if (l.id !== layerId) return l;
          const updated = { ...l };
          if (patch.x !== undefined) updated.x = patch.x;
          if (patch.y !== undefined) updated.y = patch.y;
          // Preserve aspect ratio for dimension changes on video layers
          if (
            patch.width !== undefined &&
            patch.height === undefined &&
            l.width > 0 &&
            l.height > 0
          ) {
            const ar = l.width / l.height;
            updated.width = Math.max(20, patch.width);
            updated.height = Math.max(20, Math.round(updated.width / ar));
          } else if (
            patch.height !== undefined &&
            patch.width === undefined &&
            l.width > 0 &&
            l.height > 0
          ) {
            const ar = l.width / l.height;
            updated.height = Math.max(20, patch.height);
            updated.width = Math.max(20, Math.round(updated.height * ar));
          } else {
            if (patch.width !== undefined) updated.width = Math.max(20, patch.width);
            if (patch.height !== undefined) updated.height = Math.max(20, patch.height);
          }
          return updated;
        });
      });
      throttledConfigChange?.(layersRef.current);
    },
    [setLayers, layersRef, throttledConfigChange]
  );

  const updateLayerZIndex = useCallback(
    (layerId: string, zIndex: number) => {
      setLayers((prev) => {
        return prev
          .map((l) => (l.id === layerId ? { ...l, zIndex } : l))
          .sort((a, b) => a.zIndex - b.zIndex);
      });
      throttledConfigChange?.(layersRef.current);
    },
    [setLayers, layersRef, throttledConfigChange]
  );

  // ── Visibility toggle ──────────────────────────────────────────────

  const toggleLayerVisibility = useCallback(
    (layerId: string) => {
      if (layersRef.current.some((l) => l.id === layerId)) {
        setLayers((prev) =>
          prev.map((l) => (l.id === layerId ? { ...l, visible: !l.visible } : l))
        );
        throttledConfigChange?.(layersRef.current);
        return;
      }
      if (textOverlaysRef.current.some((o) => o.id === layerId)) {
        setTextOverlays((prev) => {
          const next = prev.map((o) => (o.id === layerId ? { ...o, visible: !o.visible } : o));
          commitOverlaysRef.current(next, imageOverlaysRef.current);
          return next;
        });
        return;
      }
      if (imageOverlaysRef.current.some((o) => o.id === layerId)) {
        setImageOverlays((prev) => {
          const next = prev.map((o) => (o.id === layerId ? { ...o, visible: !o.visible } : o));
          commitOverlaysRef.current(textOverlaysRef.current, next);
          return next;
        });
      }
    },
    [
      layersRef,
      textOverlaysRef,
      imageOverlaysRef,
      setLayers,
      setTextOverlays,
      setImageOverlays,
      throttledConfigChange,
    ]
  );

  // ── Mirror toggle ──────────────────────────────────────────────────

  const updateLayerMirror = useCallback(
    (layerId: string, axis: 'horizontal' | 'vertical') => {
      const field = axis === 'horizontal' ? 'mirrorHorizontal' : 'mirrorVertical';

      if (layersRef.current.some((l) => l.id === layerId)) {
        setLayers((prev) => prev.map((l) => (l.id === layerId ? { ...l, [field]: !l[field] } : l)));
        throttledConfigChange?.(layersRef.current);
        return;
      }
      if (textOverlaysRef.current.some((o) => o.id === layerId)) {
        setTextOverlays((prev) => {
          const next = prev.map((o) => (o.id === layerId ? { ...o, [field]: !o[field] } : o));
          commitOverlaysRef.current(next, imageOverlaysRef.current);
          return next;
        });
        return;
      }
      if (imageOverlaysRef.current.some((o) => o.id === layerId)) {
        setImageOverlays((prev) => {
          const next = prev.map((o) => (o.id === layerId ? { ...o, [field]: !o[field] } : o));
          commitOverlaysRef.current(textOverlaysRef.current, next);
          return next;
        });
      }
    },
    [
      layersRef,
      textOverlaysRef,
      imageOverlaysRef,
      setLayers,
      setTextOverlays,
      setImageOverlays,
      throttledConfigChange,
    ]
  );

  // ── Crop / zoom update ──────────────────────────────────────────────────

  const updateLayerCropZoom = useCallback(
    (layerId: string, patch: { cropX?: number; cropY?: number; cropZoom?: number }) => {
      setLayers((prev) => {
        return prev.map((l) => {
          if (l.id !== layerId) return l;
          const updated = { ...l };
          if (patch.cropZoom !== undefined) updated.cropZoom = Math.max(1.0, patch.cropZoom);
          if (patch.cropX !== undefined) updated.cropX = Math.max(0, Math.min(1, patch.cropX));
          if (patch.cropY !== undefined) updated.cropY = Math.max(0, Math.min(1, patch.cropY));
          // Reset pan/tilt when zoom returns to 1.0
          if (updated.cropZoom <= 1.0) {
            updated.cropX = DEFAULT_CROP_X;
            updated.cropY = DEFAULT_CROP_Y;
          }
          return updated;
        });
      });
      throttledConfigChange?.(layersRef.current);
    },
    [setLayers, layersRef, throttledConfigChange]
  );

  // ── Overlay commit helper ──────────────────────────────────────────

  const commitOverlays = useCallback(
    (nextText: TextOverlayState[], nextImg: ImageOverlayState[]) => {
      commitAdapter?.commitOverlays(nextText, nextImg);
    },
    [commitAdapter]
  );

  // Stable ref so pointer-up / visibility / mirror can call latest commit.
  const commitOverlaysRef = useRef(commitOverlays);
  useEffect(() => {
    commitOverlaysRef.current = commitOverlays;
  }, [commitOverlays]);

  // ── Generic overlay update / remove ────────────────────────────────

  const updateOverlay = useCallback(
    <T extends { id: string }>(
      id: string,
      updates: Partial<Omit<T, 'id'>>,
      setter: React.Dispatch<React.SetStateAction<T[]>>,
      buildCommitArgs: (next: T[]) => [TextOverlayState[], ImageOverlayState[]]
    ) => {
      setter((prev) => {
        const next = prev.map((o) => (o.id === id ? { ...o, ...updates } : o));
        const [text, img] = buildCommitArgs(next);
        if (throttledOverlayCommit) {
          throttledOverlayCommit(text, img);
        } else {
          commitOverlaysRef.current(text, img);
        }
        return next;
      });
    },
    [throttledOverlayCommit]
  );

  const removeOverlay = useCallback(
    <T extends { id: string }>(
      id: string,
      setter: React.Dispatch<React.SetStateAction<T[]>>,
      buildCommitArgs: (next: T[]) => [TextOverlayState[], ImageOverlayState[]]
    ) => {
      setter((prev) => {
        const next = prev.filter((o) => o.id !== id);
        const [text, img] = buildCommitArgs(next);
        commitOverlays(text, img);
        return next;
      });
      setSelectedLayerId(null);
    },
    [commitOverlays, setSelectedLayerId]
  );

  // ── Z-index helpers ────────────────────────────────────────────────

  const maxZIndex = useCallback((): number => {
    let max = -1;
    for (const l of layersRef.current) if (l.zIndex > max) max = l.zIndex;
    for (const o of textOverlaysRef.current) if (o.zIndex > max) max = o.zIndex;
    for (const o of imageOverlaysRef.current) if (o.zIndex > max) max = o.zIndex;
    return max;
  }, [layersRef, textOverlaysRef, imageOverlaysRef]);

  // ── Text overlay CRUD ──────────────────────────────────────────────

  const addTextOverlay = useCallback(
    (text: string) => {
      setTextOverlays((prev) => {
        const newId = crypto.randomUUID();
        const next: TextOverlayState[] = [
          ...prev,
          {
            id: newId,
            text,
            x: DEFAULT_OVERLAY_X,
            y: DEFAULT_OVERLAY_Y_BASE + prev.length * DEFAULT_OVERLAY_Y_STEP,
            width: DEFAULT_TEXT_WIDTH,
            height: DEFAULT_TEXT_HEIGHT,
            color: DEFAULT_TEXT_COLOR,
            fontSize: DEFAULT_FONT_SIZE,
            fontName: DEFAULT_FONT_NAME,
            opacity: DEFAULT_OPACITY,
            rotationDegrees: DEFAULT_ROTATION_DEGREES,
            zIndex: maxZIndex() + 1,
            mirrorHorizontal: DEFAULT_MIRROR_HORIZONTAL,
            mirrorVertical: DEFAULT_MIRROR_VERTICAL,
            visible: DEFAULT_VISIBLE,
          },
        ];
        commitOverlays(next, imageOverlaysRef.current);
        setSelectedLayerId(newId);
        return next;
      });
    },
    [commitOverlays, maxZIndex, setTextOverlays, imageOverlaysRef, setSelectedLayerId]
  );

  const updateTextOverlay = useCallback(
    (id: string, updates: Partial<Omit<TextOverlayState, 'id'>>) => {
      const existing = textOverlaysRef.current.find((o) => o.id === id);
      if (existing) {
        const fontSize = updates.fontSize ?? existing.fontSize;
        const text = updates.text ?? existing.text;
        const minHeight = Math.ceil(fontSize * 1.4);
        if (existing.height < minHeight && !('height' in updates)) {
          updates = { ...updates, height: minHeight };
        }
        const minWidth = Math.ceil(text.length * fontSize * 0.6);
        if (existing.width < minWidth && !('width' in updates)) {
          updates = { ...updates, width: minWidth };
        }
      }
      updateOverlay(id, updates, setTextOverlays, (next) => [next, imageOverlaysRef.current]);
    },
    [updateOverlay, textOverlaysRef, setTextOverlays, imageOverlaysRef]
  );

  const removeTextOverlay = useCallback(
    (id: string) =>
      removeOverlay(id, setTextOverlays, (next) => [
        next as unknown as TextOverlayState[],
        imageOverlaysRef.current,
      ]),
    [removeOverlay, setTextOverlays, imageOverlaysRef]
  );

  // ── Image overlay CRUD ─────────────────────────────────────────────

  const addImageOverlay = useCallback(
    (dataBase64: string, naturalWidth?: number, naturalHeight?: number) => {
      setImageOverlays((prev) => {
        const maxDim = 200;
        let w = maxDim;
        let h = maxDim;
        if (naturalWidth && naturalHeight && naturalWidth > 0 && naturalHeight > 0) {
          const scale = Math.min(maxDim / naturalWidth, maxDim / naturalHeight, 1);
          w = Math.max(1, Math.round(naturalWidth * scale));
          h = Math.max(1, Math.round(naturalHeight * scale));
        }
        const newId = crypto.randomUUID();
        const next: ImageOverlayState[] = [
          ...prev,
          {
            id: newId,
            dataBase64,
            x: DEFAULT_OVERLAY_X,
            y: DEFAULT_OVERLAY_Y_BASE + prev.length * 60,
            width: w,
            height: h,
            opacity: DEFAULT_OPACITY,
            rotationDegrees: DEFAULT_ROTATION_DEGREES,
            zIndex: maxZIndex() + 1,
            mirrorHorizontal: DEFAULT_MIRROR_HORIZONTAL,
            mirrorVertical: DEFAULT_MIRROR_VERTICAL,
            visible: DEFAULT_VISIBLE,
          },
        ];
        commitOverlays(textOverlaysRef.current, next);
        setSelectedLayerId(newId);
        return next;
      });
    },
    [commitOverlays, maxZIndex, setImageOverlays, textOverlaysRef, setSelectedLayerId]
  );

  const updateImageOverlay = useCallback(
    (id: string, updates: Partial<Omit<ImageOverlayState, 'id'>>) =>
      updateOverlay(id, updates, setImageOverlays, (next) => [textOverlaysRef.current, next]),
    [updateOverlay, setImageOverlays, textOverlaysRef]
  );

  const removeImageOverlay = useCallback(
    (id: string) =>
      removeOverlay(id, setImageOverlays, (next) => [
        textOverlaysRef.current,
        next as unknown as ImageOverlayState[],
      ]),
    [removeOverlay, setImageOverlays, textOverlaysRef]
  );

  // ── Batch reorder ──────────────────────────────────────────────────

  const reorderLayers = useCallback(
    (entries: Array<{ id: string; kind: LayerKind; zIndex: number }>) => {
      const zMap = new Map<string, number>();
      for (const e of entries) zMap.set(e.id, e.zIndex);

      let nextLayers = layersRef.current;
      const hasVideoChanges = nextLayers.some((l) => {
        const z = zMap.get(l.id);
        return z !== undefined && z !== l.zIndex;
      });
      if (hasVideoChanges) {
        nextLayers = nextLayers
          .map((l) => {
            const z = zMap.get(l.id);
            return z !== undefined && z !== l.zIndex ? { ...l, zIndex: z } : l;
          })
          .sort((a, b) => a.zIndex - b.zIndex);
        setLayers(nextLayers);
      }

      let nextText = textOverlaysRef.current;
      const hasTextChanges = nextText.some((o) => {
        const z = zMap.get(o.id);
        return z !== undefined && z !== o.zIndex;
      });
      if (hasTextChanges) {
        nextText = nextText.map((o) => {
          const z = zMap.get(o.id);
          return z !== undefined && z !== o.zIndex ? { ...o, zIndex: z } : o;
        });
        setTextOverlays(nextText);
      }

      let nextImg = imageOverlaysRef.current;
      const hasImgChanges = nextImg.some((o) => {
        const z = zMap.get(o.id);
        return z !== undefined && z !== o.zIndex;
      });
      if (hasImgChanges) {
        nextImg = nextImg.map((o) => {
          const z = zMap.get(o.id);
          return z !== undefined && z !== o.zIndex ? { ...o, zIndex: z } : o;
        });
        setImageOverlays(nextImg);
      }

      if (hasVideoChanges || hasTextChanges || hasImgChanges) {
        commitAdapter?.commitAll(nextLayers, nextText, nextImg, {
          layers: hasVideoChanges,
          overlays: hasTextChanges || hasImgChanges,
        });
      }
    },
    [
      commitAdapter,
      layersRef,
      textOverlaysRef,
      imageOverlaysRef,
      setLayers,
      setTextOverlays,
      setImageOverlays,
    ]
  );

  return {
    selectLayer,
    updateLayerOpacity,
    updateLayerRotation,
    updateLayerPositionSize,
    updateLayerZIndex,
    toggleLayerVisibility,
    updateLayerMirror,
    updateLayerCropZoom,
    commitOverlaysRef,
    addTextOverlay,
    updateTextOverlay,
    removeTextOverlay,
    addImageOverlay,
    updateImageOverlay,
    removeImageOverlay,
    reorderLayers,
  };
}
