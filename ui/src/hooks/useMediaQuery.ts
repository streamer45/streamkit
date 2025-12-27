// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook to subscribe to a CSS media query.
 *
 * Useful for responsive UI behaviors that need to change React rendering
 * (e.g. swapping multi-pane layouts for a single-pane mobile layout).
 */

import { useEffect, useState } from 'react';

export const useMediaQuery = (query: string): boolean => {
  const getMatch = () => {
    if (typeof window === 'undefined') return false;
    return window.matchMedia(query).matches;
  };

  const [matches, setMatches] = useState<boolean>(() => getMatch());

  useEffect(() => {
    const mediaQueryList = window.matchMedia(query);

    const handleChange = () => {
      setMatches(mediaQueryList.matches);
    };

    handleChange();

    mediaQueryList.addEventListener('change', handleChange);
    return () => mediaQueryList.removeEventListener('change', handleChange);
  }, [query]);

  return matches;
};
