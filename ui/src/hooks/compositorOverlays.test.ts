// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Tests for useCompositorOverlays via a synchronous ref-based state harness.

import { act, renderHook } from '@testing-library/react';
import React from 'react';
import { describe, expect, it, vi } from 'vitest';

import type { CommitAdapter } from './compositorCommit';
import type { ImageOverlayState, LayerState, TextOverlayState } from './compositorLayerParsers';
import { useCompositorOverlays, type OverlayDeps } from './compositorOverlays';

interface Harness {
  deps: OverlayDeps;
  layersRef: { current: LayerState[] };
  textRef: { current: TextOverlayState[] };
  imgRef: { current: ImageOverlayState[] };
  selectedRef: { current: string | null };
  commit: {
    commitLayers: ReturnType<typeof vi.fn>;
    commitOverlays: ReturnType<typeof vi.fn>;
    commitAll: ReturnType<typeof vi.fn>;
  };
  throttledConfigChange: ReturnType<typeof vi.fn>;
  throttledOverlayCommit: ReturnType<typeof vi.fn>;
}

// The harness applies setter callbacks synchronously into a ref.  This is
// sufficient for the hook's callbacks, which all read fresh ref values, but
// it does NOT exercise React's batching or transition machinery — bugs that
// depend on stale-closure reads or concurrent updates won't surface here.
function makeStateSetter<T>(ref: { current: T[] }): React.Dispatch<React.SetStateAction<T[]>> {
  return (next) => {
    ref.current = typeof next === 'function' ? (next as (prev: T[]) => T[])(ref.current) : next;
  };
}

function makeHarness(opts?: {
  initialLayers?: LayerState[];
  initialText?: TextOverlayState[];
  initialImages?: ImageOverlayState[];
}): Harness {
  const layersRef = { current: opts?.initialLayers ?? [] };
  const textRef = { current: opts?.initialText ?? [] };
  const imgRef = { current: opts?.initialImages ?? [] };
  const selectedRef = { current: null as string | null };

  const commit = {
    commitLayers: vi.fn(),
    commitOverlays: vi.fn(),
    commitAll: vi.fn(),
  } satisfies CommitAdapter;

  const throttledConfigChange = vi.fn();
  const throttledOverlayCommit = vi.fn();

  const deps: OverlayDeps = {
    commitAdapter: commit,
    setLayers: makeStateSetter(layersRef),
    setTextOverlays: makeStateSetter(textRef),
    setImageOverlays: makeStateSetter(imgRef),
    setSelectedLayerId: (next) => {
      selectedRef.current =
        typeof next === 'function'
          ? (next as (prev: string | null) => string | null)(selectedRef.current)
          : next;
    },
    layersRef,
    textOverlaysRef: textRef,
    imageOverlaysRef: imgRef,
    throttledConfigChange,
    throttledOverlayCommit,
  };

  return {
    deps,
    layersRef,
    textRef,
    imgRef,
    selectedRef,
    commit,
    throttledConfigChange,
    throttledOverlayCommit,
  };
}

function makeLayer(id: string, overrides: Partial<LayerState> = {}): LayerState {
  return {
    id,
    x: 0,
    y: 0,
    width: 200,
    height: 100,
    opacity: 1,
    zIndex: 0,
    rotationDegrees: 0,
    mirrorHorizontal: false,
    mirrorVertical: false,
    visible: true,
    cropZoom: 1,
    cropX: 0.5,
    cropY: 0.5,
    cropShape: 'rect',
    aspectFit: true,
    ...overrides,
  };
}

function makeTextOverlay(id: string, overrides: Partial<TextOverlayState> = {}): TextOverlayState {
  return {
    id,
    text: 'hi',
    x: 0,
    y: 0,
    width: 200,
    height: 40,
    color: [255, 255, 255, 255],
    fontSize: 24,
    fontName: 'samples/fonts/system/DejaVuSans.ttf',
    opacity: 1,
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
    assetPath: 'samples/images/user/test.png',
    x: 0,
    y: 0,
    width: 100,
    height: 100,
    opacity: 1,
    rotationDegrees: 0,
    zIndex: 200,
    mirrorHorizontal: false,
    mirrorVertical: false,
    visible: true,
    ...overrides,
  };
}

describe('useCompositorOverlays — selectLayer', () => {
  it('writes the supplied ID to the selection setter', () => {
    const h = makeHarness();
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.selectLayer('layer-x'));
    expect(h.selectedRef.current).toBe('layer-x');

    act(() => result.current.selectLayer(null));
    expect(h.selectedRef.current).toBeNull();
  });
});

