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
 *
 * Each test mounts the real useCompositorLayers hook with a sessionId
 * (Monitor view) and drives the Zustand session store to simulate server
 * view data arrivals, then exercises the "sync from props" path and
 * asserts that server state is preserved.
 */

import { renderHook, act } from '@testing-library/react';
import { describe, it, expect, vi, afterEach } from 'vitest';

import { useSessionStore } from '@/stores/sessionStore';
import type { CompositorLayout } from '@/types/generated/compositor-types';

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

/** Build a CompositorLayout representing server-resolved positions. */
function makeServerLayout(overrides: Partial<CompositorLayout> = {}): CompositorLayout {
  return {
    canvas_width: 1280,
    canvas_height: 720,
    layers: [
      {
        id: 'in_0',
        x: 160,
        y: 0,
        width: 960,
        height: 720,
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.5,
        crop_y: 0.5,
      },
    ],
    text_overlays: [],
    image_overlays: [],
    ...overrides,
  };
}

/** Seed the Zustand store with a session so useServerLayoutSync finds it. */
function seedStore() {
  useSessionStore.getState().initSession(SESSION_ID, true);
}

/** Push server view data into the store (simulates a WS view_data message). */
function pushServerViewData(layout: CompositorLayout) {
  useSessionStore.getState().updateNodeViewData(SESSION_ID, NODE_ID, layout);
}

// ── Lifecycle ───────────────────────────────────────────────────────────────

afterEach(() => {
  // Clean up store between tests
  useSessionStore.getState().clearSession(SESSION_ID);
});

// ── Tests ───────────────────────────────────────────────────────────────────

