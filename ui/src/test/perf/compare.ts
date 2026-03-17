// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Baseline comparison utilities for render-performance tests.
 *
 * Reads / writes a JSON baseline file and produces a human-readable
 * comparison report that can be printed in the terminal or posted as
 * a PR comment.
 */

import * as fs from 'fs';
import * as path from 'path';

import type { BaselineEntry, BaselineFile, ComparisonResult, MeasureResult } from './types';

/** Default location for the baseline file (repo-root relative). */
const DEFAULT_BASELINE_PATH = path.resolve(import.meta.dirname, '../../../../perf-baselines.json');

// ── Baseline I/O ─────────────────────────────────────────────────────────────

export function readBaseline(filePath: string = DEFAULT_BASELINE_PATH): BaselineFile {
  try {
    const raw = fs.readFileSync(filePath, 'utf-8');
    return JSON.parse(raw) as BaselineFile;
  } catch {
    return { version: 1, entries: {} };
  }
}

export function writeBaseline(
  results: MeasureResult[],
  filePath: string = DEFAULT_BASELINE_PATH
): void {
  const existing = readBaseline(filePath);
  const now = new Date().toISOString();

  for (const r of results) {
    existing.entries[r.name] = {
      name: r.name,
      meanRenderCount: r.meanRenderCount,
      stdevRenderCount: r.stdevRenderCount,
      meanDuration: r.meanDuration,
      stdevDuration: r.stdevDuration,
      runs: r.runs,
      timestamp: now,
    };
  }

  fs.writeFileSync(filePath, JSON.stringify(existing, null, 2) + '\n');
}

// ── Comparison ───────────────────────────────────────────────────────────────

/**
 * Significance threshold in standard deviations.  A change must exceed
 * `baseline.mean +/- SIGMA * baseline.stdev` to be flagged.
 */
const SIGMA = 2;

export function compare(current: MeasureResult, baseline: BaselineEntry | null): ComparisonResult {
  if (!baseline) {
    return {
      name: current.name,
      current,
      baseline: null,
      renderCountDelta: null,
      durationDelta: null,
      significant: false,
      status: 'new',
    };
  }

  const rcDelta = current.meanRenderCount - baseline.meanRenderCount;
  const durDelta = current.meanDuration - baseline.meanDuration;

  // Significant if render-count change exceeds SIGMA * stdev
  const rcThreshold = SIGMA * Math.max(baseline.stdevRenderCount, 0.5);
  const significant = Math.abs(rcDelta) > rcThreshold;

  let status: ComparisonResult['status'] = 'unchanged';
  if (significant) {
    status = rcDelta > 0 ? 'slower' : 'faster';
  }

  return {
    name: current.name,
    current,
    baseline,
    renderCountDelta: rcDelta,
    durationDelta: durDelta,
    significant,
    status,
  };
}

// ── Report formatting ────────────────────────────────────────────────────────

function fmtDelta(delta: number | null, unit: string = ''): string {
  if (delta === null) return 'N/A';
  const sign = delta > 0 ? '+' : '';
  return `${sign}${delta.toFixed(1)}${unit}`;
}

const STATUS_ICON: Record<ComparisonResult['status'], string> = {
  faster: '(improved)',
  slower: '(REGRESSION)',
  unchanged: '(unchanged)',
  new: '(new)',
};

export function formatReport(comparisons: ComparisonResult[]): string {
  const lines: string[] = ['Render Performance Comparison', '='.repeat(40)];

  for (const c of comparisons) {
    const icon = STATUS_ICON[c.status];
    lines.push('');
    lines.push(`${c.name} ${icon}`);
    lines.push('-'.repeat(40));

    if (c.baseline) {
      lines.push(
        `  Render count: ${c.current.meanRenderCount.toFixed(1)} ` +
          `(was ${c.baseline.meanRenderCount.toFixed(1)}, ` +
          `delta ${fmtDelta(c.renderCountDelta)})`
      );
      lines.push(
        `  Duration:     ${c.current.meanDuration.toFixed(1)}ms ` +
          `(was ${c.baseline.meanDuration.toFixed(1)}ms, ` +
          `delta ${fmtDelta(c.durationDelta, 'ms')})`
      );
    } else {
      lines.push(`  Render count: ${c.current.meanRenderCount.toFixed(1)}`);
      lines.push(`  Duration:     ${c.current.meanDuration.toFixed(1)}ms`);
    }
  }

  const regressions = comparisons.filter((c) => c.status === 'slower');
  if (regressions.length > 0) {
    lines.push('');
    lines.push(`WARNING: ${regressions.length} regression(s) detected!`);
  }

  return lines.join('\n');
}

/**
 * Format the comparison report as a Markdown table suitable for PR comments.
 *
 * @public
 */
export function formatMarkdownReport(comparisons: ComparisonResult[]): string {
  const lines: string[] = [
    '## Render Performance Report',
    '',
    '| Scenario | Renders | Delta | Duration | Delta | Status |',
    '|----------|---------|-------|----------|-------|--------|',
  ];

  for (const c of comparisons) {
    const rc = c.current.meanRenderCount.toFixed(1);
    const rcD = fmtDelta(c.renderCountDelta);
    const dur = `${c.current.meanDuration.toFixed(1)}ms`;
    const durD = fmtDelta(c.durationDelta, 'ms');
    const status = STATUS_ICON[c.status];
    lines.push(`| ${c.name} | ${rc} | ${rcD} | ${dur} | ${durD} | ${status} |`);
  }

  const regressions = comparisons.filter((c) => c.status === 'slower');
  if (regressions.length > 0) {
    lines.push('');
    lines.push(`> **Warning:** ${regressions.length} performance regression(s) detected.`);
  }

  return lines.join('\n');
}
