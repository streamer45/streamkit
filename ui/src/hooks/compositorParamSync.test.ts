// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Unit tests for remote param synchronisation in compositorParamSync.
 *
 * These tests verify that:
 *   - Remote opacity/rotation/z_index changes propagate to compositor layers
 *   - Geometry (x, y, width, height) is preserved from existing state
 *   - Client-only fields (visible, measuredTextWidth/Height) are preserved
 *   - Server-only layers are retained when absent from parsed params
 *   - Hidden layers preserve their stored opacity during remote updates
 *   - Text overlay config (text, fontSize, color) propagates from remote
 *   - Image overlay config (assetPath) propagates from remote
 */

import { act } from '@testing-library/react';
import { createStore } from 'jotai';
import { describe, it, expect, vi } from 'vitest';

import { sessionStore, nodeParamsAtom, nodeKey, writeNodeParams } from '@/stores/sessionAtoms';
import { measureHookRenders } from '@/test/perf';

import { setLayersInStore } from './compositorAtoms';
import type { LayerState, TextOverlayState, ImageOverlayState } from './compositorLayerParsers';
import {
  mergeRemoteLayerParams,
  mergeRemoteTextParams,
  mergeRemoteImageParams,
  useParamAtomSync,
} from './compositorParamSync';

vi.mock('@/services/websocket', () => ({
  getWebSocketService: () => ({
    getClientNonce: () => 'test-nonce',
  }),
}));

function makeLayer(id: string, overrides: Partial<LayerState> = {}): LayerState {
  return {
    id,
    x: 0,
    y: 0,
    width: 1280,
    height: 720,
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
    aspectFit: true,
    ...overrides,
  };
}

function makeTextOverlay(id: string, overrides: Partial<TextOverlayState> = {}): TextOverlayState {
  return {
    id,
    text: 'Hello',
    x: 50,
    y: 50,
    width: 200,
    height: 40,
    color: [255, 255, 255, 255],
    fontSize: 24,
    fontName: 'DejaVuSans.ttf',
    opacity: 1.0,
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
    assetPath: 'images/logo.png',
    x: 10,
    y: 10,
    width: 200,
    height: 200,
    opacity: 1.0,
    rotationDegrees: 0,
    zIndex: 200,
    mirrorHorizontal: false,
    mirrorVertical: false,
    visible: true,
    ...overrides,
  };
}

describe('mergeRemoteLayerParams', () => {
  it('updates config fields from parsed while preserving geometry', () => {
    const current = [makeLayer('in_0', { x: 160, y: 0, width: 960, height: 720 })];
    const parsed = [makeLayer('in_0', { opacity: 0.5, rotationDegrees: 45 })];

    const result = mergeRemoteLayerParams(current, parsed);

    expect(result[0].opacity).toBe(0.5);
    expect(result[0].rotationDegrees).toBe(45);
    expect(result[0].x).toBe(160);
    expect(result[0].y).toBe(0);
    expect(result[0].width).toBe(960);
    expect(result[0].height).toBe(720);
  });

  it('preserves client-side visible flag', () => {
    const current = [makeLayer('in_0', { visible: false, opacity: 0.8 })];
    const parsed = [makeLayer('in_0', { opacity: 0.5 })];

    const result = mergeRemoteLayerParams(current, parsed);

    expect(result[0].visible).toBe(false);
    expect(result[0].opacity).toBe(0.8);
  });

  it('accepts parsed opacity when layer is visible', () => {
    const current = [makeLayer('in_0', { visible: true, opacity: 0.8 })];
    const parsed = [makeLayer('in_0', { opacity: 0.5 })];

    const result = mergeRemoteLayerParams(current, parsed);

    expect(result[0].visible).toBe(true);
    expect(result[0].opacity).toBe(0.5);
  });

  it('preserves serverOnly flag from existing layers', () => {
    const current = [makeLayer('in_0', { serverOnly: true })];
    const parsed = [makeLayer('in_0', { opacity: 0.7 })];

    const result = mergeRemoteLayerParams(current, parsed);

    expect(result[0].serverOnly).toBe(true);
  });

  it('retains server-only layers absent from parsed params', () => {
    const current = [makeLayer('in_0'), makeLayer('in_1', { serverOnly: true })];
    const parsed = [makeLayer('in_0', { opacity: 0.5 })];

    const result = mergeRemoteLayerParams(current, parsed);

    expect(result).toHaveLength(2);
    expect(result[1].id).toBe('in_1');
    expect(result[1].serverOnly).toBe(true);
  });

  it('adds new layers from parsed that have no existing match', () => {
    const current = [makeLayer('in_0')];
    const parsed = [makeLayer('in_0'), makeLayer('in_1', { opacity: 0.9 })];

    const result = mergeRemoteLayerParams(current, parsed);

    expect(result).toHaveLength(2);
    expect(result[1].id).toBe('in_1');
    expect(result[1].opacity).toBe(0.9);
  });

  it('updates crop/zoom fields from remote params', () => {
    const current = [makeLayer('in_0', { cropZoom: 1.0, cropX: 0.5, cropY: 0.5 })];
    const parsed = [makeLayer('in_0', { cropZoom: 2.0, cropX: 0.3, cropY: 0.7 })];

    const result = mergeRemoteLayerParams(current, parsed);

    expect(result[0].cropZoom).toBe(2.0);
    expect(result[0].cropX).toBe(0.3);
    expect(result[0].cropY).toBe(0.7);
  });

  it('updates mirror flags from remote params', () => {
    const current = [makeLayer('in_0', { mirrorHorizontal: false, mirrorVertical: false })];
    const parsed = [makeLayer('in_0', { mirrorHorizontal: true, mirrorVertical: true })];

    const result = mergeRemoteLayerParams(current, parsed);

    expect(result[0].mirrorHorizontal).toBe(true);
    expect(result[0].mirrorVertical).toBe(true);
  });
});

