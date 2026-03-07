// SPDX-FileCopyrightText: (c) 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Shared types for the render-performance measurement framework.
 *
 * Inspired by Reassure's API but designed to run natively inside Vitest
 * with React Testing Library, so no Jest dependency is needed.
 */

/** Stats collected for a single measurement run. */
export interface RenderMeasurement {
  /** Total number of React commits (re-renders) observed. */
  renderCount: number;
  /** Sum of `actualDuration` across all commits (ms). */
  totalDuration: number;
  /** Maximum single-commit `actualDuration` (ms). */
  maxCommitDuration: number;
  /** Per-commit durations in order (ms). */
  commitDurations: number[];
}

/** Aggregated statistics across multiple runs. */
export interface MeasureResult {
  /** Human-readable scenario name. */
  name: string;
  /** Number of runs executed. */
  runs: number;
  /** Mean render count across runs. */
  meanRenderCount: number;
  /** Standard deviation of render count. */
  stdevRenderCount: number;
  /** Mean total duration across runs (ms). */
  meanDuration: number;
  /** Standard deviation of total duration (ms). */
  stdevDuration: number;
  /** All individual measurements. */
  measurements: RenderMeasurement[];
}

/** Stored baseline entry for a single scenario. */
export interface BaselineEntry {
  name: string;
  meanRenderCount: number;
  stdevRenderCount: number;
  meanDuration: number;
  stdevDuration: number;
  runs: number;
  /** ISO timestamp of when the baseline was recorded. */
  timestamp: string;
}

/** Full baseline file containing multiple scenario entries. */
export interface BaselineFile {
  /** Schema version for forward-compat. */
  version: 1;
  entries: Record<string, BaselineEntry>;
}

/** Comparison result for a single scenario. */
export interface ComparisonResult {
  name: string;
  current: MeasureResult;
  baseline: BaselineEntry | null;
  /** Render-count change (current - baseline).  Null if no baseline. */
  renderCountDelta: number | null;
  /** Duration change (current - baseline).  Null if no baseline. */
  durationDelta: number | null;
  /** Whether the change is statistically significant (> 2 sigma). */
  significant: boolean;
  /** 'faster' | 'slower' | 'unchanged' | 'new' */
  status: 'faster' | 'slower' | 'unchanged' | 'new';
}
