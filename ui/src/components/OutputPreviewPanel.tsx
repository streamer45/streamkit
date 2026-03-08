// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Floating, draggable output preview panel for the Monitor View.
 *
 * Renders as a compact window that can be repositioned anywhere on the
 * canvas. When collapsed it shows only a small title bar; when expanded
 * it displays the live MoQ video stream at the correct aspect ratio.
 */

import styled from '@emotion/styled';
import { Maximize2, Minimize2 } from 'lucide-react';
import React, { useCallback, useRef, useState } from 'react';
import { useShallow } from 'zustand/shallow';

import { useVideoCanvas } from '@/hooks/useVideoCanvas';
import { useStreamStore } from '@/stores/streamStore';
import type { WatchStatus } from '@/stores/streamStore';

// ---------------------------------------------------------------------------
// Styled components
// ---------------------------------------------------------------------------

const FloatingPanel = styled.div`
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
const ResizeEdgeLeft = styled.div`
  position: absolute;
  top: 0;
  left: -3px;
  width: 6px;
  height: 100%;
  cursor: ew-resize;
  z-index: 25;
`;

/** Invisible resize handle on the top edge of the panel */
const ResizeEdgeTop = styled.div`
  position: absolute;
  top: -3px;
  left: 0;
  width: 100%;
  height: 6px;
  cursor: ns-resize;
  z-index: 25;
`;

/** Invisible resize handle on the right edge of the preview panel. */
const ResizeEdgeRight = styled.div`
  position: absolute;
  top: 0;
  right: -3px;
  width: 6px;
  height: 100%;
  cursor: ew-resize;
  z-index: 25;
`;

/** Invisible resize handle on the bottom edge of the preview panel. */
const ResizeEdgeBottom = styled.div`
  position: absolute;
  bottom: -3px;
  left: 0;
  width: 100%;
  height: 6px;
  cursor: ns-resize;
  z-index: 25;
`;

const DragHeader = styled.div`
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

const HeaderLeft = styled.span`
  display: flex;
  align-items: center;
  gap: 5px;
  overflow: hidden;
  white-space: nowrap;
`;

const StatusDot = styled.span<{ status: WatchStatus }>`
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

const HeaderButton = styled.button`
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

const PanelBody = styled.div`
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  padding: 6px;
  background: #0a0a0f;
  cursor: grab;

  &:active {
    cursor: grabbing;
  }
`;

const EmptyMessage = styled.div`
  color: var(--sk-text-muted);
  font-size: 11px;
  text-align: center;
  line-height: 1.4;
  padding: 12px 8px;
  max-width: 220px;
`;

const PreviewCanvas = styled.canvas`
  width: 100%;
  border-radius: 3px;
  background: #000;
  display: block;
`;

/** In fullscreen the canvas must fit inside the viewport without clipping.
 *  max-width + max-height + object-fit: contain gives us letterboxing. */
const FullscreenCanvas = styled.canvas`
  max-width: 100%;
  max-height: 100%;
  border-radius: 0;
  background: #000;
  display: block;
  object-fit: contain;
`;

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

interface OutputPreviewPanelProps {
  /** Whether a session is selected in the Monitor View */
  hasSession: boolean;
  /** When true the panel is only rendered if there is something to preview */
  conditionalRender?: boolean;
}

/** Default panel width (px) */
const DEFAULT_WIDTH = 320;
const MIN_WIDTH = 180;
const MAX_WIDTH = 800;

