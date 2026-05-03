// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Resolves "system" color mode preference to the actual dark/light value
 * by checking the browser's prefers-color-scheme media query.
 */

import { useTheme } from '@/context/ThemeContext';

export const useResolvedColorMode = (): 'dark' | 'light' => {
  const { colorMode: themeColorMode } = useTheme();

  if (themeColorMode === 'system') {
    return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  }

  return themeColorMode as 'dark' | 'light';
};
