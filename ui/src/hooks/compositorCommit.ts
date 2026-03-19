// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Commit / persistence logic for the compositor.
 *
 * Encapsulates the dual commit path (onConfigChange vs onParamChange) into a
 * single `CommitAdapter`, and provides a `useCompositorCommit` hook that wraps
 * the throttled commit helpers and their cleanup effect.
 */

import { debounce, throttle } from 'lodash-es';
import { useEffect, useMemo, useRef } from 'react';

import {
  buildConfig,
  serializeImageOverlays,
  serializeLayers,
  serializeTextOverlays,
} from './compositorLayerParsers';
import type { LayerState, TextOverlayState, ImageOverlayState } from './compositorLayerParsers';

// ── Commit adapter ──────────────────────────────────────────────────────────

/** Unified commit interface that hides the onConfigChange / onParamChange branching. */
export interface CommitAdapter {
  /** Persist video layer changes. */
  commitLayers: (layers: LayerState[]) => void;
  /** Persist overlay (text + image) changes. */
  commitOverlays: (text: TextOverlayState[], img: ImageOverlayState[]) => void;
  /** Persist all layers and overlays in a single commit.
   *  Optional `changed` flags control which params are sent via onParamChange
   *  (onConfigChange always sends the full config regardless). */
  commitAll: (
    layers: LayerState[],
    text: TextOverlayState[],
    img: ImageOverlayState[],
    changed?: { layers?: boolean; overlays?: boolean }
  ) => void;
}

/** Create a CommitAdapter that routes commits through the appropriate callback.
 *
 *  - Design view provides `onConfigChange` which sends the full config object.
 *  - Monitor view provides `onParamChange` which sends individual params.
 *
 *  The adapter reads current values from the provided refs so callers only need
 *  to supply the data that actually changed. */
export function createCommitAdapter(
  nodeId: string,
  onConfigChange: ((nodeId: string, config: Record<string, unknown>) => void) | undefined,
  onParamChange: ((nodeId: string, key: string, value: unknown) => void) | undefined,
  paramsRef: React.MutableRefObject<Record<string, unknown>>,
  layersRef: React.MutableRefObject<LayerState[]>,
  textOverlaysRef: React.MutableRefObject<TextOverlayState[]>,
  imageOverlaysRef: React.MutableRefObject<ImageOverlayState[]>
): CommitAdapter | null {
  if (!onConfigChange && !onParamChange) return null;

  return {
    commitLayers(layers: LayerState[]) {
      if (onConfigChange) {
        const config = buildConfig(
          paramsRef.current,
          layers,
          textOverlaysRef.current,
          imageOverlaysRef.current
        );
        onConfigChange(nodeId, config);
      } else if (onParamChange) {
        onParamChange(nodeId, 'layers', serializeLayers(layers));
      }
    },

    commitOverlays(text: TextOverlayState[], img: ImageOverlayState[]) {
      if (onConfigChange) {
        const config = buildConfig(paramsRef.current, layersRef.current, text, img);
        onConfigChange(nodeId, config);
      } else if (onParamChange) {
        onParamChange(nodeId, 'text_overlays', serializeTextOverlays(text));
        onParamChange(nodeId, 'image_overlays', serializeImageOverlays(img));
      }
    },

    commitAll(
      layers: LayerState[],
      text: TextOverlayState[],
      img: ImageOverlayState[],
      changed?: { layers?: boolean; overlays?: boolean }
    ) {
      if (onConfigChange) {
        const config = buildConfig(paramsRef.current, layers, text, img);
        onConfigChange(nodeId, config);
      } else if (onParamChange) {
        const sendLayers = changed?.layers ?? true;
        const sendOverlays = changed?.overlays ?? true;
        if (sendLayers) {
          onParamChange(nodeId, 'layers', serializeLayers(layers));
        }
        if (sendOverlays) {
          onParamChange(nodeId, 'text_overlays', serializeTextOverlays(text));
          onParamChange(nodeId, 'image_overlays', serializeImageOverlays(img));
        }
      }
    },
  };
}

// ── Hook ────────────────────────────────────────────────────────────────────

export interface UseCompositorCommitOptions {
  nodeId: string;
  onConfigChange?: (nodeId: string, config: Record<string, unknown>) => void;
  /** Silent config change: broadcasts to other clients only (no echo-back). */
  onConfigChangeSilent?: (nodeId: string, config: Record<string, unknown>) => void;
  onParamChange?: (nodeId: string, key: string, value: unknown) => void;
  throttleMs: number;
  paramsRef: React.MutableRefObject<Record<string, unknown>>;
  layersRef: React.MutableRefObject<LayerState[]>;
  textOverlaysRef: React.MutableRefObject<TextOverlayState[]>;
  imageOverlaysRef: React.MutableRefObject<ImageOverlayState[]>;
}

