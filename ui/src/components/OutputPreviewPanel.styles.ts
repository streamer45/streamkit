// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Styled components for OutputPreviewPanel.
 *
 * Extracted to a separate file to keep the main component under the
 * 500-line max-lines limit while keeping all layout/style concerns
 * co-located.
 */

import styled from '@emotion/styled';

import type { WatchStatus } from '@/stores/streamStore';

export const FloatingPanel = styled.div`
  position: absolute;
  z-index: 20;
  display: flex;
  flex-direction: column;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.25);
  pointer-events: auto;
  overflow: hidden;
  min-width: 180px;
  max-width: 90vw;
`;

/** Invisible resize handle on the left edge of the panel */
export const ResizeEdgeLeft = styled.div`
  position: absolute;
  top: 0;
  left: -3px;
  width: 6px;
  height: 100%;
  cursor: ew-resize;
  z-index: 25;
`;

/** Invisible resize handle on the top edge of the panel */
export const ResizeEdgeTop = styled.div`
  position: absolute;
  top: -3px;
  left: 0;
  width: 100%;
  height: 6px;
  cursor: ns-resize;
  z-index: 25;
`;

/** Invisible resize handle on the right edge of the preview panel. */
export const ResizeEdgeRight = styled.div`
  position: absolute;
  top: 0;
  right: -3px;
  width: 6px;
  height: 100%;
  cursor: ew-resize;
  z-index: 25;
`;

/** Invisible resize handle on the bottom edge of the preview panel. */
export const ResizeEdgeBottom = styled.div`
  position: absolute;
  bottom: -3px;
  left: 0;
  width: 100%;
  height: 6px;
  cursor: ns-resize;
  z-index: 25;
`;

/** Invisible resize handle on a panel corner.  Position and cursor are
 *  derived from the `corner` prop to avoid four near-identical components. */
export const ResizeCorner = styled.div<{
  corner: 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right';
}>`
  position: absolute;
  width: 12px;
  height: 12px;
  z-index: 26;
  ${({ corner }) => {
    switch (corner) {
      case 'top-left':
        return 'top: -3px; left: -3px; cursor: nwse-resize;';
      case 'top-right':
        return 'top: -3px; right: -3px; cursor: nesw-resize;';
      case 'bottom-left':
        return 'bottom: -3px; left: -3px; cursor: nesw-resize;';
      case 'bottom-right':
        return 'bottom: -3px; right: -3px; cursor: nwse-resize;';
    }
  }}
`;

export const DragHeader = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 8px;
  height: 28px;
  min-height: 28px;
  background: var(--sk-panel-bg);
  border-bottom: 1px solid var(--sk-border);
  cursor: grab;
  color: var(--sk-text);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.02em;
  user-select: none;

  &:active {
    cursor: grabbing;
  }
`;

export const HeaderLeft = styled.span`
  display: flex;
  align-items: center;
  gap: 5px;
  overflow: hidden;
  white-space: nowrap;
`;

export const StatusDot = styled.span<{ status: WatchStatus }>`
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex-shrink: 0;
  background: ${({ status }) => {
    switch (status) {
      case 'live':
        return 'var(--sk-success, #22c55e)';
      case 'loading':
        return 'var(--sk-warning, #eab308)';
      default:
        return 'var(--sk-text-muted, #888)';
    }
  }};
`;

export const HeaderButton = styled.button`
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  padding: 0;
  border: none;
  border-radius: 3px;
  background: none;
  color: var(--sk-text-muted);
  cursor: pointer;
  font-size: 12px;
  line-height: 1;
  flex-shrink: 0;

  &:hover {
    background: var(--sk-overlay-medium);
    color: var(--sk-text);
  }
`;

export const PanelBody = styled.div`
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  padding: 6px;
  background: #0a0a0f;
  cursor: grab;
  /* Fill remaining space below the header so the panel's explicit height
     controls the body size.  min-height: 0 allows the flex child to
     shrink below its content height for proper letterboxing. */
  flex: 1;
  min-height: 0;

  &:active {
    cursor: grabbing;
  }
`;

export const EmptyMessage = styled.div`
  color: var(--sk-text-muted);
  font-size: 11px;
  text-align: center;
  line-height: 1.4;
  padding: 12px 8px;
  max-width: 220px;
`;

/** Wrapper around the canvas + optional overlay.  Fills the PanelBody so the
 *  canvas can letterbox naturally via max-width / max-height / object-fit. */
export const CanvasWrapper = styled.div`
  position: relative;
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  min-width: 0;
  min-height: 0;
`;

/** Semi-transparent overlay shown on the canvas while media is buffering. */
export const BufferingOverlay = styled.div`
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  background: rgba(0, 0, 0, 0.55);
  color: var(--sk-text-muted);
  font-size: 11px;
  pointer-events: none;
  border-radius: 3px;
`;

/** Variant of BufferingOverlay used when no canvas is mounted yet (connecting
 *  state).  Uses `position: relative` so it occupies layout space. */
export const ConnectingOverlay = styled(BufferingOverlay)`
  position: relative;
`;

/** Small inline spinner for the buffering/connecting overlay. */
export const OverlaySpinner = styled.div`
  @keyframes preview-spin {
    to {
      transform: rotate(360deg);
    }
  }
  width: 20px;
  height: 20px;
  border: 2px solid var(--sk-border);
  border-top-color: var(--sk-primary);
  border-radius: 50%;
  animation: preview-spin 0.8s linear infinite;
`;

export const VolumeSlider = styled.input`
  -webkit-appearance: none;
  appearance: none;
  width: 56px;
  height: 3px;
  background: var(--sk-border);
  border-radius: 2px;
  outline: none;
  cursor: pointer;

  &::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--sk-text);
    cursor: pointer;
  }

  &::-moz-range-thumb {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--sk-text);
    cursor: pointer;
    border: none;
  }
`;

/** The preview canvas uses max-width + max-height + aspect-ratio to
 *  letterbox/pillarbox naturally within the freely-resizable panel body.
 *  object-fit: contain ensures the drawn bitmap scales correctly. */
export const PreviewCanvas = styled.canvas`
  max-width: 100%;
  max-height: 100%;
  border-radius: 3px;
  background: #000;
  display: block;
  object-fit: contain;
`;

/** In fullscreen the canvas must fit inside the viewport without clipping.
 *  max-width + max-height + object-fit: contain gives us letterboxing. */
export const FullscreenCanvas = styled.canvas`
  max-width: 100%;
  max-height: 100%;
  border-radius: 0;
  background: #000;
  display: block;
  object-fit: contain;
`;
