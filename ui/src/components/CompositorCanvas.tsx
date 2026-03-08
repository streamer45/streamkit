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

import React, { useCallback, useEffect, useRef, useState } from 'react';

import type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
  ResizeHandle,
} from '@/hooks/useCompositorLayers';

import { ImageOverlayLayer, TextOverlayLayer, VideoLayer } from './compositorCanvasLayers';
import { CanvasInner, CanvasOuter, EmptyState } from './compositorCanvasStyles';

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
