// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Integration tests for the Monitor view compositor data flow.
 *
 * These tests verify that server-driven layout (useServerLayoutSync) and
 * config-driven state (the "sync from props" effect) interact correctly
 * in Monitor view.  They catch the class of bugs where:
 *
 *   - A server params echo-back overwrites server-resolved positions
 *   - Runtime measurement fields (measuredTextWidth) are lost on re-merge
 *   - Focus/selection changes cause layer positions to revert
 *   - Server view-data (geometry-only) overwrites client-authoritative fields
 *
 * Each test mounts the real useCompositorLayers hook with a sessionId
 * (Monitor view) and drives the Zustand session store to simulate server
 * view data arrivals, then exercises the "sync from props" path and
 * asserts that server state is preserved.
 */

import { renderHook, act } from '@testing-library/react';
import { describe, it, expect, vi, afterEach } from 'vitest';

import { writeNodeViewData, writeSessionConnected, clearSessionAtoms } from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type { CompositorLayout } from '@/types/generated/compositor-types';

import {
  getLayersFromStore,
  getTextOverlaysFromStore,
  getImageOverlaysFromStore,
} from './compositorAtoms';
import type { UseCompositorLayersOptions } from './useCompositorLayers';
import { useCompositorLayers } from './useCompositorLayers';

// ── Helpers ─────────────────────────────────────────────────────────────────

const SESSION_ID = 'test-session';
const NODE_ID = 'compositor';

/** Build a minimal params object with a single auto-layout layer (rect: null)
 *  and optionally a text overlay. */
function makeParams(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    width: 1280,
    height: 720,
    layers: {
      in_0: { opacity: 1.0, z_index: 0 },
    },
    text_overlays: [],
    image_overlays: [],
    ...overrides,
  };
}

/** Build a params object with a text overlay. */
function makeParamsWithText(
  textOverrides: Record<string, unknown> = {},
  paramOverrides: Record<string, unknown> = {}
): Record<string, unknown> {
  return makeParams({
    text_overlays: [
      {
        id: 'text_0',
        text: 'Hello',
        color: [255, 255, 255, 255],
        font_size: 32,
        font_name: 'dejavu-sans',
        rect: { x: 0, y: 0, width: 200, height: 40 },
        opacity: 1.0,
        rotation_degrees: 0,
        z_index: 100,
        mirror_horizontal: false,
        mirror_vertical: false,
        ...textOverrides,
      },
    ],
    ...paramOverrides,
  });
}

/** Build default hook options for Monitor view (has sessionId). */
function monitorOptions(
  overrides: Partial<UseCompositorLayersOptions> = {}
): UseCompositorLayersOptions {
  return {
    nodeId: NODE_ID,
    sessionId: SESSION_ID,
    canvasWidth: 1280,
    canvasHeight: 720,
    params: makeParams(),
    onConfigChange: vi.fn(),
    throttleMs: 100,
    ...overrides,
  };
}

/** Build a geometry-only ResolvedLayer (matches narrowed server struct). */
function serverLayer(
  id: string,
  overrides: Record<string, unknown> = {}
): CompositorLayout['layers'][number] {
  return {
    id,
    x: 160,
    y: 0,
    width: 960,
    height: 720,
    ...overrides,
  };
}

/** Build a geometry-only ResolvedOverlay (matches narrowed server struct). */
function serverOverlay(
  id: string,
  overrides: Record<string, unknown> = {}
): CompositorLayout['text_overlays'][number] {
  return {
    id,
    x: 0,
    y: 0,
    width: 200,
    height: 40,
    measured_text_width: null,
    measured_text_height: null,
    ...overrides,
  };
}

/** Build a CompositorLayout representing server-resolved positions. */
function makeServerLayout(overrides: Partial<CompositorLayout> = {}): CompositorLayout {
  return {
    canvas_width: 1280,
    canvas_height: 720,
    layers: [serverLayer('in_0')],
    text_overlays: [],
    image_overlays: [],
    ...overrides,
  };
}

/** Seed the Zustand store with a session so useServerLayoutSync finds it. */
function seedStore() {
  writeSessionConnected(SESSION_ID, true);
  useSessionStore.getState().initSession(SESSION_ID, true);
}

/** Push server view data into the store (simulates a WS view_data message). */
function pushServerViewData(layout: CompositorLayout) {
  writeNodeViewData(SESSION_ID, NODE_ID, layout);
  useSessionStore.getState().updateNodeViewData(SESSION_ID, NODE_ID, layout);
}

