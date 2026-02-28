// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useCallback, useState } from 'react';
import { useShallow } from 'zustand/shallow';

import { useStreamStore } from '@/stores/streamStore';
import type { WatchStatus } from '@/stores/streamStore';

// ---------------------------------------------------------------------------
// Styled components
// ---------------------------------------------------------------------------

const PanelContainer = styled.div<{ collapsed: boolean }>`
  position: absolute;
  bottom: 0;
  left: 0;
  right: 0;
  z-index: 12;
  display: flex;
  flex-direction: column;
  background: var(--sk-panel-bg);
  border-top: 1px solid var(--sk-border);
  height: ${({ collapsed }) => (collapsed ? '32px' : '240px')};
  transition: height 0.2s ease;
  pointer-events: auto;
`;

const PanelHeader = styled.button`
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 12px;
  height: 32px;
  min-height: 32px;
  background: var(--sk-panel-bg);
  border: none;
  border-bottom: 1px solid var(--sk-border);
  cursor: pointer;
  color: var(--sk-text);
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.02em;
  font-family: inherit;
  width: 100%;
  text-align: left;

  &:hover {
    background: var(--sk-bg-hover);
  }
`;

const HeaderLeft = styled.span`
  display: flex;
  align-items: center;
  gap: 6px;
`;

const StatusDot = styled.span<{ status: WatchStatus }>`
  width: 6px;
  height: 6px;
  border-radius: 50%;
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

const ChevronIcon = styled.span<{ collapsed: boolean }>`
  display: inline-flex;
  transform: ${({ collapsed }) => (collapsed ? 'rotate(0deg)' : 'rotate(180deg)')};
  transition: transform 0.2s ease;
  font-size: 14px;
  color: var(--sk-text-muted);
`;

const PanelBody = styled.div`
  flex: 1;
  min-height: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  padding: 8px;
`;

const EmptyMessage = styled.div`
  color: var(--sk-text-muted);
  font-size: 12px;
  text-align: center;
  line-height: 1.5;
  max-width: 280px;
`;

const PreviewCanvas = styled.canvas`
  max-width: 100%;
  max-height: 100%;
  border-radius: 4px;
  background: #000;
  object-fit: contain;
`;

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

interface OutputPreviewPanelProps {
  /** Whether a session is selected in the Monitor View */
  hasSession: boolean;
}

const OutputPreviewPanel: React.FC<OutputPreviewPanelProps> = React.memo(({ hasSession }) => {
  const [collapsed, setCollapsed] = useState(true);

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
      if (el && videoRenderer) {
        videoRenderer.canvas.set(el);
      }
    },
    [videoRenderer]
  );

  const statusLabel =
    watchStatus === 'live'
      ? 'Live'
      : watchStatus === 'loading'
        ? 'Loading...'
        : isConnected
          ? 'Connected'
          : 'Disconnected';

  const renderBody = () => {
    if (!hasSession) {
      return <EmptyMessage>Select a session to preview its output stream.</EmptyMessage>;
    }

    if (!isConnected) {
      return (
        <EmptyMessage>
          Connect to the MoQ gateway in the <strong>Stream</strong> view to preview the output
          stream here.
        </EmptyMessage>
      );
    }

    if (!videoRenderer) {
      return (
        <EmptyMessage>No video renderer available. Enable Watch mode to preview.</EmptyMessage>
      );
    }

    if (!isLive && watchStatus !== 'loading') {
      return (
        <EmptyMessage>
          Waiting for video stream
          {activeSessionId ? ` from session` : ''}...
        </EmptyMessage>
      );
    }

    return (
      <PreviewCanvas
        ref={canvasRef}
        style={{
          aspectRatio: '16 / 9',
        }}
      />
    );
  };

  return (
    <PanelContainer collapsed={collapsed}>
      <PanelHeader onClick={toggleCollapsed} type="button" aria-label="Toggle output preview">
        <HeaderLeft>
          <StatusDot status={watchStatus} />
          Output Preview
          {isConnected && ` \u2014 ${statusLabel}`}
        </HeaderLeft>
        <ChevronIcon collapsed={collapsed} aria-hidden>
          &#9650;
        </ChevronIcon>
      </PanelHeader>
      {!collapsed && <PanelBody>{renderBody()}</PanelBody>}
    </PanelContainer>
  );
});

OutputPreviewPanel.displayName = 'OutputPreviewPanel';

export { OutputPreviewPanel };
