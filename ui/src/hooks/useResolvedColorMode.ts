// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook to resolve the actual color mode (dark/light) from the theme context.
 *
 * This hook handles the "system" preference by checking the browser's
 * prefers-color-scheme media query and resolving it to either "dark" or "light".
 *
 * Use this hook wherever you need the actual resolved color mode rather than
 * the user's preference setting (which may be "system", "dark", or "light").
 */

import { useTheme } from '@/context/ThemeContext';

/**
 * Returns the resolved color mode as either 'dark' or 'light'
 * @returns 'dark' | 'light' - The actual resolved color mode
 */
export const useResolvedColorMode = (): 'dark' | 'light' => {
  const { colorMode: themeColorMode } = useTheme();

  if (themeColorMode === 'system') {
    // Check system preference
    return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  }

  return themeColorMode as 'dark' | 'light';
};
