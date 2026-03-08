// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Overlay CRUD, layer property updates (opacity / rotation / z-index /
 * visibility / mirror), and batch reorder logic extracted from
 * useCompositorLayers to keep the main hook under the max-lines limit.
 */

import { useCallback, useEffect, useRef } from 'react';

import {
  buildConfig,
  serializeImageOverlays,
  serializeLayers,
  serializeTextOverlays,
} from './compositorLayerParsers';
import type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
  LayerKind,
} from './compositorLayerParsers';

// ── Shared dependency bag ────────────────────────────────────────────────

export interface OverlayDeps {
  nodeId: string;
  onConfigChange?: (nodeId: string, config: Record<string, unknown>) => void;
  onParamChange?: (nodeId: string, key: string, value: unknown) => void;
  setLayers: React.Dispatch<React.SetStateAction<LayerState[]>>;
  setTextOverlays: React.Dispatch<React.SetStateAction<TextOverlayState[]>>;
  setImageOverlays: React.Dispatch<React.SetStateAction<ImageOverlayState[]>>;
  setSelectedLayerId: React.Dispatch<React.SetStateAction<string | null>>;
  layersRef: React.MutableRefObject<LayerState[]>;
  textOverlaysRef: React.MutableRefObject<TextOverlayState[]>;
  imageOverlaysRef: React.MutableRefObject<ImageOverlayState[]>;
  paramsRef: React.MutableRefObject<Record<string, unknown>>;
  overlayCommitGuardRef: React.MutableRefObject<number>;
  throttledConfigChange: ((layers: LayerState[]) => void) | null;
  throttledOverlayCommit: ((text: TextOverlayState[], img: ImageOverlayState[]) => void) | null;
}

// ── Hook ─────────────────────────────────────────────────────────────────

export function useCompositorOverlays(deps: OverlayDeps) {
  const {
    nodeId,
    onConfigChange,
    onParamChange,
    setLayers,
    setTextOverlays,
    setImageOverlays,
    setSelectedLayerId,
    layersRef,
    textOverlaysRef,
    imageOverlaysRef,
    paramsRef,
    overlayCommitGuardRef,
    throttledConfigChange,
    throttledOverlayCommit,
  } = deps;

  // ── selectLayer ──────────────────────────────────────────────────────

  const selectLayer = useCallback(
    (id: string | null) => setSelectedLayerId(id),
    [setSelectedLayerId]
  );

  // ── Layer property updates ───────────────────────────────────────────

  const updateLayerOpacity = useCallback(
    (layerId: string, opacity: number) => {
      setLayers((prev) => {
        const next = prev.map((l) =>
          l.id === layerId ? { ...l, opacity: Math.max(0, Math.min(1, opacity)) } : l
        );
        throttledConfigChange?.(next);
        return next;
      });
    },
    [setLayers, throttledConfigChange]
  );

  const updateLayerRotation = useCallback(
    (layerId: string, degrees: number) => {
      setLayers((prev) => {
        const next = prev.map((l) => (l.id === layerId ? { ...l, rotationDegrees: degrees } : l));
        throttledConfigChange?.(next);
        return next;
      });
    },
    [setLayers, throttledConfigChange]
  );

  const updateLayerZIndex = useCallback(
    (layerId: string, zIndex: number) => {
      setLayers((prev) => {
        const next = prev
          .map((l) => (l.id === layerId ? { ...l, zIndex } : l))
          .sort((a, b) => a.zIndex - b.zIndex);
        throttledConfigChange?.(next);
        return next;
      });
    },
    [setLayers, throttledConfigChange]
  );

  // ── Visibility toggle ──────────────────────────────────────────────

  const toggleLayerVisibility = useCallback(
    (layerId: string) => {
      if (layersRef.current.some((l) => l.id === layerId)) {
        setLayers((prev) => {
          const next = prev.map((l) => (l.id === layerId ? { ...l, visible: !l.visible } : l));
          throttledConfigChange?.(next);
          return next;
        });
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
        setLayers((prev) => {
          const next = prev.map((l) => (l.id === layerId ? { ...l, [field]: !l[field] } : l));
          throttledConfigChange?.(next);
          return next;
        });
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

  // ── Overlay commit helper ──────────────────────────────────────────

  const commitOverlays = useCallback(
    (nextText: TextOverlayState[], nextImg: ImageOverlayState[]) => {
      overlayCommitGuardRef.current = Date.now();
      if (onConfigChange) {
        const config = buildConfig(paramsRef.current, layersRef.current, nextText, nextImg);
        onConfigChange(nodeId, config);
      } else if (onParamChange) {
        onParamChange(nodeId, 'text_overlays', serializeTextOverlays(nextText));
        onParamChange(nodeId, 'image_overlays', serializeImageOverlays(nextImg));
      }
    },
    [nodeId, onConfigChange, onParamChange, overlayCommitGuardRef, paramsRef, layersRef]
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
      overlayCommitGuardRef.current = Date.now();
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
    [overlayCommitGuardRef, throttledOverlayCommit]
  );

  const removeOverlay = useCallback(
    <T extends { id: string }>(
      id: string,
      idPrefix: string,
      setter: React.Dispatch<React.SetStateAction<T[]>>,
      buildCommitArgs: (next: T[]) => [TextOverlayState[], ImageOverlayState[]]
    ) => {
      setter((prev) => {
        const next = prev
          .filter((o) => o.id !== id)
          .map((o, i) => ({ ...o, id: `${idPrefix}_${i}` }));
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
        const newId = `text_${prev.length}`;
        const next: TextOverlayState[] = [
          ...prev,
          {
            id: newId,
            text,
            x: 40,
            y: 40 + prev.length * 50,
            width: 200,
            height: 40,
            color: [255, 255, 255, 255],
            fontSize: 24,
            fontName: 'dejavu-sans',
            opacity: 1.0,
            rotationDegrees: 0,
            zIndex: maxZIndex() + 1,
            mirrorHorizontal: false,
            mirrorVertical: false,
            visible: true,
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
      removeOverlay(id, 'text', setTextOverlays, (next) => [
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
        const newId = `img_${prev.length}`;
        const next: ImageOverlayState[] = [
          ...prev,
          {
            id: newId,
            dataBase64,
            x: 40,
            y: 40 + prev.length * 60,
            width: w,
            height: h,
            opacity: 1.0,
            rotationDegrees: 0,
            zIndex: maxZIndex() + 1,
            mirrorHorizontal: false,
            mirrorVertical: false,
            visible: true,
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
      removeOverlay(id, 'img', setImageOverlays, (next) => [
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
        overlayCommitGuardRef.current = Date.now();
        if (onConfigChange) {
          const config = buildConfig(paramsRef.current, nextLayers, nextText, nextImg);
          onConfigChange(nodeId, config);
        } else if (onParamChange) {
          if (hasVideoChanges) {
            onParamChange(nodeId, 'layers', serializeLayers(nextLayers));
          }
          if (hasTextChanges || hasImgChanges) {
            onParamChange(nodeId, 'text_overlays', serializeTextOverlays(nextText));
            onParamChange(nodeId, 'image_overlays', serializeImageOverlays(nextImg));
          }
        }
      }
    },
    [
      nodeId,
      onConfigChange,
      onParamChange,
      layersRef,
      textOverlaysRef,
      imageOverlaysRef,
      overlayCommitGuardRef,
      paramsRef,
      setLayers,
      setTextOverlays,
      setImageOverlays,
    ]
  );

  return {
    selectLayer,
    updateLayerOpacity,
    updateLayerRotation,
    updateLayerZIndex,
    toggleLayerVisibility,
    updateLayerMirror,
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
