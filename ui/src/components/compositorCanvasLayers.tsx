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
import React, { useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react';

import { layerAtoms, textOverlayAtoms, imageOverlayAtoms } from '@/hooks/compositorAtoms';
import type { ResizeHandle } from '@/hooks/useCompositorLayers';
import { friendlyLabel } from '@/nodes/compositorNodeParts';
import { fontFamilyForAsset } from '@/services/fontAssets';

import {
  LayerBox,
  LayerDimensions,
  LayerLabel,
  ResizeHandleDiv,
  TextContent,
} from './compositorCanvasStyles';

/** Golden-angle-based hue to maximise visual separation between layers */
export function layerHue(index: number): number {
  return (index * 137.508) % 360;
}

/** CSS fallback stacks for fonts that haven't been loaded via @font-face yet. */
const FONT_FALLBACK_MAP: Record<string, string> = {
  'samples/fonts/system/DejaVuSans.ttf': '"DejaVu Sans", "Verdana", sans-serif',
  'samples/fonts/system/DejaVuSerif.ttf': '"DejaVu Serif", "Georgia", serif',
  'samples/fonts/system/DejaVuSansMono.ttf': '"DejaVu Sans Mono", "Courier New", monospace',
  'samples/fonts/system/DejaVuSans-Bold.ttf': '"DejaVu Sans", "Verdana", sans-serif',
  'samples/fonts/system/DejaVuSerif-Bold.ttf': '"DejaVu Serif", "Georgia", serif',
  'samples/fonts/system/DejaVuSansMono-Bold.ttf': '"DejaVu Sans Mono", "Courier New", monospace',
};

export function isBoldFont(fontName: string): boolean {
  return fontName.includes('-Bold') || fontName.includes('Bold');
}

/**
 * Return the CSS `font-family` value for a font asset path.
 *
 * Uses the `@font-face` family registered by {@link loadFontAssets} when
 * available, with a static fallback stack for known system fonts.
 */
export function cssFontFamily(fontName: string): string {
  // Primary: the custom @font-face family loaded from the server asset.
  const custom = fontFamilyForAsset(fontName);
  const fallback = FONT_FALLBACK_MAP[fontName] ?? 'sans-serif';
  return `"${custom}", ${fallback}`;
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
    outlineOffset: '0px',
    background: opts.bgColor,
    filter: opts.visible ? undefined : 'grayscale(0.6)',
  };
}

