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

import { useEffect, useState } from 'react';

export const useCanvasAspectRatio = (
  canvas: HTMLCanvasElement | null | undefined
): string | undefined => {
  const [ratio, setRatio] = useState<string | undefined>(() => {
    if (!canvas) return undefined;
    const w = canvas.width;
    const h = canvas.height;
    return w > 0 && h > 0 ? `${w} / ${h}` : undefined;
  });

  useEffect(() => {
    if (!canvas) {
      setRatio(undefined);
      return;
    }

    const update = () => {
      const w = canvas.width;
      const h = canvas.height;
      if (w > 0 && h > 0) {
        setRatio(`${w} / ${h}`);
      }
    };

    // Read current values immediately.
    update();

    // Watch for attribute changes made by the Hang renderer.
    const observer = new MutationObserver(update);
    observer.observe(canvas, { attributes: true, attributeFilter: ['width', 'height'] });

    return () => observer.disconnect();
  }, [canvas]);

  return ratio;
};
