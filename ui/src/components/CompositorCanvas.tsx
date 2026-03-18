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

import { useAtomValue } from 'jotai/react';
import React, { useCallback, useEffect, useRef, useState } from 'react';

import {
  layerIdsAtom,
  textOverlayIdsAtom,
  imageOverlayIdsAtom,
  selectedLayerIdAtom,
} from '@/hooks/compositorAtoms';
import type { LayerKind, ResizeHandle } from '@/hooks/useCompositorLayers';

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

export const CompositorCanvas: React.FC<CompositorCanvasProps> = React.memo(
  ({
    canvasWidth,
    canvasHeight,
    onSelectLayer,
    onLayerPointerDown,
    onResizePointerDown,
    onTextFocusRequest,
    onLayerContextMenu,
    layerRefs,
    snapGuideRefs,
    disabled,
  }) => {
    const layerIds = useAtomValue(layerIdsAtom);
    const textOverlayIds = useAtomValue(textOverlayIdsAtom);
    const imageOverlayIds = useAtomValue(imageOverlayIdsAtom);
    const selectedLayerId = useAtomValue(selectedLayerIdAtom);

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
        if (textOverlayIds.includes(hitId)) kind = 'text';
        else if (imageOverlayIds.includes(hitId)) kind = 'image';
        onLayerContextMenu(hitId, kind, e.clientX, e.clientY);
      },
      [disabled, onLayerContextMenu, onSelectLayer, layerRefs, textOverlayIds, imageOverlayIds]
    );

    const hasContent =
      layerIds.length > 0 || textOverlayIds.length > 0 || imageOverlayIds.length > 0;

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
              {layerIds.map((layerId, i) => (
                <VideoLayer
                  key={layerId}
                  layerId={layerId}
                  index={i}
                  isSelected={selectedLayerId === layerId}
                  onPointerDown={disabled ? noopPointerDown : onLayerPointerDown}
                  onResizeStart={disabled ? noopResizeStart : onResizePointerDown}
                  layerRef={getLayerRef(layerId)}
                />
              ))}
              {textOverlayIds.map((overlayId, i) => (
                <TextOverlayLayer
                  key={overlayId}
                  overlayId={overlayId}
                  index={i}
                  isSelected={selectedLayerId === overlayId}
                  onPointerDown={disabled ? noopPointerDown : onLayerPointerDown}
                  onResizeStart={disabled ? noopResizeStart : onResizePointerDown}
                  onTextFocusRequest={disabled ? undefined : onTextFocusRequest}
                  layerRef={getLayerRef(overlayId)}
                />
              ))}
              {imageOverlayIds.map((overlayId, i) => (
                <ImageOverlayLayer
                  key={overlayId}
                  overlayId={overlayId}
                  index={i}
                  isSelected={selectedLayerId === overlayId}
                  onPointerDown={disabled ? noopPointerDown : onLayerPointerDown}
                  onResizeStart={disabled ? noopResizeStart : onResizePointerDown}
                  layerRef={getLayerRef(overlayId)}
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
  }
);

CompositorCanvas.displayName = 'CompositorCanvas';
