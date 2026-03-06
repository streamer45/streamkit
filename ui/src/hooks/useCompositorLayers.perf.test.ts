// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Performance regression test for useCompositorLayers.
 *
 * Verifies that the throttled config callbacks and overlay commit helper
 * remain referentially stable when only the `params` object reference
 * changes (e.g. server echo-back after a config update).  Prior to the
 * paramsRef optimisation these callbacks were recreated on every params
 * change, cascading to ~11 downstream callback recreations per cycle.
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

describe('useCompositorLayers callback stability', () => {
  it('callbacks remain stable across params reference changes', () => {
    const onConfigChange = vi.fn();
    const params = makeParams();

    const initialProps: UseCompositorLayersOptions = {
      nodeId: 'compositor-1',
      canvasWidth: 1280,
      canvasHeight: 720,
      params,
      onConfigChange,
      throttleMs: 100,
    };

    const { result, rerender } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps }
    );

    // Capture initial callback references
    const firstUpdateOpacity = result.current.updateLayerOpacity;
    const firstUpdateRotation = result.current.updateLayerRotation;
    const firstToggleVisibility = result.current.toggleLayerVisibility;
    const firstAddText = result.current.addTextOverlay;
    const firstRemoveText = result.current.removeTextOverlay;
    const firstAddImage = result.current.addImageOverlay;
    const firstRemoveImage = result.current.removeImageOverlay;
    const firstReorderLayers = result.current.reorderLayers;
    const firstUpdateText = result.current.updateTextOverlay;
    const firstUpdateImage = result.current.updateImageOverlay;

    // Simulate 10 params reference changes (server echo-backs)
    // Each creates a new object with identical content
    let stableCount = 0;
    const totalCycles = 10;
    const callbacks = [
      'updateLayerOpacity',
      'updateLayerRotation',
      'toggleLayerVisibility',
      'addTextOverlay',
      'removeTextOverlay',
      'addImageOverlay',
      'removeImageOverlay',
      'reorderLayers',
      'updateTextOverlay',
      'updateImageOverlay',
    ] as const;

    for (let i = 0; i < totalCycles; i++) {
      // New object reference, same content — simulates server echo-back
      const newParams = makeParams();
      act(() => {
        rerender({ ...initialProps, params: newParams });
      });
    }

    // Check each callback is still the same reference
    const firstRefs: Record<string, unknown> = {
      updateLayerOpacity: firstUpdateOpacity,
      updateLayerRotation: firstUpdateRotation,
      toggleLayerVisibility: firstToggleVisibility,
      addTextOverlay: firstAddText,
      removeTextOverlay: firstRemoveText,
      addImageOverlay: firstAddImage,
      removeImageOverlay: firstRemoveImage,
      reorderLayers: firstReorderLayers,
      updateTextOverlay: firstUpdateText,
      updateImageOverlay: firstUpdateImage,
    };

    for (const name of callbacks) {
      if (result.current[name] === firstRefs[name]) {
        stableCount++;
      }
    }

    // All 10 callbacks should remain stable across params changes
    expect(stableCount).toBe(callbacks.length);

    // Verify that callbacks still work correctly (functional test)
    act(() => {
      result.current.updateLayerOpacity('in_1', 0.5);
    });
    expect(onConfigChange).toHaveBeenCalled();
  });

  it('callbacks DO update when nodeId changes (correctness check)', () => {
    const onConfigChange = vi.fn();
    const params = makeParams();

    const initialProps: UseCompositorLayersOptions = {
      nodeId: 'compositor-1',
      canvasWidth: 1280,
      canvasHeight: 720,
      params,
      onConfigChange,
      throttleMs: 100,
    };

    const { result, rerender } = renderHook(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      { initialProps }
    );

    const firstReorder = result.current.reorderLayers;

    // Change nodeId — callbacks SHOULD be recreated
    act(() => {
      rerender({ ...initialProps, nodeId: 'compositor-2' });
    });

    // reorderLayers depends on nodeId, so it should change
    expect(result.current.reorderLayers).not.toBe(firstReorder);
  });
});