describe('Monitor view data flow integration', () => {
  it('layers are populated synchronously on initial render (no empty-state flash)', () => {
    seedStore();
    const opts = monitorOptions();
    // renderHook returns the first committed result — layers must be parsed
    // from params immediately (useLayoutEffect), not deferred to a post-paint
    // useEffect.  If this assertion fails, the sync-from-props effect was
    // changed back to useEffect and the compositor will flash "No layers".
    const { result } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    expect(result.current.layers).toHaveLength(1);
    expect(result.current.layers[0].id).toBe('in_0');
    expect(result.current.textOverlays).toHaveLength(0);
    expect(result.current.imageOverlays).toHaveLength(0);
  });

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
    expect(result.current.layers[0].x).toBe(0);
    expect(result.current.layers[0].width).toBe(1280);

    // Server view data arrives
    act(() => pushServerViewData(makeServerLayout()));

    // Server-resolved positions applied
    expect(result.current.layers[0].x).toBe(160);
    expect(result.current.layers[0].width).toBe(960);
    expect(result.current.layers[0].height).toBe(720);

    // Params echo-back: new reference, same content.
    // This triggers the "sync from props" effect.
    act(() => rerender({ ...opts, params: makeParams() }));

    // Server-resolved positions must survive the echo-back.
    expect(result.current.layers[0].x).toBe(160);
    expect(result.current.layers[0].width).toBe(960);
    expect(result.current.layers[0].height).toBe(720);
  });

  it('server text overlay measurements survive a params echo-back', () => {
    seedStore();

    const opts = monitorOptions({ params: makeParamsWithText() });
    const { result, rerender } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Initial text overlay from params
    expect(result.current.textOverlays[0].id).toBe('text_0');
    expect(result.current.textOverlays[0].measuredTextWidth).toBeUndefined();

    // Server sends layout with text measurements
    const layout = makeServerLayout({
      text_overlays: [
        {
          id: 'text_0',
          x: 50,
          y: 100,
          width: 280,
          height: 45,
          opacity: 1.0,
          z_index: 100,
          rotation_degrees: 0,
          mirror_horizontal: false,
          mirror_vertical: false,
          measured_text_width: 275,
          measured_text_height: 42,
        },
      ],
    });
    act(() => pushServerViewData(layout));

    // Measurements applied
    expect(result.current.textOverlays[0].measuredTextWidth).toBe(275);
    expect(result.current.textOverlays[0].measuredTextHeight).toBe(42);
    expect(result.current.textOverlays[0].x).toBe(50);
    expect(result.current.textOverlays[0].y).toBe(100);

    // Params echo-back
    act(() => rerender({ ...opts, params: makeParamsWithText() }));

    // Measurements and server positions must survive
    expect(result.current.textOverlays[0].measuredTextWidth).toBe(275);
    expect(result.current.textOverlays[0].measuredTextHeight).toBe(42);
    expect(result.current.textOverlays[0].x).toBe(50);
    expect(result.current.textOverlays[0].y).toBe(100);
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
        {
          id: 'text_0',
          x: 300,
          y: 200,
          width: 280,
          height: 45,
          opacity: 1.0,
          z_index: 100,
          rotation_degrees: 0,
          mirror_horizontal: false,
          mirror_vertical: false,
          measured_text_width: 275,
          measured_text_height: 42,
        },
      ],
    });
    act(() => pushServerViewData(layout));

    expect(result.current.textOverlays[0].text).toBe('Hello');
    expect(result.current.textOverlays[0].x).toBe(300);

    // Params update with different text content (e.g. from another client)
    act(() =>
      rerender({
        ...opts,
        params: makeParamsWithText({ text: 'Updated text', font_size: 48 }),
      })
    );

    // Text content updated from params
    expect(result.current.textOverlays[0].text).toBe('Updated text');
    expect(result.current.textOverlays[0].fontSize).toBe(48);
    // Server position preserved
    expect(result.current.textOverlays[0].x).toBe(300);
    expect(result.current.textOverlays[0].y).toBe(200);
    // Measurements preserved
    expect(result.current.textOverlays[0].measuredTextWidth).toBe(275);
  });

  it('server-resolved opacity/rotation/zIndex survive params echo-back', () => {
    seedStore();

    const opts = monitorOptions();
    const { result, rerender } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Server resolves layer with specific opacity/rotation/z-index
    const layout = makeServerLayout();
    layout.layers[0].opacity = 0.75;
    layout.layers[0].rotation_degrees = 45;
    layout.layers[0].z_index = 5;
    act(() => pushServerViewData(layout));

    expect(result.current.layers[0].opacity).toBe(0.75);
    expect(result.current.layers[0].rotationDegrees).toBe(45);
    expect(result.current.layers[0].zIndex).toBe(5);

    // Params echo-back (params still have default opacity=1, rotation=0, z_index=0)
    act(() => rerender({ ...opts, params: makeParams() }));

    // Server-resolved values must survive
    expect(result.current.layers[0].opacity).toBe(0.75);
    expect(result.current.layers[0].rotationDegrees).toBe(45);
    expect(result.current.layers[0].zIndex).toBe(5);
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
        {
          id: 'in_0',
          x: 160,
          y: 0,
          width: 960,
          height: 720,
          opacity: 1.0,
          z_index: 0,
          rotation_degrees: 0,
          mirror_horizontal: false,
          mirror_vertical: false,
          crop_zoom: 1.0,
          crop_x: 0.5,
          crop_y: 0.5,
        },
        {
          id: 'in_1',
          x: 800,
          y: 400,
          width: 320,
          height: 240,
          opacity: 1.0,
          z_index: 1,
          rotation_degrees: 0,
          mirror_horizontal: false,
          mirror_vertical: false,
          crop_zoom: 1.0,
          crop_x: 0.5,
          crop_y: 0.5,
        },
      ],
    });
    act(() => pushServerViewData(layout));

    // Verify server positions applied
    const layer0 = result.current.layers.find((l) => l.id === 'in_0')!;
    const layer1 = result.current.layers.find((l) => l.id === 'in_1')!;
    expect(layer0.x).toBe(160);
    expect(layer1.x).toBe(800);

    // Select layer 0
    act(() => result.current.selectLayer('in_0'));
    expect(result.current.selectedLayerId).toBe('in_0');

    // Switch to layer 1
    act(() => result.current.selectLayer('in_1'));
    expect(result.current.selectedLayerId).toBe('in_1');

    // Both layers should keep their server-resolved positions
    const layer0After = result.current.layers.find((l) => l.id === 'in_0')!;
    const layer1After = result.current.layers.find((l) => l.id === 'in_1')!;
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
        {
          id: 'text_0',
          x: 400,
          y: 300,
          width: 250,
          height: 48,
          opacity: 1.0,
          z_index: 100,
          rotation_degrees: 0,
          mirror_horizontal: false,
          mirror_vertical: false,
          measured_text_width: 245,
          measured_text_height: 44,
        },
      ],
    });
    act(() => pushServerViewData(layout));

    // Verify server state
    expect(result.current.textOverlays[0].x).toBe(400);
    expect(result.current.textOverlays[0].y).toBe(300);
    expect(result.current.textOverlays[0].measuredTextWidth).toBe(245);

    // Select the text overlay
    act(() => result.current.selectLayer('text_0'));
    expect(result.current.selectedLayerId).toBe('text_0');

    // Switch focus to video layer
    act(() => result.current.selectLayer('in_0'));
    expect(result.current.selectedLayerId).toBe('in_0');

    // Text overlay position and measurements must be unchanged
    expect(result.current.textOverlays[0].x).toBe(400);
    expect(result.current.textOverlays[0].y).toBe(300);
    expect(result.current.textOverlays[0].width).toBe(250);
    expect(result.current.textOverlays[0].measuredTextWidth).toBe(245);
    expect(result.current.textOverlays[0].measuredTextHeight).toBe(44);
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

    // Server resolves image at a different position
    const layout = makeServerLayout({
      image_overlays: [
        {
          id: 'img_0',
          x: 500,
          y: 300,
          width: 100,
          height: 80,
          opacity: 0.9,
          z_index: 50,
          rotation_degrees: 0,
          mirror_horizontal: false,
          mirror_vertical: false,
          measured_text_width: null,
          measured_text_height: null,
        },
      ],
    });
    act(() => pushServerViewData(layout));

    expect(result.current.imageOverlays[0].x).toBe(500);
    expect(result.current.imageOverlays[0].y).toBe(300);
    expect(result.current.imageOverlays[0].dataBase64).toBe('aW1hZ2UtZGF0YQ==');

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
    expect(result.current.imageOverlays[0].dataBase64).toBe('bmV3LWltYWdl');
    // Server-resolved position must be preserved
    expect(result.current.imageOverlays[0].x).toBe(500);
    expect(result.current.imageOverlays[0].y).toBe(300);
    // Server-resolved opacity must be preserved
    expect(result.current.imageOverlays[0].opacity).toBe(0.9);
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
    expect(result.current.textOverlays[0].x).toBe(0);
    expect(result.current.textOverlays[0].y).toBe(0);

    // Params update with new position
    act(() =>
      rerender({
        ...opts,
        params: makeParamsWithText({ rect: { x: 100, y: 200, width: 300, height: 50 } }),
      })
    );

    // Design view: position should update from params (not preserved)
    expect(result.current.textOverlays[0].x).toBe(100);
    expect(result.current.textOverlays[0].y).toBe(200);
  });
});
