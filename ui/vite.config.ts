// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { defineConfig, loadEnv } from 'vite';
import react, { reactCompilerPreset } from '@vitejs/plugin-react';
import babel from '@rolldown/plugin-babel';
import path from 'path';
import { reactCompilerOptions } from './reactCompilerOptions';

export default defineConfig(({ mode }) => {
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
        presets: [reactCompilerPreset(reactCompilerOptions)],
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
