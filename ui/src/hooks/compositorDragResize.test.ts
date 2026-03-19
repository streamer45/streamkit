// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Unit tests for the zero-delta click guard in useCompositorDragResize.
 *
 * Verifies that a pointer-up at the exact same position as pointer-down
 * (i.e. a click-to-select with zero movement) does NOT fire the config
 * change callback, preventing stale client positions from overwriting
 * the server's resolved layout.
 */

import { renderHook, act } from '@testing-library/react';
import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';

import type { DragResizeDeps } from './compositorDragResize';
import { useCompositorDragResize } from './compositorDragResize';
import type { LayerState } from './compositorLayerParsers';

/** Build a minimal layer for testing. */
function makeLayer(id: string): LayerState {
  return {
    id,
    x: 100,
    y: 100,
    width: 200,
    height: 150,
    opacity: 1,
    zIndex: 0,
    visible: true,
    rotationDegrees: 0,
    mirrorHorizontal: false,
    mirrorVertical: false,
    cropZoom: 1.0,
    cropX: 0.5,
    cropY: 0.5,
    cropCircle: false,
  };
}

function makeDeps(overrides: Partial<DragResizeDeps> = {}): DragResizeDeps {
  const layer = makeLayer('layer-1');
  return {
    canvasWidth: 1280,
    canvasHeight: 720,
    dragStateRef: { current: null },
    layerRefs: { current: new Map() },
    layersRef: { current: [layer] },
    textOverlaysRef: { current: [] },
    imageOverlaysRef: { current: [] },
    setLayers: vi.fn(),
    setTextOverlays: vi.fn(),
    setImageOverlays: vi.fn(),
    setSelectedLayerId: vi.fn(),
    setIsDragging: vi.fn(),
    findAnyLayer: (id: string) => {
      if (id === 'layer-1') return { state: layer, kind: 'video' as const };
      return null;
    },
    throttledConfigChange: vi.fn(),
    commitOverlaysRef: { current: vi.fn() },
    snapGuideRefs: { current: { vertical: null, horizontal: null } },
    ...overrides,
  };
}

describe('useCompositorDragResize zero-delta guard', () => {
  let deps: DragResizeDeps;

  beforeEach(() => {
    deps = makeDeps();
  });

  it('should NOT fire throttledConfigChange on zero-delta click (video layer)', () => {
    const { result } = renderHook(() => useCompositorDragResize(deps));

    // Simulate pointer-down at (500, 300)
    act(() => {
      result.current.handleLayerPointerDown('layer-1', {
        button: 0,
        clientX: 500,
        clientY: 300,
        stopPropagation: vi.fn(),
        preventDefault: vi.fn(),
      } as unknown as React.PointerEvent);
    });

    // Verify drag state was set
    expect(deps.dragStateRef.current).not.toBeNull();

    // Simulate pointer-up at the exact same position (zero delta)
    act(() => {
      const pointerUpEvent = new PointerEvent('pointerup', {
        clientX: 500,
        clientY: 300,
      });
      document.dispatchEvent(pointerUpEvent);
    });

    // throttledConfigChange must NOT have been called
    expect(deps.throttledConfigChange).not.toHaveBeenCalled();

    // setLayers must NOT have been called (no state update)
    expect(deps.setLayers).not.toHaveBeenCalled();
  });

  it('should fire throttledConfigChange on actual drag (video layer)', () => {
    const { result } = renderHook(() => useCompositorDragResize(deps));

    // Simulate pointer-down at (500, 300)
    act(() => {
      result.current.handleLayerPointerDown('layer-1', {
        button: 0,
        clientX: 500,
        clientY: 300,
        stopPropagation: vi.fn(),
        preventDefault: vi.fn(),
      } as unknown as React.PointerEvent);
    });

    // Simulate pointer-up at a DIFFERENT position (non-zero delta)
    act(() => {
      const pointerUpEvent = new PointerEvent('pointerup', {
        clientX: 520,
        clientY: 310,
      });
      document.dispatchEvent(pointerUpEvent);
    });

    // throttledConfigChange SHOULD have been called
    expect(deps.throttledConfigChange).toHaveBeenCalled();
  });
});
