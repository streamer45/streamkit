// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { defineConfig, type Plugin } from 'vitest/config';
import react, { reactCompilerPreset } from '@vitejs/plugin-react';
import { transformAsync } from '@babel/core';
import babelPresetTypescript from '@babel/preset-typescript';
import path from 'path';

// Production builds compile src with the React Compiler (reactCompilerPreset in
// vite.config.ts), but @vitejs/plugin-react only applies that preset to
// environments with consumer === 'client', and vitest runs the SSR pipeline.
// Force-apply the compiler here so tests exercise the same memoization regime
// as production. The compiler plugin and its options come from the same
// reactCompilerPreset() production uses, and the code pre-filter mirrors the
// preset's compile-candidate filter. Test/setup files are skipped because
// production never compiles them.
const compilerPreset = reactCompilerPreset().preset;

const reactCompilerForTests = (): Plugin => ({
  name: 'react-compiler-for-tests',
  enforce: 'pre',
  async transform(code, id) {
    const file = id.split('?')[0];
    if (
      !/\/src\/.*\.tsx?$/.test(file) ||
      file.includes('/node_modules/') ||
      /\.test\.tsx?$/.test(file) ||
      file.endsWith('/src/test/setup.ts') ||
      !/\b[A-Z]|\buse/.test(code)
    ) {
      return null;
    }
    const result = await transformAsync(code, {
      filename: file,
      babelrc: false,
      configFile: false,
      presets: [
        [babelPresetTypescript, { isTSX: file.endsWith('.tsx'), allExtensions: true }],
        compilerPreset,
      ],
      sourceMaps: true,
    });
    if (!result?.code) {
      return null;
    }
    return { code: result.code, map: result.map };
  },
});

export default defineConfig({
  plugins: [reactCompilerForTests(), react()],
  test: {
    environment: 'happy-dom',
    globals: false,
    setupFiles: ['./src/test/setup.ts'],
    exclude: [
      '**/node_modules/**',
      '**/.bun_install/**',
      '**/dist/**',
    ],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'json', 'lcov', 'html'],
      reportsDirectory: './coverage',
      exclude: [
        'node_modules/**',
        'src/test/**',
        '**/*.d.ts',
        '**/*.config.*',
        '**/mockData/**',
        'src/types/generated/**',
      ],
    },
  },
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
});
