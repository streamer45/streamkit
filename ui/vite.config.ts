// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '');
  const apiUrl = env.SK_SERVER__ADDRESS || '127.0.0.1:4545';

  return {
    base: './', // Use relative paths for assets (required for subpath deployments)
    plugins: [
      react({
        babel: {
          plugins: ['babel-plugin-react-compiler'],
        },
      }),
    ],
    resolve: {
      alias: {
        '@': path.resolve(__dirname, './src'),
      },
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
