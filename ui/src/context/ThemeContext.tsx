// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React, { createContext, useContext, useEffect, useState } from 'react';

export type ColorMode = 'dark' | 'light' | 'system';

interface ThemeContextValue {
  colorMode: ColorMode;
  setColorMode: (mode: ColorMode) => void;
}

const ThemeContext = createContext<ThemeContextValue | undefined>(undefined);

export const ThemeProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [colorMode, setColorMode] = useState<ColorMode>(() => {
    try {
      const saved = localStorage.getItem('skit-color-mode');
      if (saved === 'dark' || saved === 'light' || saved === 'system') {
        return saved;
      }
    } catch {
      // ignore
    }
    return 'dark';
  });

  useEffect(() => {
    try {
      localStorage.setItem('skit-color-mode', colorMode);
    } catch {
      // ignore
    }
  }, [colorMode]);

  useEffect(() => {
    const root = document.documentElement;
    if (colorMode === 'dark' || colorMode === 'light') {
      root.setAttribute('data-skit-theme', colorMode);
    } else {
      root.removeAttribute('data-skit-theme');
    }
  }, [colorMode]);

  const value = { colorMode, setColorMode };

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
};

export const useTheme = (): ThemeContextValue => {
  const ctx = useContext(ThemeContext);
  if (!ctx) {
    throw new Error('useTheme must be used within a ThemeProvider');
  }
  return ctx;
};
