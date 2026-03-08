// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Styled components for CompositorCanvas.
 *
 * Extracted to keep the main canvas module under the max-lines lint threshold.
 */

import styled from '@emotion/styled';

import type { ResizeHandle } from '@/hooks/useCompositorLayers';

export const CanvasOuter = styled.div`
  width: 100%;
  position: relative;
  overflow: hidden;
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  background: var(--sk-sidebar-bg);
`;

export const CanvasInner = styled.div`
  position: relative;
  transform-origin: top left;
  background: #1a1a2e;
  overflow: hidden;
`;

export const LayerBox = styled.div`
  position: absolute;
  cursor: move;
  user-select: none;
  will-change: transform, left, top, width, height;
  touch-action: none;
  transform-origin: center center;
`;

export const LayerLabel = styled.div`
  position: absolute;
  top: 2px;
  left: 4px;
  font-size: 10px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.95);
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.9);
  pointer-events: none;
  white-space: nowrap;
  z-index: 2;
`;

/** Text content rendered inside text overlay layers.
 *  Aligned top-left to match the backend compositor which renders text
 *  from origin (0, 0) within the overlay bitmap. */
export const TextContent = styled.div`
  position: absolute;
  inset: 0;
  display: flex;
  align-items: flex-start;
  justify-content: flex-start;
  pointer-events: none;
  z-index: 1;
`;

export const LayerDimensions = styled.div`
  position: absolute;
  bottom: 2px;
  right: 4px;
  font-size: 9px;
  color: rgba(255, 255, 255, 0.6);
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.8);
  pointer-events: none;
  font-variant-numeric: tabular-nums;
  z-index: 2;
`;

const HANDLE_SIZE = 8;

export const ResizeHandleDiv = styled.div<{ position: ResizeHandle }>`
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

export const EmptyState = styled.div`
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  min-height: 80px;
  color: var(--sk-text-muted);
  font-size: 11px;
`;
