#!/usr/bin/env node
// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Guards against the React Compiler silently not running in the real build.
 *
 * @vitejs/plugin-react v6 is oxc-based and ignores its `babel` option, so the
 * compiler only runs when wired as a separate @rolldown/plugin-babel pass in
 * vite.config.ts. A dependency bump once folded the preset back into
 * `react({ babel })`, which disabled the compiler in production for months
 * without any error — every component re-rendered uncompiled.
 *
 * This transforms a set of perf-critical components through the *real*
 * vite.config.ts and fails if any of them lacks the compiler's memo cache,
 * so the misconfiguration can never ship silently again.
 */
import { createServer } from 'vite';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const uiRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// The node-graph hot path (re-rendered on every slider tick) plus the heaviest
// views. If the compiler stops optimizing these, MonitorView/DesignView regress
// to full-subtree cascades on every parameter change.
const CRITICAL = [
  'src/views/MonitorView.tsx',
  'src/views/DesignView.tsx',
  'src/components/node/NodeFrame.tsx',
  'src/components/node/PinRow.tsx',
  'src/components/node/PinHandle.tsx',
  'src/nodes/AudioGainNode.tsx',
  'src/nodes/CompositorNode.tsx',
  'src/nodes/ConfigurableNode.tsx',
  'src/components/NodeStateIndicator.tsx',
  'src/panes/InspectorPane.tsx',
];

// The compiler allocates a memo cache via `_c(<size>)` (the `c` export of
// react/compiler-runtime) in every component it optimizes.
const MEMO_CACHE = /[^.\w]_c\(\d+\)/;

const server = await createServer({
  configFile: path.join(uiRoot, 'vite.config.ts'),
  root: uiRoot,
  server: { middlewareMode: true },
  optimizeDeps: { noDiscovery: true },
  logLevel: 'error',
});

const failures = [];
for (const rel of CRITICAL) {
  let code = '';
  try {
    const result = await server.transformRequest('/' + rel);
    code = result?.code ?? '';
  } catch (err) {
    failures.push(`${rel} (transform failed: ${err.message})`);
    continue;
  }
  if (!MEMO_CACHE.test(code)) {
    failures.push(rel);
  }
}

// server.close() hangs in middlewareMode (dep optimizer + watcher), so exit
// explicitly instead of awaiting it.
if (failures.length > 0) {
  console.error(
    `\nReact Compiler is NOT optimizing ${failures.length}/${CRITICAL.length} critical component(s):`,
  );
  for (const f of failures) console.error(`  - ${f}`);
  console.error(
    '\nThe compiler must run as a separate `babel({ presets: [reactCompilerPreset()] })`\n' +
      'plugin in ui/vite.config.ts. @vitejs/plugin-react v6 ignores `react({ babel })`.\n',
  );
  process.exit(1);
}

console.log(`React Compiler verified on ${CRITICAL.length} critical components.`);
process.exit(0);