// ── Lifecycle ───────────────────────────────────────────────────────────────

afterEach(() => {
  // Clean up store between tests
  clearSessionAtoms(SESSION_ID);
  useSessionStore.getState().clearSession(SESSION_ID);
});

// ── Tests ───────────────────────────────────────────────────────────────────

describe('Monitor view data flow integration', () => {
  it('server-resolved layer positions survive a params echo-back', () => {
    seedStore();

    // Layer in_0 has rect: null → parseLayers defaults to full canvas (0,0,1280,720).
    // The server resolves it to an aspect-fit rect (160,0,960,720).
    const opts = monitorOptions();
    const { result, rerender } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Initial state: parsed from params (full canvas fallback)
    const layers0 = getLayersFromStore(result.current.store);
    expect(layers0[0].x).toBe(0);
    expect(layers0[0].width).toBe(1280);

    // Server view data arrives (geometry only)
    act(() => pushServerViewData(makeServerLayout()));

    // Server-resolved positions applied
    const layers1 = getLayersFromStore(result.current.store);
    expect(layers1[0].x).toBe(160);
    expect(layers1[0].width).toBe(960);
    expect(layers1[0].height).toBe(720);

    // Params echo-back: new reference, same content.
    // This triggers the "sync from props" effect.
    act(() => rerender({ ...opts, params: makeParams() }));

    // Server-resolved positions must survive the echo-back.
    const layers2 = getLayersFromStore(result.current.store);
    expect(layers2[0].x).toBe(160);
    expect(layers2[0].width).toBe(960);
    expect(layers2[0].height).toBe(720);
  });

  it('server text overlay measurements survive a params echo-back', () => {
    seedStore();

    const opts = monitorOptions({ params: makeParamsWithText() });
    const { result, rerender } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Initial text overlay from params
    const text0 = getTextOverlaysFromStore(result.current.store);
    expect(text0[0].id).toBe('text_0');
    expect(text0[0].measuredTextWidth).toBeUndefined();

    // Server sends layout with text measurements
    const layout = makeServerLayout({
      text_overlays: [
        serverOverlay('text_0', {
          x: 50,
          y: 100,
          width: 280,
          height: 45,
          measured_text_width: 275,
          measured_text_height: 42,
        }),
      ],
    });
    act(() => pushServerViewData(layout));

    // Measurements applied
    const text1 = getTextOverlaysFromStore(result.current.store);
    expect(text1[0].measuredTextWidth).toBe(275);
    expect(text1[0].measuredTextHeight).toBe(42);
    expect(text1[0].x).toBe(50);
    expect(text1[0].y).toBe(100);

    // Params echo-back
    act(() => rerender({ ...opts, params: makeParamsWithText() }));

    // Measurements and server positions must survive
    const text2 = getTextOverlaysFromStore(result.current.store);
    expect(text2[0].measuredTextWidth).toBe(275);
    expect(text2[0].measuredTextHeight).toBe(42);
    expect(text2[0].x).toBe(50);
    expect(text2[0].y).toBe(100);
  });

  it('config changes from params are picked up while preserving server geometry', () => {
    seedStore();

    const opts = monitorOptions({ params: makeParamsWithText() });
    const { result, rerender } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Server positions
    const layout = makeServerLayout({
      text_overlays: [
        serverOverlay('text_0', {
          x: 300,
          y: 200,
          width: 280,
          height: 45,
          measured_text_width: 275,
          measured_text_height: 42,
        }),
      ],
    });
    act(() => pushServerViewData(layout));

    const text0 = getTextOverlaysFromStore(result.current.store);
    expect(text0[0].text).toBe('Hello');
    expect(text0[0].x).toBe(300);

    // Params update with different text content (e.g. from another client)
    act(() =>
      rerender({
        ...opts,
        params: makeParamsWithText({ text: 'Updated text', font_size: 48 }),
      })
    );

    // Text content updated from params
    const text1 = getTextOverlaysFromStore(result.current.store);
    expect(text1[0].text).toBe('Updated text');
    expect(text1[0].fontSize).toBe(48);
    // Server position preserved
    expect(text1[0].x).toBe(300);
    expect(text1[0].y).toBe(200);
    // Measurements preserved
    expect(text1[0].measuredTextWidth).toBe(275);
  });

  it('server view-data does not overwrite client-side opacity/rotation', () => {
    seedStore();

    const opts = monitorOptions({
      params: makeParams({
        layers: {
          in_0: { opacity: 0.5, z_index: 3, rotation_degrees: 45 },
        },
      }),
    });
    const { result } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Client state has the config-driven values
    const layers0 = getLayersFromStore(result.current.store);
    expect(layers0[0].opacity).toBe(0.5);
    expect(layers0[0].zIndex).toBe(3);
    expect(layers0[0].rotationDegrees).toBe(45);

    // Server sends geometry-only view data (no opacity/rotation/z_index)
    act(() => pushServerViewData(makeServerLayout()));

    // Config-driven fields must be preserved — view data only updates geometry
    const layers1 = getLayersFromStore(result.current.store);
    expect(layers1[0].opacity).toBe(0.5);
    expect(layers1[0].zIndex).toBe(3);
    expect(layers1[0].rotationDegrees).toBe(45);
    // Geometry updated from server
    expect(layers1[0].x).toBe(160);
    expect(layers1[0].width).toBe(960);
  });

  it('opacity slider changes are never overwritten by server view-data', () => {
    seedStore();

    const onConfigChangeSilent = vi.fn();
    const opts = monitorOptions({
      onConfigChangeSilent,
      params: makeParams({
        layers: {
          in_0: { opacity: 0.8, z_index: 0 },
        },
      }),
    });
    const { result } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Server resolves initial layout (geometry only)
    act(() => pushServerViewData(makeServerLayout()));

    const layers0 = getLayersFromStore(result.current.store);
    expect(layers0[0].opacity).toBe(0.8);

    // User changes opacity locally via the slider
    act(() => result.current.updateLayerOpacity('in_0', 0.5));
    expect(onConfigChangeSilent).toHaveBeenCalled();

    // Local atom now has the user's value
    const layers1 = getLayersFromStore(result.current.store);
    expect(layers1[0].opacity).toBe(0.5);

    // Server sends another view-data update (geometry only — no opacity field).
    // This must NOT touch opacity since it's not in the payload.
    act(() => pushServerViewData(makeServerLayout()));

    const layers2 = getLayersFromStore(result.current.store);
    expect(layers2[0].opacity).toBe(0.5);
  });

  it('selecting different layers does not reset positions', () => {
    seedStore();

    const params = makeParams({
      layers: {
        in_0: { opacity: 1.0, z_index: 0 },
        in_1: {
          rect: { x: 0, y: 0, width: 320, height: 240 },
          opacity: 1.0,
          z_index: 1,
        },
      },
    });
    const opts = monitorOptions({ params });
    const { result } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Server resolves both layers at specific positions
    const layout = makeServerLayout({
      layers: [
        serverLayer('in_0'),
        serverLayer('in_1', { x: 800, y: 400, width: 320, height: 240 }),
      ],
    });
    act(() => pushServerViewData(layout));

    // Verify server positions applied
    const layers0 = getLayersFromStore(result.current.store);
    const layer0 = layers0.find((l) => l.id === 'in_0')!;
    const layer1 = layers0.find((l) => l.id === 'in_1')!;
    expect(layer0.x).toBe(160);
    expect(layer1.x).toBe(800);

    // Select layer 0
    act(() => result.current.selectLayer('in_0'));
    expect(result.current.selectedLayerId).toBe('in_0');

    // Switch to layer 1
    act(() => result.current.selectLayer('in_1'));
    expect(result.current.selectedLayerId).toBe('in_1');

    // Both layers should keep their server-resolved positions
    const layers1 = getLayersFromStore(result.current.store);
    const layer0After = layers1.find((l) => l.id === 'in_0')!;
    const layer1After = layers1.find((l) => l.id === 'in_1')!;
    expect(layer0After.x).toBe(160);
    expect(layer0After.width).toBe(960);
    expect(layer1After.x).toBe(800);
    expect(layer1After.width).toBe(320);
  });

  it('selecting different layers does not reset text overlay size/position', () => {
    seedStore();

    const params = makeParams({
      layers: {
        in_0: { opacity: 1.0, z_index: 0 },
      },
      text_overlays: [
        {
          id: 'text_0',
          text: 'Title',
          color: [255, 255, 255, 255],
          font_size: 32,
          font_name: 'dejavu-sans',
          rect: { x: 0, y: 0, width: 200, height: 40 },
          opacity: 1.0,
          rotation_degrees: 0,
          z_index: 100,
          mirror_horizontal: false,
          mirror_vertical: false,
        },
      ],
    });
    const opts = monitorOptions({ params });
    const { result } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Server resolves text overlay at a different position with measurements
    const layout = makeServerLayout({
      text_overlays: [
        serverOverlay('text_0', {
          x: 400,
          y: 300,
          width: 250,
          height: 48,
          measured_text_width: 245,
          measured_text_height: 44,
        }),
      ],
    });
    act(() => pushServerViewData(layout));

    // Verify server state
    const text0 = getTextOverlaysFromStore(result.current.store);
    expect(text0[0].x).toBe(400);
    expect(text0[0].y).toBe(300);
    expect(text0[0].measuredTextWidth).toBe(245);

    // Select the text overlay
    act(() => result.current.selectLayer('text_0'));
    expect(result.current.selectedLayerId).toBe('text_0');

    // Switch focus to video layer
    act(() => result.current.selectLayer('in_0'));
    expect(result.current.selectedLayerId).toBe('in_0');

    // Text overlay position and measurements must be unchanged
    const text1 = getTextOverlaysFromStore(result.current.store);
    expect(text1[0].x).toBe(400);
    expect(text1[0].y).toBe(300);
    expect(text1[0].width).toBe(250);
    expect(text1[0].measuredTextWidth).toBe(245);
    expect(text1[0].measuredTextHeight).toBe(44);
  });

  it('image overlay dataBase64 changes are picked up in Monitor view', () => {
    seedStore();

    const params = makeParams({
      image_overlays: [
        {
          id: 'img_0',
          data_base64: 'aW1hZ2UtZGF0YQ==', // "image-data"
          rect: { x: 10, y: 20, width: 100, height: 80 },
          opacity: 1.0,
          rotation_degrees: 0,
          z_index: 50,
          mirror_horizontal: false,
          mirror_vertical: false,
        },
      ],
    });
    const opts = monitorOptions({ params });
    const { result, rerender } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Server resolves image at a different position (geometry only)
    const layout = makeServerLayout({
      image_overlays: [
        serverOverlay('img_0', {
          x: 500,
          y: 300,
          width: 100,
          height: 80,
        }),
      ],
    });
    act(() => pushServerViewData(layout));

    const img0 = getImageOverlaysFromStore(result.current.store);
    expect(img0[0].x).toBe(500);
    expect(img0[0].y).toBe(300);
    expect(img0[0].dataBase64).toBe('aW1hZ2UtZGF0YQ==');
    // Config-driven opacity preserved (not in view data)
    expect(img0[0].opacity).toBe(1.0);

    // Another client changes the image data via params
    const updatedParams = makeParams({
      image_overlays: [
        {
          id: 'img_0',
          data_base64: 'bmV3LWltYWdl', // "new-image"
          rect: { x: 10, y: 20, width: 100, height: 80 },
          opacity: 1.0,
          rotation_degrees: 0,
          z_index: 50,
          mirror_horizontal: false,
          mirror_vertical: false,
        },
      ],
    });
    act(() => rerender({ ...opts, params: updatedParams }));

    // dataBase64 must be updated (config field)
    const img1 = getImageOverlaysFromStore(result.current.store);
    expect(img1[0].dataBase64).toBe('bmV3LWltYWdl');
    // Server-resolved position must be preserved
    expect(img1[0].x).toBe(500);
    expect(img1[0].y).toBe(300);
  });

  it('Design view (no sessionId) still uses parsed positions as source of truth', () => {
    // This is the control test — Design view should NOT preserve existing geometry.
    const opts: UseCompositorLayersOptions = {
      nodeId: NODE_ID,
      // No sessionId — Design view
      canvasWidth: 1280,
      canvasHeight: 720,
      params: makeParamsWithText(),
      onConfigChange: vi.fn(),
      throttleMs: 100,
    };

    const { result, rerender } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Text overlay at parsed position
    const text0 = getTextOverlaysFromStore(result.current.store);
    expect(text0[0].x).toBe(0);
    expect(text0[0].y).toBe(0);

    // Params update with new position
    act(() =>
      rerender({
        ...opts,
        params: makeParamsWithText({ rect: { x: 100, y: 200, width: 300, height: 50 } }),
      })
    );

    // Design view: position should update from params (not preserved)
    const text1 = getTextOverlaysFromStore(result.current.store);
    expect(text1[0].x).toBe(100);
    expect(text1[0].y).toBe(200);
  });
});