export interface UseCompositorCommitResult {
  /** Adapter for immediate (non-throttled) commits. */
  commitAdapter: CommitAdapter | null;
  /** Throttled commit for video layer changes during drag/resize. */
  throttledConfigChange: ((layers: LayerState[]) => void) | null;
  /** Throttled commit for overlay property changes (e.g. slider drags). */
  throttledOverlayCommit: ((text: TextOverlayState[], img: ImageOverlayState[]) => void) | null;
  /** Ref that is `true` while a throttled silent send is in-flight.
   *  Used by sync guards to skip view-data echo-backs during slider drags
   *  (NodeViewDataUpdated is NOT suppressed by TuneNodeSilent). */
  throttleActiveRef: React.MutableRefObject<boolean>;
}

/** Hook that creates a CommitAdapter, throttled commit helpers, and
 *  cancels pending throttled calls on unmount. */
export function useCompositorCommit(opts: UseCompositorCommitOptions): UseCompositorCommitResult {
  const {
    nodeId,
    onConfigChange,
    onConfigChangeSilent,
    onParamChange,
    throttleMs,
    paramsRef,
    layersRef,
    textOverlaysRef,
    imageOverlaysRef,
  } = opts;

  const commitAdapter = useMemo(
    () =>
      createCommitAdapter(
        nodeId,
        onConfigChange,
        onParamChange,
        paramsRef,
        layersRef,
        textOverlaysRef,
        imageOverlaysRef
      ),
    [nodeId, onConfigChange, onParamChange, paramsRef, layersRef, textOverlaysRef, imageOverlaysRef]
  );

  // Silent commit adapter: used by throttled sends during slider drags.
  // Routes through onConfigChangeSilent so the server skips echo-back.
  const silentCommitAdapter = useMemo(
    () =>
      createCommitAdapter(
        nodeId,
        onConfigChangeSilent,
        onParamChange,
        paramsRef,
        layersRef,
        textOverlaysRef,
        imageOverlaysRef
      ),
    [
      nodeId,
      onConfigChangeSilent,
      onParamChange,
      paramsRef,
      layersRef,
      textOverlaysRef,
      imageOverlaysRef,
    ]
  );

  // Ref that is `true` while a throttled silent send is in-flight.
  // Used by sync guards to skip view-data echo-backs during slider drags
  // (NodeViewDataUpdated is NOT suppressed by TuneNodeSilent).
  const throttleActiveRef = useRef(false);

  // Debounced cleanup: clears the flag shortly after the last throttled tick.
  // The delay matches throttleMs so the flag stays true for the full throttle
  // window and is cleared once no more ticks arrive.
  // NOTE: there is a small theoretical window where the flag clears just before
  // the server finishes processing the last throttled message and broadcasts
  // NodeViewDataUpdated.  In practice the server processes tune messages in
  // sub-millisecond time and the debounce window is ~100ms, so this is
  // extremely unlikely to manifest.  If it does, increase the delay (e.g.
  // throttleMs * 1.5).
  const clearThrottleActive = useMemo(
    () =>
      debounce(() => {
        throttleActiveRef.current = false;
      }, throttleMs),
    [throttleMs]
  );

  const throttledConfigChange = useMemo(() => {
    // Throttled sends use the silent adapter (no echo-back) when available,
    // falling back to the normal adapter for Design view.
    const adapter = silentCommitAdapter ?? commitAdapter;
    if (!adapter) return null;
    return throttle(
      (currentLayers: LayerState[]) => {
        throttleActiveRef.current = true;
        clearThrottleActive();
        adapter.commitLayers(currentLayers);
      },
      throttleMs,
      { leading: true, trailing: true }
    );
  }, [silentCommitAdapter, commitAdapter, throttleMs, clearThrottleActive]);

  const throttledOverlayCommit = useMemo(() => {
    // Throttled sends use the silent adapter (no echo-back) when available.
    const adapter = silentCommitAdapter ?? commitAdapter;
    if (!adapter) return null;
    return throttle(
      (nextText: TextOverlayState[], nextImg: ImageOverlayState[]) => {
        throttleActiveRef.current = true;
        clearThrottleActive();
        adapter.commitOverlays(nextText, nextImg);
      },
      throttleMs,
      { leading: true, trailing: true }
    );
  }, [silentCommitAdapter, commitAdapter, throttleMs, clearThrottleActive]);

  useEffect(
    () => () => {
      throttledConfigChange?.cancel();
      throttledOverlayCommit?.cancel();
      clearThrottleActive.cancel();
    },
    [throttledConfigChange, throttledOverlayCommit, clearThrottleActive]
  );

  return { commitAdapter, throttledConfigChange, throttledOverlayCommit, throttleActiveRef };
}
