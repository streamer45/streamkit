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

import { useCallback, useState } from 'react';

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

  return { canvasRef, aspectRatio } as const;
}
