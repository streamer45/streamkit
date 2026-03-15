// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook that wires a `<canvas>` element to a Hang video renderer and tracks
 * its intrinsic aspect ratio.
 *
 * Duplicated in `StreamView` and `OutputPreviewPanel` — extracted here so
 * both consumers share the same three-line pattern:
 *
 * ```ts
 * const { canvasRef, aspectRatio } = useVideoCanvas(videoRenderer);
 * ```
 */

import { useCallback, useEffect, useState } from 'react';

import { useCanvasAspectRatio } from '@/hooks/useCanvasAspectRatio';

/** Minimal shape of the renderer expected by the hook. */
interface CanvasSetter {
  canvas: { set(el: HTMLCanvasElement): void };
}

export function useVideoCanvas(renderer: CanvasSetter | null | undefined) {
  const [canvasEl, setCanvasEl] = useState<HTMLCanvasElement | null>(null);
  const aspectRatio = useCanvasAspectRatio(canvasEl);

  const canvasRef = useCallback(
    (el: HTMLCanvasElement | null) => {
      setCanvasEl(el);
      if (el && renderer) {
        renderer.canvas.set(el);
      }
    },
    [renderer]
  );

  // Clear canvas pixels when the renderer is removed (e.g. pipeline
  // destroyed / stream disconnected) so stale frames don't linger.
  useEffect(() => {
    if (!renderer && canvasEl) {
      const ctx = canvasEl.getContext('2d');
      if (ctx) {
        ctx.clearRect(0, 0, canvasEl.width, canvasEl.height);
      }
      // Reset intrinsic size so the aspect-ratio hook also resets.
      canvasEl.width = 0;
      canvasEl.height = 0;
    }
  }, [renderer, canvasEl]);

  return { canvasRef, aspectRatio } as const;
}