const HANDLES: ResizeHandle[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

/** Handles available for text overlays — excludes pure-vertical `n`/`s`
 *  handles because text font scaling is width-based; resizing only the
 *  height would produce an oversized box with unchanged text. */
const TEXT_HANDLES: ResizeHandle[] = ['nw', 'ne', 'e', 'se', 'sw', 'w'];

export const ResizeHandles: React.FC<{
  layerId: string;
  onResizeStart: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  handles?: ResizeHandle[];
}> = React.memo(({ layerId, onResizeStart, handles }) => (
  <>
    {(handles ?? HANDLES).map((h) => (
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

  // Circle crop indicator dimensions — computed once, used in JSX below.
  const circleDiameter = layer.cropShape === 'circle' ? Math.min(layer.width, layer.height) : 0;

  return (
    <LayerBox
      ref={layerRef}
      className="nodrag nopan"
      aria-label={`Video layer: ${layer.id}`}
      style={layerBoxStyle(layer.x, layer.y, layer.width, layer.height, {
        visible: layer.visible,
        opacity: layer.opacity,
        zIndex: layer.zIndex,
        rotationDegrees: layer.rotationDegrees,
        borderColor,
        bgColor: layer.cropShape === 'circle' ? 'transparent' : bgColor,
        outlineStyle: layer.cropShape === 'circle' ? 'dashed' : layer.visible ? 'solid' : 'dashed',
      })}
      onPointerDown={handlePointerDown}
    >
      {layer.cropShape === 'circle' && (
        <div
          data-crop-circle
          style={{
            position: 'absolute',
            inset: 0,
            clipPath: `circle(${circleDiameter / 2}px at 50% 50%)`,
            background: bgColor,
            pointerEvents: 'none',
          }}
        />
      )}
      {layer.cropShape === 'circle' && isSelected && (
        <div
          style={{
            position: 'absolute',
            width: circleDiameter,
            height: circleDiameter,
            top: '50%',
            left: '50%',
            transform: 'translate(-50%, -50%)',
            borderRadius: '50%',
            border: `2px dashed ${borderColor}`,
            pointerEvents: 'none',
          }}
        />
      )}
      <LayerLabel>{layer.id}</LayerLabel>
      <LayerDimensions>
        {Math.round(layer.width)}&times;{Math.round(layer.height)}
      </LayerDimensions>
      {isSelected && <ResizeHandles layerId={layer.id} onResizeStart={onResizeStart} />}
    </LayerBox>
  );
});
VideoLayer.displayName = 'VideoLayer';

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
        style={{
          ...layerBoxStyle(overlay.x, overlay.y, displayWidth, displayHeight, {
            visible: overlay.visible,
            opacity: overlay.opacity,
            zIndex: overlay.zIndex,
            rotationDegrees: overlay.rotationDegrees,
            borderColor,
            bgColor,
            outlineStyle: 'dashed',
          }),
          cursor: 'grab',
        }}
        onPointerDown={handlePointerDown}
        onDoubleClick={handleDoubleClick}
      >
        <LayerLabel>{friendlyLabel(overlay.id, 'text', index)}</LayerLabel>
        <LayerDimensions>
          {Math.round(displayWidth)}&times;{Math.round(displayHeight)}
        </LayerDimensions>
        {isSelected && (
          <ResizeHandles
            layerId={overlay.id}
            onResizeStart={onResizeStart}
            handles={TEXT_HANDLES}
          />
        )}
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
            fontWeight: isBoldFont(overlay.fontName) ? 700 : 400,
            lineHeight: 1.2,
            whiteSpace: 'pre',
            marginTop: -overlay.fontSize * 0.1,
          }}
        >
          {overlay.text}
        </span>
        <TextContent
          style={{
            transform: contentMirrorTransform(overlay.mirrorHorizontal, overlay.mirrorVertical),
          }}
        >
          <span
            style={{
              fontSize: overlay.fontSize,
              color: textColor,
              fontFamily: cssFontFamily(overlay.fontName),
              fontWeight: isBoldFont(overlay.fontName) ? 700 : 400,
              textShadow: '0 1px 3px rgba(0,0,0,0.7)',
              lineHeight: 1.2,
              whiteSpace: 'pre',
              // CSS line-height: 1.2 adds (1.2-1)/2 = 0.1em of half-leading
              // above the first line.  The server renders glyphs from origin
              // y=0 with no leading, so pull the text up to match.
              marginTop: -overlay.fontSize * 0.1,
              // When the server provides measured text dimensions, apply CSS
              // transforms so the browser-rendered text matches the
              // server's fontdue measurements pixel-precisely.
              transform: (() => {
                const parts: string[] = [];
                if (overlay.measuredTextWidth && browserTextSize.w > 0) {
                  parts.push(`scaleX(${overlay.measuredTextWidth / browserTextSize.w})`);
                }
                if (overlay.measuredTextHeight && browserTextSize.h > 0) {
                  parts.push(`scaleY(${overlay.measuredTextHeight / browserTextSize.h})`);
                }
                return parts.length > 0 ? parts.join(' ') : undefined;
              })(),
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

  const imgSrc = useMemo(() => {
    if (!overlay?.assetPath) return undefined;

    const parts = overlay.assetPath.split('/');
    const filename = parts.pop() ?? '';
    const scope = parts.pop() ?? 'user';
    return `/api/v1/assets/images/file/${encodeURIComponent(scope)}/${encodeURIComponent(filename)}`;
  }, [overlay?.assetPath]);

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
            transform: contentMirrorTransform(overlay.mirrorHorizontal, overlay.mirrorVertical),
          }}
        />
      )}
      <LayerLabel>{friendlyLabel(overlay.id, 'image', index)}</LayerLabel>
      {isSelected && <ResizeHandles layerId={overlay.id} onResizeStart={onResizeStart} />}
    </LayerBox>
  );
});
ImageOverlayLayer.displayName = 'ImageOverlayLayer';