describe('useCompositorOverlays — updateLayerOpacity', () => {
  it('clamps to [0, 1] and writes to the matching layer', () => {
    const h = makeHarness({ initialLayers: [makeLayer('a'), makeLayer('b')] });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerOpacity('a', 0.4));
    expect(h.layersRef.current[0].opacity).toBe(0.4);

    act(() => result.current.updateLayerOpacity('a', 2));
    expect(h.layersRef.current[0].opacity).toBe(1);

    act(() => result.current.updateLayerOpacity('a', -10));
    expect(h.layersRef.current[0].opacity).toBe(0);

    // Untouched layer remains intact.
    expect(h.layersRef.current[1].opacity).toBe(1);
  });

  it('is a no-op when the layer does not exist', () => {
    const h = makeHarness({ initialLayers: [makeLayer('a')] });
    const before = h.layersRef.current;
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerOpacity('ghost', 0.5));
    expect(h.layersRef.current).toBe(before);
  });

  it('forwards the next layers array to throttledConfigChange', () => {
    const h = makeHarness({ initialLayers: [makeLayer('a')] });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerOpacity('a', 0.3));
    expect(h.throttledConfigChange).toHaveBeenCalledTimes(1);
    expect(h.throttledConfigChange).toHaveBeenCalledWith(h.layersRef.current);
  });
});

describe('useCompositorOverlays — updateLayerRotation', () => {
  it('writes rotation in degrees verbatim (no clamp)', () => {
    const h = makeHarness({ initialLayers: [makeLayer('a')] });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerRotation('a', 250));
    expect(h.layersRef.current[0].rotationDegrees).toBe(250);
  });

  it('is a no-op when the layer does not exist', () => {
    const h = makeHarness({ initialLayers: [makeLayer('a')] });
    const before = h.layersRef.current;
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerRotation('ghost', 90));
    expect(h.layersRef.current).toBe(before);
  });
});

