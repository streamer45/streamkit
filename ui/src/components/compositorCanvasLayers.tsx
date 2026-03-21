// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Individual layer renderer components for the compositor canvas.
 *
 * Extracted from CompositorCanvas to keep each module under the max-lines
 * lint threshold while preserving identical runtime behaviour.
 */

import { useAtomValue } from 'jotai/react';
import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';

import { layerAtoms, textOverlayAtoms, imageOverlayAtoms } from '@/hooks/compositorAtoms';
import type { ResizeHandle } from '@/hooks/useCompositorLayers';
import { friendlyLabel } from '@/nodes/compositorNodeParts';

import {
  LayerBox,
  LayerDimensions,
  LayerLabel,
  ResizeHandleDiv,
  TextContent,
} from './compositorCanvasStyles';

// ── Hue generation ──────────────────────────────────────────────────────────

/** Golden-angle-based hue to maximise visual separation between layers */
export function layerHue(index: number): number {
  return (index * 137.508) % 360;
}

// ── Font mapping ────────────────────────────────────────────────────────────

export const FONT_FAMILY_MAP: Record<string, string> = {
  'dejavu-sans': '"DejaVu Sans", "Verdana", sans-serif',
  'dejavu-serif': '"DejaVu Serif", "Georgia", serif',
  'dejavu-sans-mono': '"DejaVu Sans Mono", "Courier New", monospace',
  'dejavu-sans-bold': '"DejaVu Sans", "Verdana", sans-serif',
  'dejavu-serif-bold': '"DejaVu Serif", "Georgia", serif',
  'dejavu-sans-mono-bold': '"DejaVu Sans Mono", "Courier New", monospace',
};

export function isBoldFont(fontName: string): boolean {
  return fontName.endsWith('-bold');
}

export function cssFontFamily(fontName: string): string {
  return FONT_FAMILY_MAP[fontName] ?? 'sans-serif';
}

/** Build a CSS transform string for rotation only.
 *  Mirror flips are applied to inner content elements instead of the
 *  LayerBox itself so that resize handles and labels stay in the correct
 *  orientation. */
export function layerTransform(rotationDegrees: number): string | undefined {
  return rotationDegrees !== 0 ? `rotate(${rotationDegrees}deg)` : undefined;
}

/** Build a CSS transform string for mirror flips, applied to content
 *  elements (images, text) inside a layer box. */
export function contentMirrorTransform(
  mirrorHorizontal: boolean,
  mirrorVertical: boolean
): string | undefined {
  const parts: string[] = [];
  if (mirrorHorizontal) parts.push('scaleX(-1)');
  if (mirrorVertical) parts.push('scaleY(-1)');
  return parts.length > 0 ? parts.join(' ') : undefined;
}

/** Compute the common style properties for a layer box. */
export function layerBoxStyle(
  x: number,
  y: number,
  width: number,
  height: number,
  opts: {
    visible: boolean;
    opacity: number;
    zIndex: number;
    rotationDegrees: number;
    borderColor: string;
    bgColor: string;
    outlineStyle?: 'solid' | 'dashed';
  }
): React.CSSProperties {
  return {
    left: x,
    top: y,
    width,
    height,
    opacity: opts.visible ? opts.opacity : 0.2,
    transform: layerTransform(opts.rotationDegrees),
    zIndex: opts.zIndex,
    outline: `2px ${opts.outlineStyle ?? 'solid'} ${opts.borderColor}`,
    outlineOffset: '-2px',
    background: opts.bgColor,
    filter: opts.visible ? undefined : 'grayscale(0.6)',
  };
}

// ── Resize handles ──────────────────────────────────────────────────────────

