// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Remote param synchronisation for the compositor.
 *
 * Subscribes to the per-node `nodeParamsAtom` in the default (provider-less)
 * Jotai store and merges config-driven fields (opacity, rotation, z_index,
 * mirror, crop, text content, image asset, etc.) into the compositor's
 * per-instance Jotai store.
 *
 * Geometry (x, y, width, height) is NOT taken from params — that is owned
 * by `useServerLayoutSync` which reads server-resolved positions from the
 * view-data atom.  Client-only fields (visible, measuredTextWidth/Height,
 * serverOnly) are preserved from existing state.
 *
 * Uses `defaultSessionStore.sub()` instead of `useAtomValue` so the
 * subscription doesn't trigger React re-renders at the CompositorNode
 * level.  Only the per-layer/overlay atoms that actually changed will
 * wake up their subscriber components (VideoLayer, OpacityControl, etc.).
 */

import { useEffect } from 'react';

import {
  sessionStore as defaultSessionStore,
  nodeParamsAtom,
  nodeKey,
} from '@/stores/sessionAtoms';

import type { CompositorStore } from './compositorAtoms';
import {
  getLayersFromStore,
  getTextOverlaysFromStore,
  getImageOverlaysFromStore,
  setLayersInStore,
  setTextOverlaysInStore,
  setImageOverlaysInStore,
} from './compositorAtoms';
import { parseLayers, parseTextOverlays, parseImageOverlays } from './compositorLayerParsers';
import type { LayerState, TextOverlayState, ImageOverlayState } from './compositorLayerParsers';

// ── Pure merge helpers ──────────────────────────────────────────────────────

export function mergeRemoteLayerParams(current: LayerState[], parsed: LayerState[]): LayerState[] {
  const merged: LayerState[] = parsed.map((p) => {
    const existing = current.find((l) => l.id === p.id);
    if (!existing) return p;
    return {
      ...p,
      x: existing.x,
      y: existing.y,
      width: existing.width,
      height: existing.height,
      visible: existing.visible,
      opacity: existing.visible ? p.opacity : existing.opacity,
      serverOnly: existing.serverOnly,
    };
  });

  const serverOnly = current.filter((l) => l.serverOnly && !parsed.some((p) => p.id === l.id));
  if (serverOnly.length > 0) merged.push(...serverOnly);

  return merged;
}

export function mergeRemoteTextParams(
  current: TextOverlayState[],
  parsed: TextOverlayState[]
): TextOverlayState[] {
  return parsed.map((p) => {
    const existing = current.find((o) => o.id === p.id);
    if (!existing) return p;
    return {
      ...p,
      x: existing.x,
      y: existing.y,
      width: existing.width,
      height: existing.height,
      visible: existing.visible,
      opacity: existing.visible ? p.opacity : existing.opacity,
      measuredTextWidth: existing.measuredTextWidth,
      measuredTextHeight: existing.measuredTextHeight,
    };
  });
}

export function mergeRemoteImageParams(
  current: ImageOverlayState[],
  parsed: ImageOverlayState[]
): ImageOverlayState[] {
  return parsed.map((p) => {
    const existing = current.find((o) => o.id === p.id);
    if (!existing) return p;
    return {
      ...p,
      x: existing.x,
      y: existing.y,
      width: existing.width,
      height: existing.height,
      visible: existing.visible,
      opacity: existing.visible ? p.opacity : existing.opacity,
    };
  });
}

// ── Hook ────────────────────────────────────────────────────────────────────

export function useParamAtomSync(
  sessionId: string | undefined,
  nodeId: string,
  store: CompositorStore,
  canvasWidth: number,
  canvasHeight: number,
  dragStateRef: React.MutableRefObject<unknown>,
  activeInteractionRef?: React.MutableRefObject<boolean>
): void {
  useEffect(() => {
    if (!sessionId) return;

    const applyRemoteParams = (params: Record<string, unknown>) => {
      if (dragStateRef.current) return;
      if (activeInteractionRef?.current) return;
      if (!params || Object.keys(params).length === 0) return;

      const w = (params.width as number) ?? canvasWidth;
      const h = (params.height as number) ?? canvasHeight;

      const parsed = parseLayers(params, w, h);
      setLayersInStore(store, mergeRemoteLayerParams(getLayersFromStore(store), parsed));

      const parsedText = parseTextOverlays(params);
      setTextOverlaysInStore(
        store,
        mergeRemoteTextParams(getTextOverlaysFromStore(store), parsedText)
      );

      const parsedImg = parseImageOverlays(params);
      setImageOverlaysInStore(
        store,
        mergeRemoteImageParams(getImageOverlaysFromStore(store), parsedImg)
      );
    };

    const paramsAtom = nodeParamsAtom(nodeKey(sessionId, nodeId));
    const current = defaultSessionStore.get(paramsAtom);
    applyRemoteParams(current);

    const unsub = defaultSessionStore.sub(paramsAtom, () => {
      applyRemoteParams(defaultSessionStore.get(paramsAtom));
    });
    return unsub;
  }, [sessionId, nodeId, store, canvasWidth, canvasHeight, dragStateRef, activeInteractionRef]);
}
