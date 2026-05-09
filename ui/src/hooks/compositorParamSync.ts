// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Remote param sync for the compositor — subscribes to `nodeParamsAtom` in
 * the default Jotai store via `defaultSessionStore.sub()` (non-React) and
 * merges config fields into the per-instance compositor store.
 *
 * Geometry and client-only fields (visible, serverOnly, measuredText*)
 * are preserved — see `compositorServerSync` for geometry ownership.
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

// Unlike mergeOverlayState (props path), these skip pickChangedConfigFields
// because atom writes are already deduplicated by the WS handler's rev check.

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
      // visible is client-only — hidden layers keep stored opacity
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
      // visible is client-only — hidden overlays keep stored opacity
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
      // visible is client-only — hidden overlays keep stored opacity
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
