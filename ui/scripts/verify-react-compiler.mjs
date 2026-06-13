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
 * Two independent layers, both driven by the *real* vite.config.ts, so the
 * misconfiguration can never ship silently again:
 *   1. dev transform — names exactly which perf-critical components regressed.
 *   2. production `vite build` — proves the compiler runs in the shipped bundle,
 *      not just the dev server (the bug disabled both, but only the build is
 *      what users get).
 */
import { build, createServer } from 'vite';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const uiRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const viteConfig = path.join(uiRoot, 'vite.config.ts');

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

// Unminified dev output names the compiler's memo cache `_c(<size>)` (the `c`
// export of react/compiler-runtime) in every component it optimizes.
const DEV_MEMO_CACHE = /[^.\w]_c\(\d+\)/;

// Minification renames `_c` but preserves the structural memo guard `[<n>]!==`
// it generates for each cached value. An uncompiled build emits only the few
// dozen guards that ship pre-compiled inside node_modules; a compiled one emits
// thousands, so this threshold is far from either regime.
const BUILD_GUARD = /\]\d*\]?!==/g;
const MIN_BUILD_GUARDS = 500;

const errors = [];

const server = await createServer({
  configFile: viteConfig,
  root: uiRoot,
  server: { middlewareMode: true },
  optimizeDeps: { noDiscovery: true },
  logLevel: 'error',
});

const uncompiled = [];
for (const rel of CRITICAL) {
  let code = '';
  try {
    code = (await server.transformRequest('/' + rel))?.code ?? '';
  } catch (err) {
    uncompiled.push(`${rel} (transform failed: ${err.message})`);
    continue;
  }
  if (!DEV_MEMO_CACHE.test(code)) uncompiled.push(rel);
}
if (uncompiled.length > 0) {
  errors.push(
    `Dev transform: React Compiler did not optimize ${uncompiled.length}/${CRITICAL.length} critical component(s):\n` +
      uncompiled.map((f) => `    - ${f}`).join('\n')
  );
}

const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'verify-react-compiler-'));
try {
  await build({
    configFile: viteConfig,
    root: uiRoot,
    logLevel: 'error',
    build: { outDir, emptyOutDir: true, write: true },
  });
  let guards = 0;
  const stack = [outDir];
  while (stack.length > 0) {
    const dir = stack.pop();
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) stack.push(full);
      else if (entry.name.endsWith('.js')) {
        guards += (fs.readFileSync(full, 'utf8').match(BUILD_GUARD) ?? []).length;
      }
    }
  }
  if (guards < MIN_BUILD_GUARDS) {
    errors.push(
      `Production build: only ${guards} React Compiler memo guards in the bundle ` +
        `(expected >= ${MIN_BUILD_GUARDS}); the compiler is not running in \`vite build\`.`
    );
  }
} catch (err) {
  errors.push(`Production build failed: ${err.message}`);
} finally {
  fs.rmSync(outDir, { recursive: true, force: true });
}

// server.close() hangs in middlewareMode (dep optimizer + watcher), so exit
// explicitly instead of awaiting it.
if (errors.length > 0) {
  console.error('\n' + errors.join('\n\n'));
  console.error(
    '\nThe compiler must run as a separate `babel({ presets: [reactCompilerPreset()] })`\n' +
      'plugin in ui/vite.config.ts. @vitejs/plugin-react v6 ignores `react({ babel })`.\n'
  );
  process.exit(1);
}

console.log(
  `React Compiler verified: ${CRITICAL.length} critical components optimized (dev transform) ` +
    `and memo guards present in the production build.`
);
process.exit(0);
