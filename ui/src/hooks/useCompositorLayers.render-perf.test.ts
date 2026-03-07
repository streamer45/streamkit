// SPDX-FileCopyrightText: (c) 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Render-performance regression tests for the compositor layer hook.
 *
 * Uses the Layer 1 measureHookRenders framework to verify that:
 *   1. Rapid slider interactions produce bounded render counts.
 *   2. Callback references remain stable (preventing cascade re-renders
 *      in memoized siblings like UnifiedLayerList, OpacityControl, etc.).
 *   3. Server echo-back param changes don't trigger unnecessary re-renders.
 *
 * These tests catch the class of regression fixed in PR #89 where slider
 * drags caused callback instability, breaking React.memo barriers and
 * cascading re-renders to unrelated components.
 */

import { act } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';

import {
  measureHookRenders,
  writeBaseline,
  readBaseline,
  compare,
  formatReport,
} from '@/test/perf';
import type { MeasureResult } from '@/test/perf';

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

/**
 * Maximum renders allowed for 20 rapid slider ticks.
 *
 * Expected: 1 (mount) + 1 (selectLayer) + 20 (slider ticks) = 22.
 * Budget gives headroom for React batching variance.
 */
const SLIDER_BUDGET = 30;

/**
 * Maximum renders allowed for 10 param reference changes.
 *
 * Expected: 1 (mount) + 10 (rerenders) = 11.
 * The hook's mergeOverlayState should avoid extra state updates when
 * content is identical, so many of these may be no-ops.
 */
const PARAM_ECHO_BUDGET = 15;

describe('useCompositorLayers render-performance', () => {
  const results: MeasureResult[] = [];

  it('rapid opacity slider drags produce bounded renders', () => {
    const opts = defaultOptions();
    const result = measureHookRenders(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      {
        initialProps: opts,
        scenario: ({ result }) => {
          act(() => result.current.selectLayer('in_1'));
          for (let i = 0; i < 20; i++) {
            act(() => result.current.updateLayerOpacity('in_1', 0.5 + i * 0.02));
          }
        },
      }
    );
    result.name = 'opacity-slider-20-ticks';
    results.push(result);

    expect(result.meanRenderCount).toBeLessThanOrEqual(SLIDER_BUDGET);
  });

  it('rapid rotation slider drags produce bounded renders', () => {
    const opts = defaultOptions();
    const result = measureHookRenders(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      {
        initialProps: opts,
        scenario: ({ result }) => {
          act(() => result.current.selectLayer('in_1'));
          for (let i = 0; i < 20; i++) {
            act(() => result.current.updateLayerRotation('in_1', i * 18));
          }
        },
      }
    );
    result.name = 'rotation-slider-20-ticks';
    results.push(result);

    expect(result.meanRenderCount).toBeLessThanOrEqual(SLIDER_BUDGET);
  });

  it('callback references stay stable across param echo-backs (cascade prevention)', () => {
    const opts = defaultOptions();
    const result = measureHookRenders(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      {
        initialProps: opts,
        scenario: ({ result, rerender }) => {
          const before = {
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
          };

          // 10 param reference changes (server echo-backs with same content)
          for (let i = 0; i < 10; i++) {
            act(() => rerender({ ...opts, params: makeParams() }));
          }

          const after = result.current;
          const callbacks = Object.keys(before) as Array<keyof typeof before>;
          const unstable = callbacks.filter(
            (name) => before[name] !== after[name as keyof typeof after]
          );

          // Assert inside scenario so the measurement captures the full cost
          expect(unstable).toEqual([]);
        },
      }
    );
    result.name = 'param-echo-callback-stability';
    results.push(result);

    expect(result.meanRenderCount).toBeLessThanOrEqual(PARAM_ECHO_BUDGET);
  });

  it('mixed opacity + rotation slider updates remain efficient', () => {
    const opts = defaultOptions();
    const result = measureHookRenders(
      (props: UseCompositorLayersOptions) => useCompositorLayers(props),
      {
        initialProps: opts,
        scenario: ({ result }) => {
          act(() => result.current.selectLayer('in_1'));
          // Alternate between opacity and rotation (simulates switching controls)
          for (let i = 0; i < 10; i++) {
            act(() => result.current.updateLayerOpacity('in_1', 0.3 + i * 0.05));
            act(() => result.current.updateLayerRotation('in_1', i * 36));
          }
        },
      }
    );
    result.name = 'mixed-slider-20-ticks';
    results.push(result);

    // 20 mixed ticks should stay within slider budget
    expect(result.meanRenderCount).toBeLessThanOrEqual(SLIDER_BUDGET);
  });

  it('writes baseline and prints comparison report', () => {
    // Read existing baseline (if any) and compare
    const baseline = readBaseline();
    const comparisons = results.map((r) => compare(r, baseline.entries[r.name] ?? null));
    const report = formatReport(comparisons);

    // Print report for visibility in CI logs
    // eslint-disable-next-line no-console
    console.log('\n' + report + '\n');

    // Only overwrite the baseline when running via `just perf-ui` (sets
    // UPDATE_PERF_BASELINE=1).  Regular `just test-ui` runs compare but
    // never silently clobber the committed baseline.
    if (process.env.UPDATE_PERF_BASELINE === '1') {
      writeBaseline(results);
    }

    // Fail if any scenario regressed compared to baseline
    const regressions = comparisons.filter((c) => c.status === 'slower');
    expect(
      regressions,
      `Regressions detected:\n${regressions.map((r) => r.name).join(', ')}`
    ).toHaveLength(0);
  });
});
