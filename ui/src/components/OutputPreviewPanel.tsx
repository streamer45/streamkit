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
import React, { useCallback, useRef, useState } from 'react';
import { useShallow } from 'zustand/shallow';

import { useCanvasAspectRatio } from '@/hooks/useCanvasAspectRatio';
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

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

interface OutputPreviewPanelProps {
  /** Whether a session is selected in the Monitor View */
  hasSession: boolean;
}

/** Default panel width (px) */
const DEFAULT_WIDTH = 320;

const OutputPreviewPanel: React.FC<OutputPreviewPanelProps> = React.memo(({ hasSession }) => {
  const [collapsed, setCollapsed] = useState(false);
  const [canvasEl, setCanvasEl] = useState<HTMLCanvasElement | null>(null);
  const canvasAspectRatio = useCanvasAspectRatio(canvasEl);
  // Position relative to bottom-right of the container
  const [pos, setPos] = useState({ x: 16, y: 16 });
  const dragRef = useRef<{
    startX: number;
    startY: number;
    origX: number;
    origY: number;
  } | null>(null);

  const { status, watchStatus, videoRenderer, activeSessionId } = useStreamStore(
    useShallow((s) => ({
      status: s.status,
      watchStatus: s.watchStatus,
      videoRenderer: s.videoRenderer,
      activeSessionId: s.activeSessionId,
    }))
  );

  const isConnected = status === 'connected';
  const isLive = watchStatus === 'live';

  const toggleCollapsed = useCallback(() => {
    setCollapsed((prev) => !prev);
  }, []);

  const canvasRef = useCallback(
    (el: HTMLCanvasElement | null) => {
      setCanvasEl(el);
      if (el && videoRenderer) {
        videoRenderer.canvas.set(el);
      }
    },
    [videoRenderer]
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
          y: Math.max(0, dragRef.current.origY + dy),
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
      style={{
        right: pos.x,
        bottom: pos.y,
        width: collapsed ? undefined : DEFAULT_WIDTH,
      }}
    >
      <DragHeader onPointerDown={handleDragStart}>
        <HeaderLeft>
          <StatusDot status={watchStatus} />
          Preview
          {isConnected && ` \u2014 ${statusLabel}`}
        </HeaderLeft>
        <HeaderButton onClick={toggleCollapsed} title={collapsed ? 'Expand' : 'Collapse'}>
          {collapsed ? '\u25B3' : '\u25BD'}
        </HeaderButton>
      </DragHeader>
      {!collapsed && <PanelBody>{renderBody()}</PanelBody>}
    </FloatingPanel>
  );
});

OutputPreviewPanel.displayName = 'OutputPreviewPanel';

export { OutputPreviewPanel };
