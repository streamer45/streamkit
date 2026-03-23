// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Unit tests for the compositor commit adapter's causal-consistency stamping.
 *
 * Verifies that every commit path (commitLayers, commitOverlays, commitAll)
 * injects `_sender` and `_rev` into outgoing config/params, and that the
 * rev counter increments monotonically across calls.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

import { createCommitAdapter } from './compositorCommit';
import type { LayerState, TextOverlayState, ImageOverlayState } from './compositorLayerParsers';
import { resetAllConfigRevs } from './useConfigRev';

// Mock the WebSocket service to control the client nonce
vi.mock('@/services/websocket', () => ({
  getWebSocketService: () => ({
    getClientNonce: () => 'test-nonce-123',
  }),
}));

const NODE_ID = 'compositor';

function makeLayerState(id: string): LayerState {
  return {
    id,
    x: 0,
    y: 0,
    width: 640,
    height: 480,
    opacity: 1.0,
    zIndex: 0,
    rotationDegrees: 0,
    mirrorHorizontal: false,
    mirrorVertical: false,
    visible: true,
    cropX: 0.5,
    cropY: 0.5,
    cropZoom: 1.0,
    cropShape: 'rect',
  };
}

function makeRefs() {
  return {
    paramsRef: { current: { width: 1280, height: 720 } as Record<string, unknown> },
    layersRef: { current: [makeLayerState('in_0')] as LayerState[] },
    textOverlaysRef: { current: [] as TextOverlayState[] },
    imageOverlaysRef: { current: [] as ImageOverlayState[] },
  };
}

beforeEach(() => {
  resetAllConfigRevs();
});

describe('CommitAdapter causal-consistency stamping', () => {
  it('commitLayers via onConfigChange stamps _sender and _rev', () => {
    const onConfigChange = vi.fn();
    const refs = makeRefs();

    const adapter = createCommitAdapter(
      NODE_ID,
      onConfigChange,
      undefined,
      refs.paramsRef,
      refs.layersRef,
      refs.textOverlaysRef,
      refs.imageOverlaysRef
    )!;

    adapter.commitLayers([makeLayerState('in_0')]);

    expect(onConfigChange).toHaveBeenCalledTimes(1);
    const config = onConfigChange.mock.calls[0][1] as Record<string, unknown>;
    expect(config._sender).toBe('test-nonce-123');
    expect(config._rev).toBe(1);
  });

  it('commitLayers via onParamChange sends _sender and _rev as separate params', () => {
    const onParamChange = vi.fn();
    const refs = makeRefs();

    const adapter = createCommitAdapter(
      NODE_ID,
      undefined,
      onParamChange,
      refs.paramsRef,
      refs.layersRef,
      refs.textOverlaysRef,
      refs.imageOverlaysRef
    )!;

    adapter.commitLayers([makeLayerState('in_0')]);

    // Should have 3 calls: layers, _sender, _rev
    expect(onParamChange).toHaveBeenCalledTimes(3);
    expect(onParamChange.mock.calls[1][1]).toBe('_sender');
    expect(onParamChange.mock.calls[1][2]).toBe('test-nonce-123');
    expect(onParamChange.mock.calls[2][1]).toBe('_rev');
    expect(onParamChange.mock.calls[2][2]).toBe(1);
  });

  it('rev increments monotonically across multiple commits', () => {
    const onConfigChange = vi.fn();
    const refs = makeRefs();

    const adapter = createCommitAdapter(
      NODE_ID,
      onConfigChange,
      undefined,
      refs.paramsRef,
      refs.layersRef,
      refs.textOverlaysRef,
      refs.imageOverlaysRef
    )!;

    adapter.commitLayers([makeLayerState('in_0')]);
    adapter.commitOverlays([], []);
    adapter.commitAll([makeLayerState('in_0')], [], []);

    const rev1 = (onConfigChange.mock.calls[0][1] as Record<string, unknown>)._rev;
    const rev2 = (onConfigChange.mock.calls[1][1] as Record<string, unknown>)._rev;
    const rev3 = (onConfigChange.mock.calls[2][1] as Record<string, unknown>)._rev;

    expect(rev1).toBe(1);
    expect(rev2).toBe(2);
    expect(rev3).toBe(3);
  });

  it('commitOverlays via onParamChange sends _sender/_rev once for the batch', () => {
    const onParamChange = vi.fn();
    const refs = makeRefs();

    const adapter = createCommitAdapter(
      NODE_ID,
      undefined,
      onParamChange,
      refs.paramsRef,
      refs.layersRef,
      refs.textOverlaysRef,
      refs.imageOverlaysRef
    )!;

    adapter.commitOverlays([], []);

    // text_overlays, image_overlays, _sender, _rev
    expect(onParamChange).toHaveBeenCalledTimes(4);
    const calls = onParamChange.mock.calls;
    expect(calls[0][1]).toBe('text_overlays');
    expect(calls[1][1]).toBe('image_overlays');
    expect(calls[2][1]).toBe('_sender');
    expect(calls[3][1]).toBe('_rev');
  });

  it('returns null when both callbacks are undefined', () => {
    const refs = makeRefs();
    const adapter = createCommitAdapter(
      NODE_ID,
      undefined,
      undefined,
      refs.paramsRef,
      refs.layersRef,
      refs.textOverlaysRef,
      refs.imageOverlaysRef
    );
    expect(adapter).toBeNull();
  });
});
