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

import { throttle } from 'lodash-es';
import { useEffect, useMemo } from 'react';

import {
  buildConfig,
  serializeImageOverlays,
  serializeLayers,
  serializeTextOverlays,
} from './compositorLayerParsers';
import type { LayerState, TextOverlayState, ImageOverlayState } from './compositorLayerParsers';
import { bumpConfigRev, getClientNonce } from './useConfigRev';

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

  /** Stamp a config object with causal-consistency metadata. */
  function stamp(config: Record<string, unknown>): Record<string, unknown> {
    const rev = bumpConfigRev(nodeId);
    return { ...config, _sender: getClientNonce(), _rev: rev };
  }

  // NOTE: The onParamChange path (tuneNode) sends each key as a separate
  // UpdateParams WS message that replaces the server's full node.params.
  // Sending _sender/_rev as standalone messages would wipe durable params
  // to {} after stripping.  Only the onConfigChange path (tuneNodeConfig)
  // can safely carry stamped metadata because it sends the full config
  // object in a single message.

  return {
    commitLayers(layers: LayerState[]) {
      if (onConfigChange) {
        const config = stamp(
          buildConfig(paramsRef.current, layers, textOverlaysRef.current, imageOverlaysRef.current)
        );
        onConfigChange(nodeId, config);
      } else if (onParamChange) {
        onParamChange(nodeId, 'layers', serializeLayers(layers));
      }
    },

    commitOverlays(text: TextOverlayState[], img: ImageOverlayState[]) {
      if (onConfigChange) {
        const config = stamp(buildConfig(paramsRef.current, layersRef.current, text, img));
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
        const config = stamp(buildConfig(paramsRef.current, layers, text, img));
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

export interface UseCompositorCommitOptions {
  nodeId: string;
  onConfigChange?: (nodeId: string, config: Record<string, unknown>) => void;
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
}

/** Hook that creates a CommitAdapter, throttled commit helpers, and
 *  cancels pending throttled calls on unmount. */
export function useCompositorCommit(opts: UseCompositorCommitOptions): UseCompositorCommitResult {
  const {
    nodeId,
    onConfigChange,
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

  const throttledConfigChange = useMemo(() => {
    const adapter = commitAdapter;
    if (!adapter) return null;
    return throttle(
      (currentLayers: LayerState[]) => {
        adapter.commitLayers(currentLayers);
      },
      throttleMs,
      { leading: true, trailing: true }
    );
  }, [commitAdapter, throttleMs]);

  const throttledOverlayCommit = useMemo(() => {
    const adapter = commitAdapter;
    if (!adapter) return null;
    return throttle(
      (nextText: TextOverlayState[], nextImg: ImageOverlayState[]) => {
        adapter.commitOverlays(nextText, nextImg);
      },
      throttleMs,
      { leading: true, trailing: true }
    );
  }, [commitAdapter, throttleMs]);

  useEffect(
    () => () => {
      throttledConfigChange?.cancel();
      throttledOverlayCommit?.cancel();
    },
    [throttledConfigChange, throttledOverlayCommit]
  );

  return { commitAdapter, throttledConfigChange, throttledOverlayCommit };
}