const HANDLES: ResizeHandle[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

export const ResizeHandles: React.FC<{
  layerId: string;
  onResizeStart: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
}> = React.memo(({ layerId, onResizeStart }) => (
  <>
    {HANDLES.map((h) => (
      <ResizeHandleDiv
        key={h}
        position={h}
        className="nodrag nopan"
        onPointerDown={(e) => onResizeStart(layerId, h, e)}
      />
    ))}
  </>
));
ResizeHandles.displayName = 'ResizeHandles';

// ── Video input layer ───────────────────────────────────────────────────────

export const VideoLayer: React.FC<{
  layerId: string;
  index: number;
  isSelected: boolean;
  onPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onResizeStart: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  layerRef: (el: HTMLDivElement | null) => void;
}> = React.memo(({ layerId, index, isSelected, onPointerDown, onResizeStart, layerRef }) => {
  const layer = useAtomValue(layerAtoms(layerId));

  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      onPointerDown(layerId, e);
    },
    [layerId, onPointerDown]
  );

  if (!layer) return null;

  const hue = layerHue(index);
  const borderColor = isSelected ? 'var(--sk-primary)' : `hsla(${hue}, 70%, 65%, 0.8)`;
  const bgColor = isSelected ? `hsla(${hue}, 60%, 50%, 0.25)` : `hsla(${hue}, 60%, 50%, 0.15)`;

  return (
    <LayerBox
      ref={layerRef}
      className="nodrag nopan"
      aria-label={`Video layer: ${layer.id}`}
      style={{
        ...layerBoxStyle(layer.x, layer.y, layer.width, layer.height, {
          visible: layer.visible,
          opacity: layer.opacity,
          zIndex: layer.zIndex,
          rotationDegrees: layer.rotationDegrees,
          borderColor,
          bgColor,
          outlineStyle: layer.visible ? 'solid' : 'dashed',
        }),
        borderRadius: layer.cropShape === 'circle' ? '50%' : undefined,
      }}
      onPointerDown={handlePointerDown}
    >
      <LayerLabel>{layer.id}</LayerLabel>
      <LayerDimensions>
        {Math.round(layer.width)}&times;{Math.round(layer.height)}
      </LayerDimensions>
      {isSelected && <ResizeHandles layerId={layer.id} onResizeStart={onResizeStart} />}
    </LayerBox>
  );
});
VideoLayer.displayName = 'VideoLayer';

// ── Text overlay layer ──────────────────────────────────────────────────────

