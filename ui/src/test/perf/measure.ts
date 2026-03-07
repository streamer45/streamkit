// SPDX-FileCopyrightText: (c) 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Render-performance measurement utilities for Vitest + React Testing Library.
 *
 * Wraps a component in React.Profiler, runs a scenario multiple times, and
 * collects render count / duration statistics.  Designed as a lightweight,
 * Vitest-native alternative to Reassure.
 *
 * Usage in a *.perf-test.tsx file:
 *
 *   import { measureRenders } from '@/test/perf/measure';
 *
 *   test('OpacityControl renders efficiently', async () => {
 *     const result = await measureRenders(<OpacityControl value={0.5} onChange={() => {}} />, {
 *       runs: 5,
 *       scenario: async (screen) => {
 *         // interact with the component
 *       },
 *     });
 *     expect(result.meanRenderCount).toBeLessThanOrEqual(3);
 *   });
 */

import { render, cleanup, type RenderResult } from '@testing-library/react';
import React from 'react';

import type { MeasureResult, RenderMeasurement } from './types';

/** Options for {@link measureRenders}. */
export interface MeasureRendersOptions {
  /** Number of measurement runs (default: 7). */
  runs?: number;
  /** Number of warm-up runs discarded before measurement (default: 1). */
  warmupRuns?: number;
  /**
   * Async scenario executed after mount.  Receives the RTL `screen` object
   * so you can fire interactions (click, type, etc.) and measure the resulting
   * re-renders.
   */
  scenario?: (screen: RenderResult) => Promise<void>;
  /**
   * Optional wrapper component (e.g. a context provider) that will wrap the
   * component under test.
   */
  wrapper?: React.ComponentType<{ children: React.ReactNode }>;
}

function stats(values: number[]): { mean: number; stdev: number } {
  if (values.length === 0) return { mean: 0, stdev: 0 };
  const mean = values.reduce((a, b) => a + b, 0) / values.length;
  const variance = values.reduce((sum, v) => sum + (v - mean) ** 2, 0) / values.length;
  return { mean, stdev: Math.sqrt(variance) };
}

/**
 * Mount `ui`, optionally run a `scenario`, and measure React render
 * performance across multiple runs.
 */
export async function measureRenders(
  ui: React.ReactElement,
  options: MeasureRendersOptions = {}
): Promise<MeasureResult> {
  const { runs = 7, warmupRuns = 1, scenario, wrapper: Wrapper } = options;

  const totalRuns = warmupRuns + runs;
  const allMeasurements: RenderMeasurement[] = [];

  for (let i = 0; i < totalRuns; i++) {
    const measurement: RenderMeasurement = {
      renderCount: 0,
      totalDuration: 0,
      maxCommitDuration: 0,
      commitDurations: [],
    };

    const onRender: React.ProfilerOnRenderCallback = (_id, _phase, actualDuration) => {
      measurement.renderCount += 1;
      measurement.totalDuration += actualDuration;
      measurement.maxCommitDuration = Math.max(measurement.maxCommitDuration, actualDuration);
      measurement.commitDurations.push(actualDuration);
    };

    const profiled = React.createElement(React.Profiler, { id: 'measure', onRender }, ui);

    const wrapped = Wrapper ? React.createElement(Wrapper, null, profiled) : profiled;

    const screen = render(wrapped);

    if (scenario) {
      await scenario(screen);
    }

    // Small tick to let any pending effects flush.
    await new Promise((r) => setTimeout(r, 0));

    cleanup();

    // Only record after warm-up runs.
    if (i >= warmupRuns) {
      allMeasurements.push(measurement);
    }
  }

  const renderCounts = allMeasurements.map((m) => m.renderCount);
  const durations = allMeasurements.map((m) => m.totalDuration);
  const rcStats = stats(renderCounts);
  const durStats = stats(durations);

  return {
    name: '',
    runs,
    meanRenderCount: rcStats.mean,
    stdevRenderCount: rcStats.stdev,
    meanDuration: durStats.mean,
    stdevDuration: durStats.stdev,
    measurements: allMeasurements,
  };
}
