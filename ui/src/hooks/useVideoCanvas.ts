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

import { useCallback, useEffect, useRef, useState } from 'react';

import { useCanvasAspectRatio } from '@/hooks/useCanvasAspectRatio';

/** Minimal shape of the renderer expected by the hook. */
interface CanvasSetter {
  canvas: { set(el: HTMLCanvasElement): void };
}

export function useVideoCanvas(renderer: CanvasSetter | null | undefined) {
  const [canvasEl, setCanvasEl] = useState<HTMLCanvasElement | null>(null);
  const canvasElRef = useRef<HTMLCanvasElement | null>(null);
  const aspectRatio = useCanvasAspectRatio(canvasEl);

  const canvasRef = useCallback(
    (el: HTMLCanvasElement | null) => {
      canvasElRef.current = el;
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
    const el = canvasElRef.current;
    if (!renderer && el) {
      const ctx = el.getContext('2d');
      if (ctx) {
        ctx.clearRect(0, 0, el.width, el.height);
      }
      // Reset intrinsic size so the aspect-ratio hook also resets.
      el.width = 0;
      el.height = 0;
    }
  }, [renderer, canvasEl]);

  return { canvasRef, aspectRatio } as const;
}
