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
import { promoteEditedServerOnly } from './useCompositorLayers';

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
    aspectFit: true,
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
    // Add the inner crop element that applyVisualUpdate targets
    const cropEl = document.createElement('div');
    cropEl.setAttribute('data-crop-circle', '');
    el.appendChild(cropEl);
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

    // The clipPath should have been set on the inner [data-crop-circle] element
    const innerCrop = el.querySelector('[data-crop-circle]') as HTMLElement;
    expect(innerCrop.style.clipPath).toMatch(/^circle\(/);
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

// ── First-drag-of-server-stub regression ────────────────────────────────────
//
// Auto-PiP layers materialised by `mapServerLayers` carry `serverOnly:
// true`.  `serializeLayers` skips serverOnly layers so the server can keep
// aspect-fitting them.  When the user drags such a layer, the dragged
// entry must be promoted (serverOnly cleared) BEFORE serialization so
// the user's edit reaches the server.  Pre-fix, `pointerup` reconstructed
// the array from a closure-captured `updated` that still had
// `serverOnly: true`, the server never received the edit, and the next
// view-data tick snapped the layer back to its auto-fitted position.
describe('useCompositorDragResize first-drag of serverOnly layer', () => {
  it('fires throttledConfigChange with serverOnly cleared on the dragged layer', () => {
    const stubLayer: LayerState = { ...makeLayer('stub-1'), serverOnly: true };
    const otherStub: LayerState = { ...makeLayer('stub-2'), x: 500, serverOnly: true };

    // Simulate the production setLayers + store.sub + ref-update chain:
    // setLayers commits via promoteEditedServerOnly, then layersRef.current
    // catches up synchronously via the store subscription.
    const layersRef: { current: LayerState[] } = { current: [stubLayer, otherStub] };
    const setLayers = vi.fn((action: React.SetStateAction<LayerState[]>) => {
      const next =
        typeof action === 'function'
          ? (action as (prev: LayerState[]) => LayerState[])(layersRef.current)
          : action;
      layersRef.current = promoteEditedServerOnly(layersRef.current, next);
    });

    const deps = makeDeps({
      layersRef,
      setLayers,
      findAnyLayer: (id: string) => {
        const found = layersRef.current.find((l) => l.id === id);
        return found ? { state: found, kind: 'video' as const } : null;
      },
    });
    const { result } = renderHook(() => useCompositorDragResize(deps));

    act(() => {
      result.current.handleLayerPointerDown('stub-1', {
        button: 0,
        clientX: 200,
        clientY: 150,
        stopPropagation: vi.fn(),
        preventDefault: vi.fn(),
      } as unknown as React.PointerEvent);
    });

    act(() => {
      document.dispatchEvent(
        new PointerEvent('pointerup', { clientX: 260, clientY: 200 })
      );
    });

    expect(deps.throttledConfigChange).toHaveBeenCalledTimes(1);
    const sentLayers = (deps.throttledConfigChange as ReturnType<typeof vi.fn>).mock
      .calls[0][0] as LayerState[];

    // The dragged stub must be promoted to explicit config — otherwise
    // serializeLayers would skip it and the server would never receive
    // the user's edit.
    const draggedSent = sentLayers.find((l) => l.id === 'stub-1');
    expect(draggedSent).toBeDefined();
    expect(draggedSent?.serverOnly).toBeUndefined();
    // Untouched stubs in the same commit retain serverOnly so the
    // server keeps aspect-fitting sources the user didn't drag.
    const untouchedSent = sentLayers.find((l) => l.id === 'stub-2');
    expect(untouchedSent?.serverOnly).toBe(true);
  });
});
