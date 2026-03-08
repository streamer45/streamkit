// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Visual canvas for the video compositor node.
 *
 * Renders a scaled representation of the compositor output with draggable,
 * resizable layer boxes. Each layer type (video input, text overlay, image
 * overlay) is rendered differently:
 *
 * - Video input layers: colored rectangles with a unique hue per layer
 * - Text overlays: render the actual text content at scaled font size
 * - Image overlays: rectangles with an image icon and crosshatch pattern
 *
 * All interaction is handled via pointer events; visual updates during drag
 * use direct DOM manipulation (refs) so React never re-renders mid-interaction.
 */

import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';

import type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
  ResizeHandle,
} from '@/hooks/useCompositorLayers';

import {
  CanvasInner,
  CanvasOuter,
  EmptyState,
  LayerBox,
  LayerDimensions,
  LayerLabel,
  ResizeHandleDiv,
  TextContent,
} from './compositorCanvasStyles';

// ── Hue generation ──────────────────────────────────────────────────────────

/** Golden-angle-based hue to maximise visual separation between layers */
function layerHue(index: number): number {
  return (index * 137.508) % 360;
}

// ── Font mapping ────────────────────────────────────────────────────────────

const FONT_FAMILY_MAP: Record<string, string> = {
  'dejavu-sans': '"DejaVu Sans", "Verdana", sans-serif',
  'dejavu-serif': '"DejaVu Serif", "Georgia", serif',
  'dejavu-sans-mono': '"DejaVu Sans Mono", "Courier New", monospace',
  'dejavu-sans-bold': '"DejaVu Sans", "Verdana", sans-serif',
  'dejavu-serif-bold': '"DejaVu Serif", "Georgia", serif',
  'dejavu-sans-mono-bold': '"DejaVu Sans Mono", "Courier New", monospace',
};

function isBoldFont(fontName: string): boolean {
  return fontName.endsWith('-bold');
}

function cssFontFamily(fontName: string): string {
  return FONT_FAMILY_MAP[fontName] ?? 'sans-serif';
}

/** Build a short display label from a stable overlay id.
 *  E.g. "text · a3f2" or "img · 91cb" */
function overlayLabel(id: string, kind: 'text' | 'img'): string {
  const short = id.length > 8 ? id.slice(0, 4) : id;
  return `${kind} · ${short}`;
}

/** Build a CSS transform string combining rotation and mirror flips. */
function layerTransform(
  rotationDegrees: number,
  mirrorHorizontal: boolean,
  mirrorVertical: boolean
): string | undefined {
  const parts: string[] = [];
  if (rotationDegrees !== 0) parts.push(`rotate(${rotationDegrees}deg)`);
  if (mirrorHorizontal) parts.push('scaleX(-1)');
  if (mirrorVertical) parts.push('scaleY(-1)');
  return parts.length > 0 ? parts.join(' ') : undefined;
}

/** Compute the common style properties for a layer box. */
function layerBoxStyle(
  x: number,
  y: number,
  width: number,
  height: number,
  opts: {
    visible: boolean;
    opacity: number;
    zIndex: number;
    rotationDegrees: number;
    mirrorHorizontal: boolean;
    mirrorVertical: boolean;
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
    transform: layerTransform(opts.rotationDegrees, opts.mirrorHorizontal, opts.mirrorVertical),
    zIndex: opts.zIndex,
    outline: `2px ${opts.outlineStyle ?? 'solid'} ${opts.borderColor}`,
    outlineOffset: '-2px',
    background: opts.bgColor,
    filter: opts.visible ? undefined : 'grayscale(0.6)',
  };
}

// ── Resize handles ──────────────────────────────────────────────────────────

