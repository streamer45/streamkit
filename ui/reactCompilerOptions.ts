// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { reactCompilerPreset } from '@vitejs/plugin-react';

// Single source of React Compiler options for both the production build
// (vite.config.ts) and the test pipeline (vitest.config.ts), so the two can
// never silently diverge.
export const reactCompilerOptions: Parameters<typeof reactCompilerPreset>[0] = {};