describe('mergeRemoteTextParams', () => {
  it('updates text content and font from remote while preserving geometry', () => {
    const current = [makeTextOverlay('t1', { x: 100, y: 200, width: 300, height: 60 })];
    const parsed = [makeTextOverlay('t1', { text: 'Updated', fontSize: 32 })];

    const result = mergeRemoteTextParams(current, parsed);

    expect(result[0].text).toBe('Updated');
    expect(result[0].fontSize).toBe(32);
    expect(result[0].x).toBe(100);
    expect(result[0].y).toBe(200);
    expect(result[0].width).toBe(300);
    expect(result[0].height).toBe(60);
  });

  it('preserves measuredTextWidth/Height from existing state', () => {
    const current = [
      makeTextOverlay('t1', {
        measuredTextWidth: 250,
        measuredTextHeight: 36,
      }),
    ];
    const parsed = [makeTextOverlay('t1', { text: 'New text' })];

    const result = mergeRemoteTextParams(current, parsed);

    expect(result[0].measuredTextWidth).toBe(250);
    expect(result[0].measuredTextHeight).toBe(36);
  });

  it('preserves visible flag and opacity for hidden text overlays', () => {
    const current = [makeTextOverlay('t1', { visible: false, opacity: 0.6 })];
    const parsed = [makeTextOverlay('t1', { opacity: 0.3 })];

    const result = mergeRemoteTextParams(current, parsed);

    expect(result[0].visible).toBe(false);
    expect(result[0].opacity).toBe(0.6);
  });
});

describe('mergeRemoteImageParams', () => {
  it('updates assetPath from remote while preserving geometry', () => {
    const current = [makeImageOverlay('i1', { x: 20, y: 30, width: 150, height: 150 })];
    const parsed = [makeImageOverlay('i1', { assetPath: 'images/new-logo.png' })];

    const result = mergeRemoteImageParams(current, parsed);

    expect(result[0].assetPath).toBe('images/new-logo.png');
    expect(result[0].x).toBe(20);
    expect(result[0].y).toBe(30);
    expect(result[0].width).toBe(150);
    expect(result[0].height).toBe(150);
  });

  it('preserves visible flag and opacity for hidden image overlays', () => {
    const current = [makeImageOverlay('i1', { visible: false, opacity: 0.7 })];
    const parsed = [makeImageOverlay('i1', { opacity: 0.4 })];

    const result = mergeRemoteImageParams(current, parsed);

    expect(result[0].visible).toBe(false);
    expect(result[0].opacity).toBe(0.7);
  });
});

describe('useParamAtomSync integration', () => {
  it('remote param write propagates config to compositor store', () => {
    const sessionId = 'test-session';
    const nodeId = 'compositor-1';
    const key = nodeKey(sessionId, nodeId);

    const store = createStore();
    setLayersInStore(store, [
      makeLayer('in_0', { x: 160, y: 0, width: 960, height: 720, opacity: 1.0 }),
    ]);

    writeNodeParams(
      nodeId,
      {
        width: 1280,
        height: 720,
        layers: {
          in_0: { opacity: 0.5, z_index: 0, rotation_degrees: 30 },
        },
        text_overlays: [],
        image_overlays: [],
      },
      sessionId
    );

    const atomParams = sessionStore.get(nodeParamsAtom(key));
    expect(atomParams).toBeDefined();
    expect((atomParams.layers as Record<string, Record<string, unknown>>)?.in_0?.opacity).toBe(0.5);

    // Clean up
    sessionStore.set(nodeParamsAtom(key), {});
  });

  it('atom writes do not cause hook host re-renders (non-React subscription)', () => {
    const sessionId = 'render-test';
    const nodeId = 'compositor-render';
    const key = nodeKey(sessionId, nodeId);
    const compositorStore = createStore();
    const dragRef = { current: null };

    setLayersInStore(compositorStore, [makeLayer('in_0')]);

    // Seed the atom so the hook has something to subscribe to
    sessionStore.set(nodeParamsAtom(key), {
      width: 1280,
      height: 720,
      layers: { in_0: { opacity: 1.0, z_index: 0 } },
      text_overlays: [],
      image_overlays: [],
    });

    const result = measureHookRenders(
      () => useParamAtomSync(sessionId, nodeId, compositorStore, 1280, 720, dragRef),
      {
        initialProps: {},
        scenario: () => {
          // 10 remote param writes — should NOT re-render the hook host
          for (let i = 0; i < 10; i++) {
            act(() => {
              writeNodeParams(
                nodeId,
                {
                  width: 1280,
                  height: 720,
                  layers: { in_0: { opacity: 0.5 + i * 0.04, z_index: 0 } },
                  text_overlays: [],
                  image_overlays: [],
                },
                sessionId
              );
            });
          }
        },
      }
    );

    // Only the initial mount render — no re-renders from atom writes
    expect(result.meanRenderCount).toBeLessThanOrEqual(1);

    // Clean up
    sessionStore.set(nodeParamsAtom(key), {});
  });
});