const HANDLES: ResizeHandle[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

const ResizeHandles: React.FC<{
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

const VideoLayer: React.FC<{
  layer: LayerState;
  index: number;
  isSelected: boolean;
  onPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onResizeStart: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  layerRef: (el: HTMLDivElement | null) => void;
}> = React.memo(({ layer, index, isSelected, onPointerDown, onResizeStart, layerRef }) => {
  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      onPointerDown(layer.id, e);
    },
    [layer.id, onPointerDown]
  );

  const hue = layerHue(index);
  const borderColor = isSelected ? 'var(--sk-primary)' : `hsla(${hue}, 70%, 65%, 0.8)`;
  const bgColor = isSelected ? `hsla(${hue}, 60%, 50%, 0.25)` : `hsla(${hue}, 60%, 50%, 0.15)`;

  return (
    <LayerBox
      ref={layerRef}
      className="nodrag nopan"
      style={layerBoxStyle(layer.x, layer.y, layer.width, layer.height, {
        visible: layer.visible,
        opacity: layer.opacity,
        zIndex: layer.zIndex + 1,
        rotationDegrees: layer.rotationDegrees,
        mirrorHorizontal: layer.mirrorHorizontal,
        mirrorVertical: layer.mirrorVertical,
        borderColor,
        bgColor,
        outlineStyle: layer.visible ? 'solid' : 'dashed',
      })}
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

const TextOverlayLayer: React.FC<{
  overlay: TextOverlayState;
  index: number;
  isSelected: boolean;
  onPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onResizeStart: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  onTextFocusRequest?: (id: string) => void;
  layerRef: (el: HTMLDivElement | null) => void;
}> = React.memo(
  ({ overlay, index, isSelected, onPointerDown, onResizeStart, onTextFocusRequest, layerRef }) => {
    // Measure the browser-rendered text dimensions with a hidden span.
    // No word wrapping — only explicit newlines break lines, so the
    // natural text width is deterministic and the box auto-sizes to fit.
    const measureRef = useRef<HTMLSpanElement>(null);
    const [browserTextSize, setBrowserTextSize] = useState({ w: 0, h: 0 });
    useLayoutEffect(() => {
      if (measureRef.current) {
        // offsetWidth / offsetHeight are in the element's own CSS-pixel
        // coordinate space, unaffected by ancestor transforms (the canvas
        // scale).  getBoundingClientRect() would return viewport-scaled
        // values and cause the box to be too small.
        setBrowserTextSize({
          w: measureRef.current.offsetWidth,
          h: measureRef.current.offsetHeight,
        });
      }
    }, [overlay.text, overlay.fontSize, overlay.fontName]);

    const handlePointerDown = useCallback(
      (e: React.PointerEvent) => {
        onPointerDown(overlay.id, e);
      },
      [overlay.id, onPointerDown]
    );

    const handleDoubleClick = useCallback(
      (e: React.MouseEvent) => {
        e.stopPropagation();
        e.preventDefault();
        onTextFocusRequest?.(overlay.id);
      },
      [onTextFocusRequest, overlay.id]
    );

    const hue = layerHue(index + 100); // offset from video layers
    const borderColor = isSelected ? 'var(--sk-primary)' : `hsla(${hue}, 70%, 65%, 0.8)`;
    const bgColor = isSelected ? `hsla(${hue}, 60%, 50%, 0.25)` : `hsla(${hue}, 60%, 50%, 0.12)`;

    const [r, g, b, a] = overlay.color;
    const textColor = `rgba(${r}, ${g}, ${b}, ${(a ?? 255) / 255})`;

    // Auto-size to the natural text dimensions.  Server measurement takes
    // priority; browser measurement is the fallback.
    const displayWidth = overlay.measuredTextWidth ?? browserTextSize.w ?? overlay.width;
    const displayHeight = overlay.measuredTextHeight ?? browserTextSize.h ?? overlay.height;

    return (
      <LayerBox
        ref={layerRef}
        className="nodrag nopan"
        style={layerBoxStyle(overlay.x, overlay.y, displayWidth, displayHeight, {
          visible: overlay.visible,
          opacity: overlay.opacity,
          zIndex: overlay.zIndex ?? 100 + index,
          rotationDegrees: overlay.rotationDegrees,
          mirrorHorizontal: overlay.mirrorHorizontal,
          mirrorVertical: overlay.mirrorVertical,
          borderColor,
          bgColor,
          outlineStyle: 'dashed',
        })}
        onPointerDown={handlePointerDown}
        onDoubleClick={handleDoubleClick}
      >
        <LayerLabel>{overlayLabel(overlay.id, 'text')}</LayerLabel>
        <LayerDimensions>
          {Math.round(displayWidth)}&times;{Math.round(displayHeight)}
        </LayerDimensions>
        {isSelected && <ResizeHandles layerId={overlay.id} onResizeStart={onResizeStart} />}
        {/* Hidden measurement span — no wrapping (white-space: pre) so
          offsetWidth / offsetHeight reflect the natural text extent.
          Text only breaks on explicit newlines. */}
        <span
          ref={measureRef}
          aria-hidden
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
        <TextContent>
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

const ImageOverlayLayer: React.FC<{
  overlay: ImageOverlayState;
  index: number;
  isSelected: boolean;
  onPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onResizeStart: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  layerRef: (el: HTMLDivElement | null) => void;
}> = React.memo(({ overlay, index, isSelected, onPointerDown, onResizeStart, layerRef }) => {
  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      onPointerDown(overlay.id, e);
    },
    [overlay.id, onPointerDown]
  );

  const hue = layerHue(index + 200); // offset from text overlays
  const borderColor = isSelected ? 'var(--sk-primary)' : `hsla(${hue}, 70%, 65%, 0.8)`;
  const bgColor = isSelected ? `hsla(${hue}, 60%, 50%, 0.25)` : `hsla(${hue}, 60%, 50%, 0.12)`;

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
    if (!overlay.dataBase64) {
      setImgSrc(undefined);
      return;
    }
    let mime = 'image/jpeg'; // default fallback
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
  }, [overlay.dataBase64]);

  return (
    <LayerBox
      ref={layerRef}
      className="nodrag nopan"
      style={layerBoxStyle(overlay.x, overlay.y, overlay.width, overlay.height, {
        visible: overlay.visible,
        opacity: overlay.opacity,
        zIndex: overlay.zIndex ?? 200 + index,
        rotationDegrees: overlay.rotationDegrees,
        mirrorHorizontal: overlay.mirrorHorizontal,
        mirrorVertical: overlay.mirrorVertical,
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
          }}
        />
      )}
      <LayerLabel>{overlayLabel(overlay.id, 'img')}</LayerLabel>
      {isSelected && <ResizeHandles layerId={overlay.id} onResizeStart={onResizeStart} />}
    </LayerBox>
  );
});
ImageOverlayLayer.displayName = 'ImageOverlayLayer';

// ── Main canvas ─────────────────────────────────────────────────────────────

export interface CompositorCanvasProps {
  canvasWidth: number;
  canvasHeight: number;
  layers: LayerState[];
  textOverlays?: TextOverlayState[];
  imageOverlays?: ImageOverlayState[];
  selectedLayerId: string | null;
  onSelectLayer: (id: string | null) => void;
  onLayerPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onResizePointerDown: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  onTextFocusRequest?: (id: string) => void;
  layerRefs: React.MutableRefObject<Map<string, HTMLDivElement>>;
  disabled?: boolean;
}

export const CompositorCanvas: React.FC<CompositorCanvasProps> = React.memo(
  ({
    canvasWidth,
    canvasHeight,
    layers,
    textOverlays = [],
    imageOverlays = [],
    selectedLayerId,
    onSelectLayer,
    onLayerPointerDown,
    onResizePointerDown,
    onTextFocusRequest,
    layerRefs,
    disabled,
  }) => {
    const outerRef = useRef<HTMLDivElement>(null);
    const [scale, setScale] = useState(1);

    // Recompute scale when container resizes
    useEffect(() => {
      const outer = outerRef.current;
      if (!outer) return;

      const observer = new ResizeObserver((entries) => {
        for (const entry of entries) {
          const containerWidth = entry.contentRect.width;
          if (containerWidth > 0 && canvasWidth > 0) {
            setScale(containerWidth / canvasWidth);
          }
        }
      });

      observer.observe(outer);
      return () => observer.disconnect();
    }, [canvasWidth]);

    // Blur any active element (e.g. inline text input) before deselecting
    // so that the input's onBlur → commitEdit fires reliably.
    const handlePaneClick = useCallback(() => {
      if (document.activeElement instanceof HTMLElement) {
        document.activeElement.blur();
      }
      onSelectLayer(null);
    }, [onSelectLayer]);

    // Cache ref-callbacks per layer id so React.memo on VideoLayer/TextOverlayLayer/
    // ImageOverlayLayer sees a stable function reference across renders.
    const layerRefCacheRef = useRef(new Map<string, (el: HTMLDivElement | null) => void>());
    const getLayerRef = useCallback(
      (layerId: string) => {
        let fn = layerRefCacheRef.current.get(layerId);
        if (!fn) {
          fn = (el: HTMLDivElement | null) => {
            if (el) {
              layerRefs.current.set(layerId, el);
            } else {
              layerRefs.current.delete(layerId);
            }
          };
          layerRefCacheRef.current.set(layerId, fn);
        }
        return fn;
      },
      [layerRefs]
    );

    const noopPointerDown = useCallback(() => {}, []);
    const noopResizeStart = useCallback(
      (() => {}) as (id: string, handle: ResizeHandle, e: React.PointerEvent) => void,
      []
    );

    const hasContent = layers.length > 0 || textOverlays.length > 0 || imageOverlays.length > 0;

    return (
      <CanvasOuter ref={outerRef} className="nodrag nopan">
        <CanvasInner
          data-canvas-width={canvasWidth}
          style={{
            width: canvasWidth,
            height: canvasHeight,
            transform: `scale(${scale})`,
            // CSS transform doesn't affect layout box — use negative margin
            // to collapse the unscaled height so the outer container fits tightly.
            marginBottom: canvasHeight * (scale - 1),
          }}
          onPointerDown={disabled ? undefined : handlePaneClick}
        >
          {!hasContent ? (
            <EmptyState>No layers configured</EmptyState>
          ) : (
            <>
              {layers.map((layer, i) => (
                <VideoLayer
                  key={layer.id}
                  layer={layer}
                  index={i}
                  isSelected={selectedLayerId === layer.id}
                  onPointerDown={disabled ? noopPointerDown : onLayerPointerDown}
                  onResizeStart={disabled ? noopResizeStart : onResizePointerDown}
                  layerRef={getLayerRef(layer.id)}
                />
              ))}
              {textOverlays.map((overlay, i) => (
                <TextOverlayLayer
                  key={overlay.id}
                  overlay={overlay}
                  index={i}
                  isSelected={selectedLayerId === overlay.id}
                  onPointerDown={disabled ? noopPointerDown : onLayerPointerDown}
                  onResizeStart={disabled ? noopResizeStart : onResizePointerDown}
                  onTextFocusRequest={disabled ? undefined : onTextFocusRequest}
                  layerRef={getLayerRef(overlay.id)}
                />
              ))}
              {imageOverlays.map((overlay, i) => (
                <ImageOverlayLayer
                  key={overlay.id}
                  overlay={overlay}
                  index={i}
                  isSelected={selectedLayerId === overlay.id}
                  onPointerDown={disabled ? noopPointerDown : onLayerPointerDown}
                  onResizeStart={disabled ? noopResizeStart : onResizePointerDown}
                  layerRef={getLayerRef(overlay.id)}
                />
              ))}
            </>
          )}
        </CanvasInner>
      </CanvasOuter>
    );
  }
);

CompositorCanvas.displayName = 'CompositorCanvas';
