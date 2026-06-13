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
 * `react({ babel })`, disabling the compiler in production for months with no
 * error — every component shipped uncompiled.
 *
 * vite.config.ts attaches a logger to the compiler when VERIFY_COMPILER_LOG is
 * set, recording a CompileSuccess event per optimized function straight from
 * the real `vite build`. This reads that event log and asserts every perf-
 * critical component was optimized — an exact, per-component, minifier-
 * independent signal with no regexes or thresholds.
 *
 * CI sets VERIFY_COMPILER_LOG so the existing Build step emits the log and this
 * step just reads it (no second build). Run locally with the variable unset, it
 * builds once itself.
 */
import { build } from 'vite';
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

const WIRING_HELP =
  'The compiler must run as a separate `babel({ presets: [reactCompilerPreset()] })`\n' +
  'plugin in ui/vite.config.ts. @vitejs/plugin-react v6 ignores `react({ babel })`.';

// Temp artifacts created by a local build (never the CI-provided log), removed
// before every exit since process.exit() skips finally blocks.
const tempPaths = [];
function cleanup() {
  for (const p of tempPaths) fs.rmSync(p, { recursive: true, force: true });
}

function fail(message, { wiring = false } = {}) {
  cleanup();
  console.error('\n' + message);
  if (wiring) console.error('\n' + WIRING_HELP);
  process.exit(1);
}

// Reuse the event log the CI Build step already produced; otherwise build once
// here (local runs). A build() failure is reported as such, without the
// compiler-wiring banner — it is unrelated to how the compiler is wired.
let logPath = process.env.VERIFY_COMPILER_LOG;
const haveLog = logPath && fs.existsSync(logPath) && fs.statSync(logPath).size > 0;
if (!haveLog) {
  if (!logPath) {
    logPath = path.join(os.tmpdir(), `react-compiler-events-${process.pid}.ndjson`);
    tempPaths.push(logPath);
  }
  process.env.VERIFY_COMPILER_LOG = logPath;
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'verify-react-compiler-'));
  tempPaths.push(outDir);
  try {
    await build({
      configFile: viteConfig,
      root: uiRoot,
      logLevel: 'error',
      build: { outDir, emptyOutDir: true, write: true },
    });
  } catch (err) {
    fail(`Production build failed (unrelated to the React Compiler wiring):\n    ${err.message}`);
  }
}

let optimized;
try {
  optimized = fs
    .readFileSync(logPath, 'utf8')
    .split('\n')
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line).filename ?? '');
} catch (err) {
  fail(`Could not read the React Compiler event log at ${logPath}:\n    ${err.message}`);
}

// No CompileSuccess events at all means the compiler never ran — the exact
// `react({ babel })` regression this guard exists to catch.
if (optimized.length === 0) {
  fail('React Compiler emitted no CompileSuccess events — it is not running in `vite build`.', {
    wiring: true,
  });
}

const optimizedSet = new Set(optimized.map((f) => f.split('?')[0].replace(/\\/g, '/')));
// Anchor on a path separator so a longer suffix (e.g. XCompositorNode.tsx)
// cannot match a shorter CRITICAL entry (CompositorNode.tsx).
const wasOptimized = (rel) => {
  for (const f of optimizedSet) if (f === rel || f.endsWith('/' + rel)) return true;
  return false;
};

const problems = [];
for (const rel of CRITICAL) {
  if (!fs.existsSync(path.join(uiRoot, rel))) {
    problems.push(`${rel} — file not found; update CRITICAL in this script if it moved/renamed.`);
  } else if (!wasOptimized(rel)) {
    problems.push(`${rel} — no CompileSuccess event; the compiler did not optimize it.`);
  }
}

if (problems.length > 0) {
  fail(
    `React Compiler did not optimize ${problems.length}/${CRITICAL.length} perf-critical component(s):\n` +
      problems.map((p) => `    - ${p}`).join('\n')
  );
}

cleanup();
console.log(
  `React Compiler verified: all ${CRITICAL.length} perf-critical components optimized ` +
    `(${optimized.length} functions compiled in the production build).`
);
process.exit(0);