describe('useCompositorOverlays — updateLayerPositionSize', () => {
  it('applies partial x/y updates without touching dimensions', () => {
    const h = makeHarness({
      initialLayers: [makeLayer('a', { x: 0, y: 0, width: 200, height: 100 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerPositionSize('a', { x: 50, y: 75 }));

    expect(h.layersRef.current[0]).toMatchObject({ x: 50, y: 75, width: 200, height: 100 });
  });

  it('preserves aspect ratio when only width changes (video / image layers)', () => {
    const h = makeHarness({
      initialLayers: [makeLayer('a', { width: 200, height: 100 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerPositionSize('a', { width: 400 }));

    expect(h.layersRef.current[0].width).toBe(400);
    expect(h.layersRef.current[0].height).toBe(200);
  });

  it('preserves aspect ratio when only height changes', () => {
    const h = makeHarness({
      initialLayers: [makeLayer('a', { width: 200, height: 100 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerPositionSize('a', { height: 200 }));

    expect(h.layersRef.current[0].height).toBe(200);
    expect(h.layersRef.current[0].width).toBe(400);
  });

  it('honours independent width AND height changes (no AR preservation)', () => {
    const h = makeHarness({
      initialLayers: [makeLayer('a', { width: 200, height: 100 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerPositionSize('a', { width: 300, height: 333 }));

    expect(h.layersRef.current[0]).toMatchObject({ width: 300, height: 333 });
  });

  it('enforces a 20px minimum dimension', () => {
    const h = makeHarness({
      initialLayers: [makeLayer('a', { width: 200, height: 100 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerPositionSize('a', { width: 5, height: 5 }));

    expect(h.layersRef.current[0].width).toBeGreaterThanOrEqual(20);
    expect(h.layersRef.current[0].height).toBeGreaterThanOrEqual(20);
  });
});

describe('useCompositorOverlays — updateLayerZIndex', () => {
  it('re-sorts the array by zIndex after the update', () => {
    const h = makeHarness({
      initialLayers: [
        makeLayer('a', { zIndex: 0 }),
        makeLayer('b', { zIndex: 1 }),
        makeLayer('c', { zIndex: 2 }),
      ],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerZIndex('a', 99));

    expect(h.layersRef.current.map((l) => l.id)).toEqual(['b', 'c', 'a']);
  });
});

describe('useCompositorOverlays — toggleLayerVisibility', () => {
  it('flips visibility on a video layer', () => {
    const h = makeHarness({ initialLayers: [makeLayer('a', { visible: true })] });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.toggleLayerVisibility('a'));
    expect(h.layersRef.current[0].visible).toBe(false);

    act(() => result.current.toggleLayerVisibility('a'));
    expect(h.layersRef.current[0].visible).toBe(true);
  });

  it('flips visibility on a text overlay and commits via the overlays adapter', () => {
    const h = makeHarness({
      initialText: [makeTextOverlay('t1', { visible: true })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.toggleLayerVisibility('t1'));

    expect(h.textRef.current[0].visible).toBe(false);
    expect(h.commit.commitOverlays).toHaveBeenCalledTimes(1);
    expect(h.commit.commitOverlays).toHaveBeenCalledWith(h.textRef.current, h.imgRef.current);
  });

  it('flips visibility on an image overlay and commits via the overlays adapter', () => {
    const h = makeHarness({
      initialImages: [makeImageOverlay('i1', { visible: true })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.toggleLayerVisibility('i1'));

    expect(h.imgRef.current[0].visible).toBe(false);
    expect(h.commit.commitOverlays).toHaveBeenCalledTimes(1);
  });

  it('is a no-op when the ID matches nothing', () => {
    const h = makeHarness({
      initialLayers: [makeLayer('a')],
      initialText: [makeTextOverlay('t1')],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.toggleLayerVisibility('ghost'));

    expect(h.layersRef.current[0].visible).toBe(true);
    expect(h.textRef.current[0].visible).toBe(true);
  });
});

describe('useCompositorOverlays — updateLayerMirror', () => {
  it('flips horizontal mirror on a video layer', () => {
    const h = makeHarness({ initialLayers: [makeLayer('a')] });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerMirror('a', 'horizontal'));
    expect(h.layersRef.current[0].mirrorHorizontal).toBe(true);
    expect(h.layersRef.current[0].mirrorVertical).toBe(false);
  });

  it('flips vertical mirror on a video layer', () => {
    const h = makeHarness({ initialLayers: [makeLayer('a')] });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerMirror('a', 'vertical'));
    expect(h.layersRef.current[0].mirrorVertical).toBe(true);
  });

  it('flips mirror on a text overlay and commits', () => {
    const h = makeHarness({ initialText: [makeTextOverlay('t1')] });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerMirror('t1', 'horizontal'));

    expect(h.textRef.current[0].mirrorHorizontal).toBe(true);
    expect(h.commit.commitOverlays).toHaveBeenCalledTimes(1);
  });

  it('flips mirror on an image overlay and commits', () => {
    const h = makeHarness({ initialImages: [makeImageOverlay('i1')] });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerMirror('i1', 'vertical'));

    expect(h.imgRef.current[0].mirrorVertical).toBe(true);
    expect(h.commit.commitOverlays).toHaveBeenCalledTimes(1);
  });
});

describe('useCompositorOverlays — updateLayerCropZoom', () => {
  it('clamps cropZoom to a minimum of 1.0', () => {
    const h = makeHarness({ initialLayers: [makeLayer('a', { cropZoom: 1 })] });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerCropZoom('a', { cropZoom: 0.2 }));
    expect(h.layersRef.current[0].cropZoom).toBe(1);
  });

  it('clamps cropX / cropY to [0, 1]', () => {
    const h = makeHarness({
      initialLayers: [makeLayer('a', { cropZoom: 2, cropX: 0.5, cropY: 0.5 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerCropZoom('a', { cropX: -5, cropY: 9 }));
    expect(h.layersRef.current[0].cropX).toBe(0);
    expect(h.layersRef.current[0].cropY).toBe(1);
  });

  it('resets cropX/cropY to 0.5 when cropZoom is set back to 1.0', () => {
    const h = makeHarness({
      initialLayers: [makeLayer('a', { cropZoom: 3, cropX: 0.1, cropY: 0.9 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerCropZoom('a', { cropZoom: 1.0 }));

    expect(h.layersRef.current[0]).toMatchObject({ cropZoom: 1, cropX: 0.5, cropY: 0.5 });
  });

  it('updates cropShape when supplied', () => {
    const h = makeHarness({ initialLayers: [makeLayer('a', { cropShape: 'rect' })] });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateLayerCropZoom('a', { cropShape: 'circle' }));
    expect(h.layersRef.current[0].cropShape).toBe('circle');
  });
});

describe('useCompositorOverlays — addTextOverlay', () => {
  it('appends a new overlay with stagger and selects it', () => {
    const h = makeHarness();
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.addTextOverlay('Hello'));
    act(() => result.current.addTextOverlay('World'));

    expect(h.textRef.current).toHaveLength(2);
    expect(h.textRef.current[0].text).toBe('Hello');
    expect(h.textRef.current[1].text).toBe('World');
    // The second overlay's Y is staggered down by DEFAULT_OVERLAY_Y_STEP (=50).
    expect(h.textRef.current[1].y).toBeGreaterThan(h.textRef.current[0].y);
    // The hook always selects the newly added overlay.
    expect(h.selectedRef.current).toBe(h.textRef.current[1].id);
    expect(h.commit.commitOverlays).toHaveBeenCalled();
  });

  it.each([
    { name: 'layer holds the max', layer: 100, text: 50, image: 30, expected: 101 },
    { name: 'image holds the max', layer: 5, text: 50, image: 200, expected: 201 },
    { name: 'text holds the max', layer: 5, text: 75, image: 30, expected: 76 },
  ])(
    'assigns zIndex = max(layer, text, image) + 1 when $name',
    ({ layer, text, image, expected }) => {
      const h = makeHarness({
        initialLayers: [makeLayer('a', { zIndex: layer })],
        initialText: [makeTextOverlay('t1', { zIndex: text })],
        initialImages: [makeImageOverlay('i1', { zIndex: image })],
      });
      const { result } = renderHook(() => useCompositorOverlays(h.deps));

      act(() => result.current.addTextOverlay('top'));

      const added = h.textRef.current[h.textRef.current.length - 1];
      expect(added.zIndex).toBe(expected);
    }
  );
});

describe('useCompositorOverlays — updateTextOverlay', () => {
  it('applies partial updates and commits via throttledOverlayCommit', () => {
    const h = makeHarness({
      initialText: [makeTextOverlay('t1', { text: 'hi', fontSize: 24 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateTextOverlay('t1', { text: 'updated' }));

    expect(h.textRef.current[0].text).toBe('updated');
    expect(h.throttledOverlayCommit).toHaveBeenCalledTimes(1);
  });

  it('clears stale measured dimensions when fontSize / fontName / text changes', () => {
    const h = makeHarness({
      initialText: [
        makeTextOverlay('t1', {
          text: 'old',
          measuredTextWidth: 80,
          measuredTextHeight: 32,
          width: 1000,
          height: 1000,
        }),
      ],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateTextOverlay('t1', { text: 'new' }));

    expect(h.textRef.current[0].measuredTextWidth).toBeUndefined();
    expect(h.textRef.current[0].measuredTextHeight).toBeUndefined();
  });

  it('auto-expands height to fit a larger font when no explicit height is supplied', () => {
    // Existing overlay has a tiny height; updateTextOverlay must enforce
    // a minimum height of ceil(fontSize * 1.4) so the bounding box can
    // contain the new font.
    const h = makeHarness({
      initialText: [makeTextOverlay('t1', { fontSize: 24, height: 10 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateTextOverlay('t1', { fontSize: 48 }));

    expect(h.textRef.current[0].height).toBeGreaterThanOrEqual(Math.ceil(48 * 1.4));
  });

  it('does NOT override an explicit height supplied in the same update', () => {
    const h = makeHarness({
      initialText: [makeTextOverlay('t1', { fontSize: 24, height: 10 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateTextOverlay('t1', { fontSize: 48, height: 50 }));

    expect(h.textRef.current[0].height).toBe(50);
  });
});

describe('useCompositorOverlays — removeTextOverlay', () => {
  it('removes the overlay, clears selection, and commits via the immediate adapter', () => {
    const h = makeHarness({
      initialText: [makeTextOverlay('t1'), makeTextOverlay('t2')],
    });
    h.selectedRef.current = 't1';
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.removeTextOverlay('t1'));

    expect(h.textRef.current.map((o) => o.id)).toEqual(['t2']);
    expect(h.selectedRef.current).toBeNull();
    expect(h.commit.commitOverlays).toHaveBeenCalledTimes(1);
  });
});

describe('useCompositorOverlays — addImageOverlay', () => {
  it('appends a default 200×200 overlay when natural dimensions are missing', () => {
    const h = makeHarness();
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.addImageOverlay('samples/images/user/logo.png'));

    expect(h.imgRef.current).toHaveLength(1);
    expect(h.imgRef.current[0]).toMatchObject({
      assetPath: 'samples/images/user/logo.png',
      width: 200,
      height: 200,
    });
  });

  it('preserves aspect ratio when natural dimensions are supplied (wide image)', () => {
    const h = makeHarness();
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.addImageOverlay('a.png', 800, 400));

    expect(h.imgRef.current[0].width).toBe(200);
    expect(h.imgRef.current[0].height).toBe(100);
  });

  it('preserves aspect ratio when natural dimensions are supplied (tall image)', () => {
    const h = makeHarness();
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.addImageOverlay('a.png', 100, 400));

    expect(h.imgRef.current[0].width).toBe(50);
    expect(h.imgRef.current[0].height).toBe(200);
  });

  it('does not upscale images smaller than the 200px cap', () => {
    const h = makeHarness();
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.addImageOverlay('small.png', 80, 60));

    expect(h.imgRef.current[0].width).toBe(80);
    expect(h.imgRef.current[0].height).toBe(60);
  });
});

describe('useCompositorOverlays — image overlay update + remove', () => {
  it('updates fields and commits via throttledOverlayCommit', () => {
    const h = makeHarness({ initialImages: [makeImageOverlay('i1', { opacity: 1 })] });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.updateImageOverlay('i1', { opacity: 0.3 }));

    expect(h.imgRef.current[0].opacity).toBe(0.3);
    expect(h.throttledOverlayCommit).toHaveBeenCalledTimes(1);
  });

  it('removes an image overlay and clears selection', () => {
    const h = makeHarness({ initialImages: [makeImageOverlay('i1'), makeImageOverlay('i2')] });
    h.selectedRef.current = 'i1';
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() => result.current.removeImageOverlay('i1'));

    expect(h.imgRef.current.map((o) => o.id)).toEqual(['i2']);
    expect(h.selectedRef.current).toBeNull();
  });
});

describe('useCompositorOverlays — reorderLayers', () => {
  it('only commits the bands that changed', () => {
    const h = makeHarness({
      initialLayers: [makeLayer('a', { zIndex: 0 }), makeLayer('b', { zIndex: 1 })],
      initialText: [makeTextOverlay('t1', { zIndex: 100 })],
      initialImages: [makeImageOverlay('i1', { zIndex: 200 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() =>
      result.current.reorderLayers([
        { id: 'a', kind: 'video', zIndex: 5 },
        { id: 'b', kind: 'video', zIndex: 1 }, // unchanged
        { id: 't1', kind: 'text', zIndex: 100 }, // unchanged
        { id: 'i1', kind: 'image', zIndex: 200 }, // unchanged
      ])
    );

    // Video layers re-sorted after a's zIndex change.
    expect(h.layersRef.current.map((l) => l.id)).toEqual(['b', 'a']);
    expect(h.commit.commitAll).toHaveBeenCalledTimes(1);
    expect(h.commit.commitAll).toHaveBeenCalledWith(
      h.layersRef.current,
      h.textRef.current,
      h.imgRef.current,
      { layers: true, overlays: false }
    );
  });

  it('updates text + image overlays when their zIndex changes', () => {
    const h = makeHarness({
      initialLayers: [makeLayer('a', { zIndex: 0 })],
      initialText: [makeTextOverlay('t1', { zIndex: 100 })],
      initialImages: [makeImageOverlay('i1', { zIndex: 200 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() =>
      result.current.reorderLayers([
        { id: 'a', kind: 'video', zIndex: 0 },
        { id: 't1', kind: 'text', zIndex: 150 },
        { id: 'i1', kind: 'image', zIndex: 250 },
      ])
    );

    expect(h.textRef.current[0].zIndex).toBe(150);
    expect(h.imgRef.current[0].zIndex).toBe(250);
    expect(h.commit.commitAll).toHaveBeenCalledWith(
      h.layersRef.current,
      h.textRef.current,
      h.imgRef.current,
      { layers: false, overlays: true }
    );
  });

  it('does NOT commit when no entries actually changed', () => {
    const h = makeHarness({
      initialLayers: [makeLayer('a', { zIndex: 0 })],
      initialText: [makeTextOverlay('t1', { zIndex: 100 })],
    });
    const { result } = renderHook(() => useCompositorOverlays(h.deps));

    act(() =>
      result.current.reorderLayers([
        { id: 'a', kind: 'video', zIndex: 0 },
        { id: 't1', kind: 'text', zIndex: 100 },
      ])
    );

    expect(h.commit.commitAll).not.toHaveBeenCalled();
  });
});