const OutputPreviewPanel: React.FC<OutputPreviewPanelProps> = React.memo(
  ({ hasSession, conditionalRender = false }) => {
    const [collapsed, setCollapsed] = useState(false);
    const [isFullscreen, setIsFullscreen] = useState(false);
    // Position relative to bottom-right of the container
    const [pos, setPos] = useState({ x: 16, y: 16 });
    const [panelWidth, setPanelWidth] = useState(DEFAULT_WIDTH);
    const panelRef = useRef<HTMLDivElement>(null);
    const dragRef = useRef<{
      startX: number;
      startY: number;
      origX: number;
      origY: number;
    } | null>(null);
    const resizeRef = useRef<{
      startX: number;
      startY: number;
      origWidth: number;
      origY: number;
      edge: 'left' | 'top' | 'right' | 'bottom';
    } | null>(null);

    const { status, watchStatus, videoRenderer, activeSessionId } = useStreamStore(
      useShallow((s) => ({
        status: s.status,
        watchStatus: s.watchStatus,
        videoRenderer: s.videoRenderer,
        activeSessionId: s.activeSessionId,
      }))
    );

    const { canvasRef, aspectRatio: canvasAspectRatio } = useVideoCanvas(videoRenderer);

    const isConnected = status === 'connected';
    const isLive = watchStatus === 'live';

    const toggleCollapsed = useCallback(() => {
      setCollapsed((prev) => !prev);
    }, []);

    const toggleFullscreen = useCallback(() => {
      if (!panelRef.current) return;
      if (!document.fullscreenElement) {
        panelRef.current
          .requestFullscreen()
          .then(() => setIsFullscreen(true))
          .catch(() => {});
      } else {
        document
          .exitFullscreen()
          .then(() => setIsFullscreen(false))
          .catch(() => {});
      }
    }, []);

    // Sync fullscreen state when user exits via Escape key
    React.useEffect(() => {
      const handler = () => {
        if (!document.fullscreenElement) setIsFullscreen(false);
      };
      document.addEventListener('fullscreenchange', handler);
      return () => document.removeEventListener('fullscreenchange', handler);
    }, []);

    // ── Resize handling ─────────────────────────────────────────────────────
    // Support resizing from all four edges of the preview panel.
    const handleResizeStart = useCallback(
      (edge: 'left' | 'top' | 'right' | 'bottom', e: React.PointerEvent) => {
        e.preventDefault();
        e.stopPropagation();
        resizeRef.current = {
          startX: e.clientX,
          startY: e.clientY,
          origWidth: panelWidth,
          origY: pos.y,
          edge,
        };

        const handleResizeMove = (ev: PointerEvent) => {
          if (!resizeRef.current) return;
          const curEdge = resizeRef.current.edge;
          if (curEdge === 'left') {
            // Dragging left edge: moving left increases width (panel anchored to right)
            const dx = resizeRef.current.startX - ev.clientX;
            setPanelWidth(
              Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, resizeRef.current.origWidth + dx))
            );
          } else if (curEdge === 'right') {
            // Dragging right edge: moving right increases width
            // Panel is anchored to right, so also shift position
            const dx = ev.clientX - resizeRef.current.startX;
            setPanelWidth(
              Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, resizeRef.current.origWidth + dx))
            );
          } else if (curEdge === 'top') {
            // Dragging top edge: moving up increases height → increase width proportionally
            const dy = resizeRef.current.startY - ev.clientY;
            setPanelWidth(
              Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, resizeRef.current.origWidth + dy * 1.78))
            );
          } else if (curEdge === 'bottom') {
            // Dragging bottom edge: moving down increases height → increase width proportionally
            const dy = ev.clientY - resizeRef.current.startY;
            setPanelWidth(
              Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, resizeRef.current.origWidth + dy * 1.78))
            );
            // Shift bottom anchor down so the panel grows downward naturally
            setPos((prev) => ({
              ...prev,
              y: Math.max(0, resizeRef.current!.origY - dy),
            }));
          }
        };

        const handleResizeUp = () => {
          resizeRef.current = null;
          document.removeEventListener('pointermove', handleResizeMove);
          document.removeEventListener('pointerup', handleResizeUp);
        };

        document.addEventListener('pointermove', handleResizeMove);
        document.addEventListener('pointerup', handleResizeUp);
      },
      [panelWidth, pos.y]
    );

    // ── Drag handling ────────────────────────────────────────────────────────
    const handleDragStart = useCallback(
      (e: React.PointerEvent) => {
        // Only start drag from the header itself (not buttons inside it)
        if ((e.target as HTMLElement).closest('button')) return;
        e.preventDefault();
        e.stopPropagation();
        dragRef.current = {
          startX: e.clientX,
          startY: e.clientY,
          origX: pos.x,
          origY: pos.y,
        };

        const handleMove = (ev: PointerEvent) => {
          if (!dragRef.current) return;
          const dx = ev.clientX - dragRef.current.startX;
          const dy = ev.clientY - dragRef.current.startY;
          // Inverted because position is relative to bottom-right
          setPos({
            x: Math.max(0, dragRef.current.origX - dx),
            y: Math.max(0, dragRef.current.origY - dy),
          });
        };

        const handleUp = () => {
          dragRef.current = null;
          document.removeEventListener('pointermove', handleMove);
          document.removeEventListener('pointerup', handleUp);
        };

        document.addEventListener('pointermove', handleMove);
        document.addEventListener('pointerup', handleUp);
      },
      [pos]
    );

    // Conditional rendering: hide panel when there's nothing to preview.
    // Placed after all hooks to satisfy rules-of-hooks (no conditional hook calls).
    const shouldShow = !conditionalRender || (isConnected && (isLive || watchStatus === 'loading'));
    if (!shouldShow) return null;

    const statusLabel =
      watchStatus === 'live'
        ? 'Live'
        : watchStatus === 'loading'
          ? 'Loading...'
          : isConnected
            ? 'Connected'
            : 'Off';

    const renderBody = () => {
      if (!hasSession) {
        return <EmptyMessage>Select a session to preview output.</EmptyMessage>;
      }

      if (!isConnected) {
        return (
          <EmptyMessage>
            Connect to the MoQ gateway in the <strong>Stream</strong> view to preview.
          </EmptyMessage>
        );
      }

      if (!videoRenderer) {
        return <EmptyMessage>No video renderer. Enable Watch mode.</EmptyMessage>;
      }

      if (!isLive && watchStatus !== 'loading') {
        return (
          <EmptyMessage>
            Waiting for video stream{activeSessionId ? ' from session' : ''}...
          </EmptyMessage>
        );
      }

      if (isFullscreen) {
        return (
          <FullscreenCanvas
            ref={canvasRef}
            style={{
              aspectRatio: canvasAspectRatio,
            }}
          />
        );
      }

      return (
        <PreviewCanvas
          ref={canvasRef}
          style={{
            aspectRatio: canvasAspectRatio,
          }}
        />
      );
    };

    return (
      <FloatingPanel
        ref={panelRef}
        style={{
          right: isFullscreen ? 0 : pos.x,
          bottom: isFullscreen ? 0 : pos.y,
          width: isFullscreen ? '100%' : collapsed ? undefined : panelWidth,
          height: isFullscreen ? '100%' : undefined,
          borderRadius: isFullscreen ? 0 : undefined,
        }}
      >
        {!isFullscreen && !collapsed && (
          <>
            <ResizeEdgeLeft
              className="nodrag nopan"
              onPointerDown={(e) => handleResizeStart('left', e)}
            />
            <ResizeEdgeTop
              className="nodrag nopan"
              onPointerDown={(e) => handleResizeStart('top', e)}
            />
            <ResizeEdgeRight
              className="nodrag nopan"
              onPointerDown={(e) => handleResizeStart('right', e)}
            />
            <ResizeEdgeBottom
              className="nodrag nopan"
              onPointerDown={(e) => handleResizeStart('bottom', e)}
            />
          </>
        )}
        <DragHeader onPointerDown={handleDragStart} onDoubleClick={toggleFullscreen}>
          <HeaderLeft>
            <StatusDot status={watchStatus} />
            Preview
            {isConnected && ` \u2014 ${statusLabel}`}
          </HeaderLeft>
          <span style={{ display: 'flex', gap: 2 }}>
            <HeaderButton
              onClick={toggleFullscreen}
              title={isFullscreen ? 'Exit fullscreen' : 'Fullscreen'}
            >
              {isFullscreen ? <Minimize2 size={12} /> : <Maximize2 size={12} />}
            </HeaderButton>
            <HeaderButton onClick={toggleCollapsed} title={collapsed ? 'Expand' : 'Collapse'}>
              {collapsed ? '\u25B3' : '\u25BD'}
            </HeaderButton>
          </span>
        </DragHeader>
        {!collapsed && (
          <PanelBody
            onPointerDown={isFullscreen ? undefined : handleDragStart}
            style={
              isFullscreen
                ? {
                    flex: 1,
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    background: '#000',
                  }
                : undefined
            }
          >
            {renderBody()}
          </PanelBody>
        )}
      </FloatingPanel>
    );
  }
);

OutputPreviewPanel.displayName = 'OutputPreviewPanel';

export { OutputPreviewPanel };
