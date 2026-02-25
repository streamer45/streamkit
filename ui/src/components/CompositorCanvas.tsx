// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Visual canvas for the video compositor node.
 *
 * Renders a scaled representation of the compositor output with draggable,
 * resizable layer boxes. All interaction is handled via pointer events;
 * visual updates during drag use direct DOM manipulation (refs) so React
 * never re-renders mid-interaction.
 */

import styled from '@emotion/styled';
import React, { useCallback, useEffect, useRef, useState } from 'react';

import type { LayerState, ResizeHandle } from '@/hooks/useCompositorLayers';

// ── Styled components ───────────────────────────────────────────────────────

const CanvasOuter = styled.div`
  width: 100%;
  position: relative;
  overflow: hidden;
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  background: var(--sk-sidebar-bg);
`;

const CanvasInner = styled.div`
  position: relative;
  transform-origin: top left;
  background: #1a1a2e;
  overflow: hidden;
`;

const LayerBox = styled.div<{ isSelected: boolean }>`
  position: absolute;
  box-sizing: border-box;
  border: 2px solid
    ${(props) => (props.isSelected ? 'var(--sk-primary)' : 'rgba(255, 255, 255, 0.5)')};
  background: ${(props) =>
    props.isSelected ? 'rgba(99, 102, 241, 0.15)' : 'rgba(255, 255, 255, 0.08)'};
  cursor: move;
  user-select: none;
  will-change: transform, left, top, width, height;
  touch-action: none;

  &:hover {
    border-color: ${(props) =>
      props.isSelected ? 'var(--sk-primary)' : 'rgba(255, 255, 255, 0.8)'};
    background: ${(props) =>
      props.isSelected ? 'rgba(99, 102, 241, 0.2)' : 'rgba(255, 255, 255, 0.12)'};
  }
`;

const LayerLabel = styled.div`
  position: absolute;
  top: 2px;
  left: 4px;
  font-size: 10px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.9);
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.8);
  pointer-events: none;
  white-space: nowrap;
`;

const LayerDimensions = styled.div`
  position: absolute;
  bottom: 2px;
  right: 4px;
  font-size: 9px;
  color: rgba(255, 255, 255, 0.6);
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.8);
  pointer-events: none;
  font-variant-numeric: tabular-nums;
`;

const HANDLE_SIZE = 8;

const ResizeHandleDiv = styled.div<{ position: ResizeHandle }>`
  position: absolute;
  width: ${HANDLE_SIZE}px;
  height: ${HANDLE_SIZE}px;
  background: var(--sk-primary);
  border: 1px solid rgba(255, 255, 255, 0.8);
  border-radius: 2px;
  z-index: 10;
  touch-action: none;

  ${(props) => {
    const half = -HANDLE_SIZE / 2;
    switch (props.position) {
      case 'nw':
        return `top: ${half}px; left: ${half}px; cursor: nw-resize;`;
      case 'n':
        return `top: ${half}px; left: 50%; margin-left: ${half}px; cursor: n-resize;`;
      case 'ne':
        return `top: ${half}px; right: ${half}px; cursor: ne-resize;`;
      case 'e':
        return `top: 50%; margin-top: ${half}px; right: ${half}px; cursor: e-resize;`;
      case 'se':
        return `bottom: ${half}px; right: ${half}px; cursor: se-resize;`;
      case 's':
        return `bottom: ${half}px; left: 50%; margin-left: ${half}px; cursor: s-resize;`;
      case 'sw':
        return `bottom: ${half}px; left: ${half}px; cursor: sw-resize;`;
      case 'w':
        return `top: 50%; margin-top: ${half}px; left: ${half}px; cursor: w-resize;`;
    }
  }}
`;

const EmptyState = styled.div`
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  min-height: 80px;
  color: var(--sk-text-muted);
  font-size: 11px;
`;

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

// ── Layer component ─────────────────────────────────────────────────────────

const Layer: React.FC<{
  layer: LayerState;
  isSelected: boolean;
  onPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onResizeStart: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  layerRef: (el: HTMLDivElement | null) => void;
}> = React.memo(({ layer, isSelected, onPointerDown, onResizeStart, layerRef }) => {
  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      onPointerDown(layer.id, e);
    },
    [layer.id, onPointerDown]
  );

  return (
    <LayerBox
      ref={layerRef}
      isSelected={isSelected}
      className="nodrag nopan"
      style={{
        left: layer.x,
        top: layer.y,
        width: layer.width,
        height: layer.height,
        opacity: layer.opacity,
        transform: layer.rotationDegrees !== 0 ? `rotate(${layer.rotationDegrees}deg)` : undefined,
        zIndex: layer.zIndex + 1,
      }}
      onPointerDown={handlePointerDown}
    >
      <LayerLabel>{layer.id}</LayerLabel>
      <LayerDimensions>
        {Math.round(layer.width)}x{Math.round(layer.height)}
      </LayerDimensions>
      {isSelected && <ResizeHandles layerId={layer.id} onResizeStart={onResizeStart} />}
    </LayerBox>
  );
});
Layer.displayName = 'Layer';

// ── Main canvas ─────────────────────────────────────────────────────────────

export interface CompositorCanvasProps {
  canvasWidth: number;
  canvasHeight: number;
  layers: LayerState[];
  selectedLayerId: string | null;
  onSelectLayer: (id: string | null) => void;
  onLayerPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onResizePointerDown: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  layerRefs: React.MutableRefObject<Map<string, HTMLDivElement>>;
  disabled?: boolean;
}

export const CompositorCanvas: React.FC<CompositorCanvasProps> = React.memo(
  ({
    canvasWidth,
    canvasHeight,
    layers,
    selectedLayerId,
    onSelectLayer,
    onLayerPointerDown,
    onResizePointerDown,
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

    const handlePaneClick = useCallback(() => {
      onSelectLayer(null);
    }, [onSelectLayer]);

    const setLayerRef = useCallback(
      (layerId: string) => (el: HTMLDivElement | null) => {
        if (el) {
          layerRefs.current.set(layerId, el);
        } else {
          layerRefs.current.delete(layerId);
        }
      },
      [layerRefs]
    );

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
          {layers.length === 0 ? (
            <EmptyState>No layers configured</EmptyState>
          ) : (
            layers.map((layer) => (
              <Layer
                key={layer.id}
                layer={layer}
                isSelected={selectedLayerId === layer.id}
                onPointerDown={disabled ? () => {} : onLayerPointerDown}
                onResizeStart={disabled ? () => {} : onResizePointerDown}
                layerRef={setLayerRef(layer.id)}
              />
            ))
          )}
        </CanvasInner>
      </CanvasOuter>
    );
  }
);

CompositorCanvas.displayName = 'CompositorCanvas';
