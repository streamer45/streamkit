// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook to track a canvas element's intrinsic aspect ratio.
 *
 * The Hang video renderer sets `canvas.width` / `canvas.height` to match the
 * decoded video dimensions.  This hook observes those attribute changes via
 * MutationObserver and returns a CSS `aspect-ratio` string (e.g. `"640 / 480"`)
 * so the layout always matches the actual stream, rather than relying on a
 * hardcoded ratio.
 */

import { useCallback, useSyncExternalStore } from 'react';

const readRatio = (canvas: HTMLCanvasElement | null | undefined): string | undefined => {
  if (!canvas) return undefined;
  const wAttr = canvas.getAttribute('width');
  const hAttr = canvas.getAttribute('height');
  if (wAttr === null || hAttr === null) return undefined;
  const w = canvas.width;
  const h = canvas.height;
  return w > 0 && h > 0 ? `${w} / ${h}` : undefined;
};

export const useCanvasAspectRatio = (
  canvas: HTMLCanvasElement | null | undefined
): string | undefined => {
  // Watch for attribute changes made by the Hang renderer.
  const subscribe = useCallback(
    (onChange: () => void) => {
      if (!canvas) return () => {};
      const observer = new MutationObserver(onChange);
      observer.observe(canvas, { attributes: true, attributeFilter: ['width', 'height'] });
      return () => observer.disconnect();
    },
    [canvas]
  );

  return useSyncExternalStore(subscribe, () => readRatio(canvas));
};
