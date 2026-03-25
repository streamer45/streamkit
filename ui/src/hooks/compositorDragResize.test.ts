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
    cropShape: 'rect' as const,
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
    snapGuideRefs: {
      current: {
        vertical: null,
        horizontal: null,
        left: null,
        right: null,
        top: null,
        bottom: null,
      },
    },
    ...overrides,
  };
}

// ── applyVisualUpdate — circle clip-path during resize ──────────────────────

describe('useCompositorDragResize circle crop resize', () => {
  it('updates clipPath on the DOM element when resizing a circle-crop layer', () => {
    // Enable fake timers BEFORE dispatching events so rAF (shimmed as
    // setTimeout in jsdom) is captured by the fake clock.
    vi.useFakeTimers();

    const circleLayer: LayerState = {
      ...makeLayer('layer-circle'),
      cropShape: 'circle' as const,
    };
    const el = document.createElement('div');
    const layerRefs = { current: new Map<string, HTMLDivElement>() };
    layerRefs.current.set('layer-circle', el);

    const deps = makeDeps({
      layerRefs,
      layersRef: { current: [circleLayer] },
      findAnyLayer: (id: string) => {
        if (id === 'layer-circle') return { state: circleLayer, kind: 'video' as const };
        return null;
      },
    });
    const { result } = renderHook(() => useCompositorDragResize(deps));

    // Simulate pointer-down on a resize handle
    act(() => {
      result.current.handleResizePointerDown('layer-circle', 'se', {
        button: 0,
        clientX: 300,
        clientY: 250,
        stopPropagation: vi.fn(),
        preventDefault: vi.fn(),
      } as unknown as React.PointerEvent);
    });

    // Simulate pointer-move to trigger resize + flush rAF
    act(() => {
      const moveEvent = new PointerEvent('pointermove', {
        clientX: 350,
        clientY: 300,
      });
      document.dispatchEvent(moveEvent);
      vi.runAllTimers();
    });

    vi.useRealTimers();

    // The clipPath should have been set to a circle() value
    expect(el.style.clipPath).toMatch(/^circle\(/);
  });

  it('does NOT set clipPath on rect-crop layers during resize', () => {
    vi.useFakeTimers();

    const rectLayer = makeLayer('layer-rect');
    const el = document.createElement('div');
    const layerRefs = { current: new Map<string, HTMLDivElement>() };
    layerRefs.current.set('layer-rect', el);

    const deps = makeDeps({
      layerRefs,
      layersRef: { current: [rectLayer] },
      findAnyLayer: (id: string) => {
        if (id === 'layer-rect') return { state: rectLayer, kind: 'video' as const };
        return null;
      },
    });
    const { result } = renderHook(() => useCompositorDragResize(deps));

    act(() => {
      result.current.handleResizePointerDown('layer-rect', 'se', {
        button: 0,
        clientX: 300,
        clientY: 250,
        stopPropagation: vi.fn(),
        preventDefault: vi.fn(),
      } as unknown as React.PointerEvent);
    });

    act(() => {
      const moveEvent = new PointerEvent('pointermove', {
        clientX: 350,
        clientY: 300,
      });
      document.dispatchEvent(moveEvent);
      vi.runAllTimers();
    });

    vi.useRealTimers();

    // clipPath should remain empty for rect crops
    expect(el.style.clipPath).toBe('');
  });
});

// ── zero-delta guard ────────────────────────────────────────────────────────

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
