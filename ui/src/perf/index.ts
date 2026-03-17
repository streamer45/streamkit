// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Runtime render-performance profiler (Layer 2).
 *
 * Import `perfOnRender` and pass it to `<React.Profiler>` in dev builds
 * to collect render metrics that Playwright can read via
 * `window.__PERF_DATA__`.
 *
 * @example
 * ```tsx
 * import { Profiler } from 'react';
 * import { perfOnRender } from '@/perf';
 *
 * function MyComponent() {
 *   return (
 *     <Profiler id="MyComponent" onRender={perfOnRender}>
 *       <Inner />
 *     </Profiler>
 *   );
 * }
 * ```
 */

/** @public — re-exports for Playwright e2e tests */
export { perfOnRender, resetPerfData, getPerfData } from './profiler';
/** @public — re-exports for Playwright e2e tests */
export type { PerfCommit, PerfComponentData, PerfDataStore } from './profiler';
