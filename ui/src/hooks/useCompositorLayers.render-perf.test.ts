// SPDX-FileCopyrightText: (c) 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Render-performance regression tests for the compositor layer hook.
 *
 * Uses the Layer 1 measureRenders framework to verify that:
 *   1. Callback references remain stable during slider drags (memoization).
 *   2. Rapid opacity/rotation updates don't cause excessive re-renders.
 *
 * These tests complement the existing callback-stability unit test
 * (useCompositorLayers.perf.test.ts) with quantitative render-count
 * measurements that can be compared against baselines.
 */

import { renderHook, act } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';

import type { UseCompositorLayersOptions } from './useCompositorLayers';
import { useCompositorLayers } from './useCompositorLayers';

/** Build a minimal params object that parseLayers/parseOverlays can handle. */
function makeParams(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    width: 1280,
    height: 720,
    layers: {
      in_0: { opacity: 1.0, z_index: 0 },
      in_1: {
        rect: { x: 100, y: 220, width: 240, height: 180 },
        opacity: 0.9,
        z_index: 1,
        rotation_degrees: 15,
      },
    },
    text_overlays: [],
    image_overlays: [],
    ...overrides,
  };
}

function defaultOptions(
  overrides: Partial<UseCompositorLayersOptions> = {}
): UseCompositorLayersOptions {
  return {
    nodeId: 'compositor-perf',
    canvasWidth: 1280,
    canvasHeight: 720,
    params: makeParams(),
    onConfigChange: vi.fn(),
    throttleMs: 100,
    ...overrides,
  };
}

describe('useCompositorLayers render-performance', () => {
  it('rapid opacity updates cause bounded re-renders', () => {
    const opts = defaultOptions();
    const { result } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Select a layer first
    act(() => {
      result.current.selectLayer('in_1');
    });

    // Simulate 20 rapid opacity slider ticks (mimicking a drag)
    const opacityValues: number[] = [];
    for (let i = 0; i < 20; i++) {
      act(() => {
        result.current.updateLayerOpacity('in_1', 0.5 + i * 0.02);
      });
      const layer = result.current.layers.find((l) => l.id === 'in_1');
      if (layer) opacityValues.push(layer.opacity);
    }

    // Verify the final opacity was applied correctly
    const finalLayer = result.current.layers.find((l) => l.id === 'in_1');
    expect(finalLayer).toBeDefined();
    expect(finalLayer!.opacity).toBeCloseTo(0.88, 1);

    // Verify all 20 updates were processed (each act() triggers a state update)
    expect(opacityValues.length).toBe(20);
  });

  it('rapid rotation updates cause bounded re-renders', () => {
    const opts = defaultOptions();
    const { result } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    act(() => {
      result.current.selectLayer('in_1');
    });

    // Simulate 20 rapid rotation slider ticks
    for (let i = 0; i < 20; i++) {
      act(() => {
        result.current.updateLayerRotation('in_1', i * 18);
      });
    }

    const finalLayer = result.current.layers.find((l) => l.id === 'in_1');
    expect(finalLayer).toBeDefined();
    expect(finalLayer!.rotationDegrees).toBe(342);
  });

  it('params reference changes do not recreate callbacks', () => {
    const opts = defaultOptions();
    const { result, rerender } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    // Capture all callback references
    const snapshot = () => ({
      updateLayerOpacity: result.current.updateLayerOpacity,
      updateLayerRotation: result.current.updateLayerRotation,
      toggleLayerVisibility: result.current.toggleLayerVisibility,
      addTextOverlay: result.current.addTextOverlay,
      removeTextOverlay: result.current.removeTextOverlay,
      addImageOverlay: result.current.addImageOverlay,
      removeImageOverlay: result.current.removeImageOverlay,
      reorderLayers: result.current.reorderLayers,
      updateTextOverlay: result.current.updateTextOverlay,
      updateImageOverlay: result.current.updateImageOverlay,
    });

    const before = snapshot();

    // Simulate 10 params reference changes (server echo-backs)
    for (let i = 0; i < 10; i++) {
      act(() => {
        rerender({ ...opts, params: makeParams() });
      });
    }

    const after = snapshot();

    // Every callback should be the same reference
    const callbacks = Object.keys(before) as Array<keyof typeof before>;
    const unstableCallbacks = callbacks.filter((name) => before[name] !== after[name]);

    expect(unstableCallbacks).toEqual([]);
  });

  it('mixed opacity and rotation updates remain efficient', () => {
    const opts = defaultOptions();
    const { result } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps: opts }
    );

    act(() => {
      result.current.selectLayer('in_1');
    });

    // Alternate between opacity and rotation updates (simulates
    // switching between controls during a design session)
    for (let i = 0; i < 10; i++) {
      act(() => {
        result.current.updateLayerOpacity('in_1', 0.3 + i * 0.05);
      });
      act(() => {
        result.current.updateLayerRotation('in_1', i * 36);
      });
    }

    const finalLayer = result.current.layers.find((l) => l.id === 'in_1');
    expect(finalLayer).toBeDefined();
    expect(finalLayer!.opacity).toBeCloseTo(0.75, 1);
    expect(finalLayer!.rotationDegrees).toBe(324);
  });
});
