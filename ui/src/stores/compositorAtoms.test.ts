// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Unit tests for compositor Jotai atoms.
 *
 * These tests verify two correctness properties that, when violated,
 * cause the render-cascade bug reported in PR #173:
 *
 *   1. Per-layer atom isolation — writing to a per-layer opacity/rotation
 *      atom does NOT cause the full layers-array atom to emit, so
 *      CompositorNode (which subscribes to the array) is NOT re-rendered.
 *
 *   2. syncLayerAppearanceAtoms round-trips — after syncing from a
 *      merged layer list, every per-layer atom reflects the correct value.
 */

import { describe, it, expect, afterEach } from 'vitest';

import type { LayerState } from '@/hooks/compositorLayerParsers';
import {
  compositorLayersAtom,
  compositorLayerOpacityAtom,
  compositorLayerRotationAtom,
  syncLayerAppearanceAtoms,
  cleanupCompositorAtoms,
} from '@/stores/compositorAtoms';
import { jotaiStore } from '@/stores/jotaiStore';

const NODE = 'test-node';

afterEach(() => {
  cleanupCompositorAtoms(NODE);
});

describe('per-layer atom isolation', () => {
  it('writing per-layer opacity atom does NOT trigger the layers-array atom', () => {
    // Seed the layers-array atom with proper LayerState objects
    const layers: LayerState[] = [
      {
        id: 'in_0',
        x: 0,
        y: 0,
        width: 1280,
        height: 720,
        opacity: 1,
        zIndex: 0,
        rotationDegrees: 0,
        mirrorHorizontal: false,
        mirrorVertical: false,
        visible: true,
        cropZoom: 1,
        cropX: 0.5,
        cropY: 0.5,
      },
      {
        id: 'in_1',
        x: 100,
        y: 100,
        width: 320,
        height: 240,
        opacity: 0.8,
        zIndex: 1,
        rotationDegrees: 0,
        mirrorHorizontal: false,
        mirrorVertical: false,
        visible: true,
        cropZoom: 1,
        cropX: 0.5,
        cropY: 0.5,
      },
    ];
    jotaiStore.set(compositorLayersAtom(NODE), layers);

    // Subscribe to layers-array atom and track notifications
    let layersNotified = 0;
    const unsub = jotaiStore.sub(compositorLayersAtom(NODE), () => {
      layersNotified++;
    });

    // Write to per-layer opacity atom (this is what slider ticks do)
    jotaiStore.set(compositorLayerOpacityAtom(`${NODE}:in_1`), 0.5);
    jotaiStore.set(compositorLayerOpacityAtom(`${NODE}:in_1`), 0.6);
    jotaiStore.set(compositorLayerOpacityAtom(`${NODE}:in_1`), 0.7);

    // The layers-array atom should NOT have been notified
    expect(layersNotified).toBe(0);

    unsub();
  });

  it('writing per-layer rotation atom does NOT trigger the layers-array atom', () => {
    jotaiStore.set(compositorLayersAtom(NODE), []);

    let layersNotified = 0;
    const unsub = jotaiStore.sub(compositorLayersAtom(NODE), () => {
      layersNotified++;
    });

    jotaiStore.set(compositorLayerRotationAtom(`${NODE}:in_0`), 15);
    jotaiStore.set(compositorLayerRotationAtom(`${NODE}:in_0`), 30);
    jotaiStore.set(compositorLayerRotationAtom(`${NODE}:in_0`), 45);

    expect(layersNotified).toBe(0);

    unsub();
  });

  it('per-layer atoms for different layers are independent', () => {
    // Subscribe to layer A opacity
    let layerANotified = 0;
    const unsubA = jotaiStore.sub(compositorLayerOpacityAtom(`${NODE}:in_0`), () => {
      layerANotified++;
    });

    // Write to layer B opacity — should NOT notify layer A
    jotaiStore.set(compositorLayerOpacityAtom(`${NODE}:in_1`), 0.3);

    expect(layerANotified).toBe(0);

    // Write to layer A — should notify only layer A
    jotaiStore.set(compositorLayerOpacityAtom(`${NODE}:in_0`), 0.9);
    expect(layerANotified).toBe(1);

    unsubA();
  });
});

describe('syncLayerAppearanceAtoms', () => {
  it('syncs all per-layer atoms from a merged layer list', () => {
    const items = [
      { id: 'in_0', opacity: 0.75, rotationDegrees: 15 },
      { id: 'in_1', opacity: 0.5, rotationDegrees: 90 },
    ];

    syncLayerAppearanceAtoms(NODE, items);

    expect(jotaiStore.get(compositorLayerOpacityAtom(`${NODE}:in_0`))).toBe(0.75);
    expect(jotaiStore.get(compositorLayerRotationAtom(`${NODE}:in_0`))).toBe(15);
    expect(jotaiStore.get(compositorLayerOpacityAtom(`${NODE}:in_1`))).toBe(0.5);
    expect(jotaiStore.get(compositorLayerRotationAtom(`${NODE}:in_1`))).toBe(90);
  });

  it('cleanup removes per-layer atoms created by sync', () => {
    syncLayerAppearanceAtoms(NODE, [{ id: 'in_0', opacity: 0.5, rotationDegrees: 45 }]);

    // Verify atom exists with value
    expect(jotaiStore.get(compositorLayerOpacityAtom(`${NODE}:in_0`))).toBe(0.5);

    cleanupCompositorAtoms(NODE);

    // After cleanup, a fresh read should return default values
    // because the atom family entry was removed.
    expect(jotaiStore.get(compositorLayerOpacityAtom(`${NODE}:in_0`))).toBe(1);
    expect(jotaiStore.get(compositorLayerRotationAtom(`${NODE}:in_0`))).toBe(0);
  });
});
