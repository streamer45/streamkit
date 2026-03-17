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
  LayerKind,
  ResizeHandle,
} from '@/hooks/useCompositorLayers';

import { ImageOverlayLayer, TextOverlayLayer, VideoLayer } from './compositorCanvasLayers';
import { CanvasInner, CanvasOuter, EmptyState, SnapGuideLine } from './compositorCanvasStyles';

// Module-level no-op callbacks used when the canvas is disabled.
// Defined here rather than inside the component to avoid per-instance creation.
const noopPointerDown = () => {};
const noopResizeStart = (() => {}) as (
  id: string,
  handle: ResizeHandle,
  e: React.PointerEvent
) => void;

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
  onLayerContextMenu?: (layerId: string, layerKind: LayerKind, x: number, y: number) => void;
  layerRefs: React.MutableRefObject<Map<string, HTMLDivElement>>;
  snapGuideRefs: React.MutableRefObject<{
    vertical: HTMLDivElement | null;
    horizontal: HTMLDivElement | null;
  }>;
  disabled?: boolean;
}

/**
 * Custom comparator for CompositorCanvas memo.
 *
 * Video layers use zero-render DOM updates for opacity/rotation during
 * slider drags, so we only compare geometry fields for those.
 * Text and image overlays update via React state (not the zero-render path),
 * so we use reference equality — a new array reference means content changed.
 */
function areCanvasPropsEqual(prev: CompositorCanvasProps, next: CompositorCanvasProps): boolean {
  // Scalar props
  if (prev.canvasWidth !== next.canvasWidth) return false;
  if (prev.canvasHeight !== next.canvasHeight) return false;
  if (prev.selectedLayerId !== next.selectedLayerId) return false;
  if (prev.disabled !== next.disabled) return false;

  // Callback identity (stable via useCallback in parent)
  if (prev.onSelectLayer !== next.onSelectLayer) return false;
  if (prev.onLayerPointerDown !== next.onLayerPointerDown) return false;
  if (prev.onResizePointerDown !== next.onResizePointerDown) return false;
  if (prev.onTextFocusRequest !== next.onTextFocusRequest) return false;
  if (prev.onLayerContextMenu !== next.onLayerContextMenu) return false;

  // Ref identity (stable MutableRefObjects)
  if (prev.layerRefs !== next.layerRefs) return false;
  if (prev.snapGuideRefs !== next.snapGuideRefs) return false;

  // Video layers: compare geometry only, skip appearance (opacity, rotation, mirror)
  // since those use the zero-render DOM path
  if (!layerArrayGeometryEqual(prev.layers, next.layers)) return false;

  // Text/image overlays: use reference equality since they update via React state
  // and content changes (text, font, color, opacity, rotation) need to re-render
  if (prev.textOverlays !== next.textOverlays) return false;
  if (prev.imageOverlays !== next.imageOverlays) return false;

  return true;
}

/**
 * Compare layer arrays by geometry + mirror fields, skipping opacity/rotation
 * (those use the zero-render DOM path during slider drags).
 * Mirror is included because updateLayerMirror updates via React state, not DOM.
 */
function layerArrayGeometryEqual(a: readonly LayerState[], b: readonly LayerState[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const la = a[i];
    const lb = b[i];
    if (
      la.id !== lb.id ||
      la.x !== lb.x ||
      la.y !== lb.y ||
      la.width !== lb.width ||
      la.height !== lb.height ||
      la.zIndex !== lb.zIndex ||
      la.visible !== lb.visible ||
      la.mirrorHorizontal !== lb.mirrorHorizontal ||
      la.mirrorVertical !== lb.mirrorVertical
    ) {
      return false;
    }
  }
  return true;
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
    onLayerContextMenu,
    layerRefs,
    snapGuideRefs,
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
    const handlePaneClick = useCallback(
      (e: React.PointerEvent) => {
        if (e.button !== 0) return; // only primary (left) click deselects
        if (document.activeElement instanceof HTMLElement) {
          document.activeElement.blur();
        }
        onSelectLayer(null);
      },
      [onSelectLayer]
    );

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

    // Right-click context menu via event delegation: walk layerRefs to
    // find which layer element contains the event target, then determine
    // its kind from the layers/textOverlays/imageOverlays arrays.
    // When layers overlap, pick the one with the highest z-index.
    const handleCanvasContextMenu = useCallback(
      (e: React.MouseEvent) => {
        if (disabled || !onLayerContextMenu) return;
        const target = e.target as Node;
        let hitId: string | null = null;
        let hitZ = -1;
        for (const [id, el] of layerRefs.current) {
          if (el.contains(target)) {
            const z = Number(el.style.zIndex) || 0;
            if (z >= hitZ) {
              hitId = id;
              hitZ = z;
            }
          }
        }
        if (!hitId) return;
        e.preventDefault();
        onSelectLayer(hitId);
        let kind: LayerKind = 'video';
        if (textOverlays.some((o) => o.id === hitId)) kind = 'text';
        else if (imageOverlays.some((o) => o.id === hitId)) kind = 'image';
        onLayerContextMenu(hitId, kind, e.clientX, e.clientY);
      },
      [disabled, onLayerContextMenu, onSelectLayer, layerRefs, textOverlays, imageOverlays]
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
          onContextMenu={handleCanvasContextMenu}
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
          <SnapGuideLine
            data-axis="vertical"
            ref={(el: HTMLDivElement | null) => {
              snapGuideRefs.current.vertical = el;
            }}
          />
          <SnapGuideLine
            data-axis="horizontal"
            ref={(el: HTMLDivElement | null) => {
              snapGuideRefs.current.horizontal = el;
            }}
          />
        </CanvasInner>
      </CanvasOuter>
    );
  },
  areCanvasPropsEqual
);

CompositorCanvas.displayName = 'CompositorCanvas';
