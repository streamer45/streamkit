// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { defineConfig, loadEnv } from 'vite';
import react, { reactCompilerPreset } from '@vitejs/plugin-react';
import babel from '@rolldown/plugin-babel';
import fs from 'node:fs';
import path from 'path';
import { reactCompilerOptions } from './reactCompilerOptions';

// When VERIFY_COMPILER_LOG points at a file, attach the React Compiler's
// structured logger so it records a CompileSuccess event for every function it
// optimizes during the real build. The guard in scripts/verify-react-compiler.mjs
// reads this log to prove — per component, without parsing minified output —
// that the compiler actually ran on the perf-critical components. The variable
// is unset in normal dev/build, so there is zero cost or output then.
function compilerEventLogger(logPath: string) {
  fs.writeFileSync(logPath, '');
  return {
    logEvent(
      filename: string | null,
      event: { kind?: string; fnName?: string | null; memoSlots?: number } | null
    ) {
      const kind = event?.kind;
      if (kind === 'CompileSuccess') {
        fs.appendFileSync(
          logPath,
          JSON.stringify({
            filename,
            fnName: event?.fnName ?? null,
            memoSlots: typeof event?.memoSlots === 'number' ? event.memoSlots : null,
          }) + '\n'
        );
      }
    },
  };
}

export default defineConfig(({ mode }) => {
  const compilerLogPath = process.env.VERIFY_COMPILER_LOG;
  const compilerOptions = compilerLogPath
    ? { ...reactCompilerOptions, logger: compilerEventLogger(compilerLogPath) }
    : reactCompilerOptions;
  const env = loadEnv(mode, process.cwd(), '');
  const apiUrl = env.SK_SERVER__ADDRESS || '127.0.0.1:4545';
  const moqHangWorkletFix = () => ({
    name: 'moq-hang-worklet-fix',
    resolveId(id: string, importer?: string) {
      if (!importer) {
        return null;
      }

      if (!importer.includes('@moq/hang')) {
        return null;
      }

      if (id.startsWith('./') && id.includes('.ts?')) {
        const queryIndex = id.indexOf('?');
        const query = queryIndex === -1 ? '' : id.slice(queryIndex);
        const base = queryIndex === -1 ? id : id.slice(0, queryIndex);
        const resolved = path.resolve(path.dirname(importer.split('?')[0]), base)
          .replace(/\.ts$/, '.js');

        return `${resolved}${query}`;
      }

      return null;
    },
  });

  return {
    base: './', // Use relative paths for assets (required for subpath deployments)
    plugins: [
      react(),
      // @vitejs/plugin-react v6 is oxc-based and ignores its `babel` option, so
      // the React Compiler must run as a separate @rolldown/plugin-babel pass.
      babel({
        presets: [reactCompilerPreset(compilerOptions)],
      }),
      // @moq/hang publishes a JS worklet file but imports it as .ts.
      moqHangWorkletFix(),
    ],
    resolve: {
      alias: {
        '@': path.resolve(__dirname, './src'),
      },
      dedupe: [
        '@codemirror/state',
        '@codemirror/view',
        '@codemirror/language',
        '@codemirror/commands',
        '@codemirror/autocomplete',
        '@codemirror/lint',
        '@codemirror/search',
      ],
    },
    define: {
      'import.meta.env.VITE_WS_URL':
        mode === 'development'
          ? JSON.stringify(`ws://${apiUrl}/api/v1/control`)
          : undefined,
      // Only define VITE_API_BASE in development (for direct backend connection)
      // In production, leave undefined so getApiUrl() uses <base> tag for subpath support
      ...(mode === 'development' && {
        'import.meta.env.VITE_API_BASE': JSON.stringify(`http://${apiUrl}`),
      }),
    },
    server: {
      port: 3045,
      proxy: {
        // Proxy API requests to skit backend (enables E2E tests against dev server)
        '/api': {
          target: `http://${apiUrl}`,
          changeOrigin: true,
        },
        '/healthz': {
          target: `http://${apiUrl}`,
          changeOrigin: true,
        },
      },
    },
    optimizeDeps: {
      exclude: ['@moq/hang'],
    },
    build: {
      rollupOptions: {
        output: {
          manualChunks: () => {
            return undefined;
          },
        },
      },
    },
  };
});