export const TextOverlayLayer: React.FC<{
  overlayId: string;
  index: number;
  isSelected: boolean;
  onPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onResizeStart: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  onTextFocusRequest?: (id: string) => void;
  layerRef: (el: HTMLDivElement | null) => void;
}> = React.memo(
  ({
    overlayId,
    index,
    isSelected,
    onPointerDown,
    onResizeStart,
    onTextFocusRequest,
    layerRef,
  }) => {
    const overlay = useAtomValue(textOverlayAtoms(overlayId));

    // Measure the browser-rendered text dimensions with a hidden span.
    // No word wrapping — only explicit newlines break lines, so the
    // natural text width is deterministic and the box auto-sizes to fit.
    const measureRef = useRef<HTMLSpanElement>(null);
    const [browserTextSize, setBrowserTextSize] = useState({ w: 0, h: 0 });
    useLayoutEffect(() => {
      if (measureRef.current && overlay) {
        // offsetWidth / offsetHeight are in the element's own CSS-pixel
        // coordinate space, unaffected by ancestor transforms (the canvas
        // scale).  getBoundingClientRect() would return viewport-scaled
        // values and cause the box to be too small.
        setBrowserTextSize({
          w: measureRef.current.offsetWidth,
          h: measureRef.current.offsetHeight,
        });
      }
      // eslint-disable-next-line react-hooks/exhaustive-deps -- measure only when text/font changes, not the full overlay
    }, [overlay?.text, overlay?.fontSize, overlay?.fontName]);

    const handlePointerDown = useCallback(
      (e: React.PointerEvent) => {
        onPointerDown(overlayId, e);
      },
      [overlayId, onPointerDown]
    );

    const handleDoubleClick = useCallback(
      (e: React.MouseEvent) => {
        e.stopPropagation();
        e.preventDefault();
        onTextFocusRequest?.(overlayId);
      },
      [onTextFocusRequest, overlayId]
    );

    if (!overlay) return null;

    const hue = layerHue(index + 100); // offset from video layers
    const borderColor = isSelected ? 'var(--sk-primary)' : `hsla(${hue}, 70%, 65%, 0.8)`;
    const bgColor = isSelected ? `hsla(${hue}, 60%, 50%, 0.25)` : `hsla(${hue}, 60%, 50%, 0.12)`;

    const [r, g, b, a] = overlay.color;
    const textColor = `rgba(${r}, ${g}, ${b}, ${(a ?? 255) / 255})`;

    // Auto-size to the natural text dimensions.  Server measurement takes
    // priority; browser measurement is the fallback.  Guard against zero
    // values from the initial render (before useLayoutEffect has measured)
    // to prevent a visible flash at 0×0 size.
    const displayWidth = overlay.measuredTextWidth || browserTextSize.w || overlay.width;
    const displayHeight = overlay.measuredTextHeight || browserTextSize.h || overlay.height;

    return (
      <LayerBox
        ref={layerRef}
        className="nodrag nopan"
        aria-label={`Text overlay: ${friendlyLabel(overlay.id, 'text', index)}`}
        style={layerBoxStyle(overlay.x, overlay.y, displayWidth, displayHeight, {
          visible: overlay.visible,
          opacity: overlay.opacity,
          zIndex: overlay.zIndex,
          rotationDegrees: overlay.rotationDegrees,
          borderColor,
          bgColor,
          outlineStyle: 'dashed',
        })}
        onPointerDown={handlePointerDown}
        onDoubleClick={handleDoubleClick}
      >
        <LayerLabel>{friendlyLabel(overlay.id, 'text', index)}</LayerLabel>
        <LayerDimensions>
          {Math.round(displayWidth)}&times;{Math.round(displayHeight)}
        </LayerDimensions>
        {isSelected && <ResizeHandles layerId={overlay.id} onResizeStart={onResizeStart} />}
        {/* Hidden measurement span — no wrapping (white-space: pre) so
          offsetWidth / offsetHeight reflect the natural text extent.
          Text only breaks on explicit newlines. */}
        <span
          ref={measureRef}
          aria-hidden="true"
          style={{
            position: 'absolute',
            visibility: 'hidden',
            top: 0,
            left: 0,
            fontSize: overlay.fontSize,
            fontFamily: cssFontFamily(overlay.fontName),
            fontWeight: isBoldFont(overlay.fontName) ? 700 : 600,
            lineHeight: 1.2,
            whiteSpace: 'pre',
            marginTop: -overlay.fontSize * 0.1,
          }}
        >
          {overlay.text}
        </span>
        <TextContent
          style={{
            transform: contentMirrorTransform(
              overlay.mirrorHorizontal,
              overlay.mirrorVertical
            ),
          }}
        >
          <span
            style={{
              fontSize: overlay.fontSize,
              color: textColor,
              fontFamily: cssFontFamily(overlay.fontName),
              fontWeight: isBoldFont(overlay.fontName) ? 700 : 600,
              textShadow: '0 1px 3px rgba(0,0,0,0.7)',
              lineHeight: 1.2,
              whiteSpace: 'pre',
              // CSS line-height: 1.2 adds (1.2-1)/2 = 0.1em of half-leading
              // above the first line.  The server renders glyphs from origin
              // y=0 with no leading, so pull the text up to match.
              marginTop: -overlay.fontSize * 0.1,
              // When the server provides measured text dimensions, apply a
              // CSS transform so the browser-rendered text matches the
              // server's fontdue measurements pixel-precisely.
              transform:
                overlay.measuredTextWidth && browserTextSize.w > 0
                  ? `scaleX(${overlay.measuredTextWidth / browserTextSize.w})`
                  : undefined,
              transformOrigin: 'top left',
            }}
          >
            {overlay.text}
          </span>
        </TextContent>
      </LayerBox>
    );
  }
);
TextOverlayLayer.displayName = 'TextOverlayLayer';

