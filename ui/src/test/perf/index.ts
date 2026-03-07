// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Render-performance measurement framework for Vitest.
 *
 * Layer 1 of the two-layer profiling approach:
 *   - Layer 1 (this): Component-level render regression testing via Vitest.
 *   - Layer 2: Interaction-level profiling via Playwright + React.Profiler.
 *
 * @example
 * ```tsx
 * import { measureRenders } from '@/test/perf';
 *
 * test('slider renders efficiently', async () => {
 *   const result = await measureRenders(<MySlider value={50} />, {
 *     scenario: async (screen) => {
 *       await userEvent.click(screen.getByRole('slider'));
 *     },
 *   });
 *   expect(result.meanRenderCount).toBeLessThanOrEqual(3);
 * });
 * ```
 */

export { measureRenders, measureHookRenders } from './measure';
export type { MeasureRendersOptions, MeasureHookRendersOptions } from './measure';

export {
  readBaseline,
  writeBaseline,
  compare,
  formatReport,
  formatMarkdownReport,
} from './compare';

export type {
  RenderMeasurement,
  MeasureResult,
  BaselineEntry,
  BaselineFile,
  ComparisonResult,
} from './types';