// ── Image overlay layer ─────────────────────────────────────────────────────

export const ImageOverlayLayer: React.FC<{
  overlayId: string;
  index: number;
  isSelected: boolean;
  onPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onResizeStart: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  layerRef: (el: HTMLDivElement | null) => void;
}> = React.memo(({ overlayId, index, isSelected, onPointerDown, onResizeStart, layerRef }) => {
  const overlay = useAtomValue(imageOverlayAtoms(overlayId));

  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      onPointerDown(overlayId, e);
    },
    [overlayId, onPointerDown]
  );

  // Build a blob URL for the image thumbnail.  Using fetch() with a
  // data-URI lets the browser decode the base64 natively, which is
  // more efficient than the manual atob() + byte-by-byte Uint8Array
  // copy for large images.
  //
  // MIME detection: we inspect the base64-encoded magic bytes at the
  // start of the string to pick the correct MIME type.  The fallback
  // is JPEG, which covers the common `/9j/` prefix and variants.
  const [imgSrc, setImgSrc] = useState<string | undefined>();

  useEffect(() => {
    if (!overlay?.dataBase64) {
      setImgSrc(undefined);
      return;
    }
    let mime = 'image/jpeg'; // default fallback
    // MIME detection via base64 magic-byte prefixes.  Covers the most common
    // web formats; unrecognised formats (AVIF, BMP, TIFF, …) fall back to
    // JPEG which the browser may still decode correctly via content sniffing.
    if (overlay.dataBase64.startsWith('iVBOR')) mime = 'image/png';
    else if (overlay.dataBase64.startsWith('R0lGOD')) mime = 'image/gif';
    else if (overlay.dataBase64.startsWith('UklGR')) mime = 'image/webp';

    let cancelled = false;
    let url: string | undefined;

    fetch(`data:${mime};base64,${overlay.dataBase64}`)
      .then((r) => r.blob())
      .then((blob) => {
        if (cancelled) return;
        url = URL.createObjectURL(blob);
        setImgSrc(url);
      })
      .catch(() => {
        // Ignore decode failures — no thumbnail shown.
      });

    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [overlay?.dataBase64]);

  if (!overlay) return null;

  const hue = layerHue(index + 200); // offset from text overlays
  const borderColor = isSelected ? 'var(--sk-primary)' : `hsla(${hue}, 70%, 65%, 0.8)`;
  const bgColor = isSelected ? `hsla(${hue}, 60%, 50%, 0.25)` : `hsla(${hue}, 60%, 50%, 0.12)`;

  return (
    <LayerBox
      ref={layerRef}
      className="nodrag nopan"
      aria-label={`Image overlay: ${friendlyLabel(overlay.id, 'image', index)}`}
      style={layerBoxStyle(overlay.x, overlay.y, overlay.width, overlay.height, {
        visible: overlay.visible,
        opacity: overlay.opacity,
        zIndex: overlay.zIndex,
        rotationDegrees: overlay.rotationDegrees,
        borderColor,
        bgColor,
      })}
      onPointerDown={handlePointerDown}
    >
      {imgSrc && (
        <img
          src={imgSrc}
          alt={`Image overlay ${index}`}
          style={{
            position: 'absolute',
            inset: 0,
            width: '100%',
            height: '100%',
            objectFit: 'contain',
            pointerEvents: 'none',
            opacity: 0.85,
            transform: contentMirrorTransform(
              overlay.mirrorHorizontal,
              overlay.mirrorVertical
            ),
          }}
        />
      )}
      <LayerLabel>{friendlyLabel(overlay.id, 'image', index)}</LayerLabel>
      {isSelected && <ResizeHandles layerId={overlay.id} onResizeStart={onResizeStart} />}
    </LayerBox>
  );
});
ImageOverlayLayer.displayName = 'ImageOverlayLayer';
