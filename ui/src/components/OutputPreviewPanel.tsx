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
import { Maximize2, Minimize2, Volume2, VolumeX } from 'lucide-react';
import React from 'react';
import { useShallow } from 'zustand/shallow';

import { useAudioControls } from '@/hooks/useAudioControls';
import { useVideoCanvas } from '@/hooks/useVideoCanvas';
import { useStreamStore } from '@/stores/streamStore';
import type { ConnectionStatus, WatchStatus } from '@/stores/streamStore';

import { usePreviewPanelInteraction, type ResizeEdge } from './usePreviewPanelInteraction';

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

/** Invisible resize handle on a panel corner.  Position and cursor are
 *  derived from the `corner` prop to avoid four near-identical components. */
const ResizeCorner = styled.div<{
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
  /* Fill remaining space below the header so the panel's explicit height
     controls the body size.  min-height: 0 allows the flex child to
     shrink below its content height for proper letterboxing. */
  flex: 1;
  min-height: 0;

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

/** Wrapper around the canvas + optional overlay.  Fills the PanelBody so the
 *  canvas can letterbox naturally via max-width / max-height / object-fit. */
const CanvasWrapper = styled.div`
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
const BufferingOverlay = styled.div`
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
const ConnectingOverlay = styled(BufferingOverlay)`
  position: relative;
`;

/** Small inline spinner for the buffering/connecting overlay. */
const OverlaySpinner = styled.div`
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
const PreviewCanvas = styled.canvas`
  max-width: 100%;
  max-height: 100%;
  border-radius: 3px;
  background: #000;
  display: block;
  object-fit: contain;
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
// Hooks — re-exported from dedicated module for backward compatibility
// ---------------------------------------------------------------------------

// Re-export so existing imports from this file continue to work.
export { useAudioControls } from '@/hooks/useAudioControls';

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

/** Human-readable connecting-step labels (mirrors StreamView). */
const CONNECTING_STEP_TEXT: Record<string, string> = {
  devices: 'Requesting devices',
  relay: 'Connecting to relay',
  pipeline: 'Waiting for pipeline',
};

/** Resolve the textual status label from connection/watch state. */
function statusLabel(
  connectionStatus: ConnectionStatus,
  watchStatus: WatchStatus,
  connectingStep: string
): string {
  if (connectionStatus === 'connecting') {
    return connectingStep
      ? (CONNECTING_STEP_TEXT[connectingStep] ?? 'Connecting...')
      : 'Connecting...';
  }
  if (watchStatus === 'live') return 'Live';
  if (watchStatus === 'loading') return 'Buffering...';
  return connectionStatus === 'connected' ? 'Connected' : 'Off';
}

/** Preview body – renders the canvas with optional buffering overlay, or an
 *  empty-state message when no media is available.
 *
 *  The canvas is mounted as soon as a video renderer exists and the store is
 *  connected (even during the ‘loading’ phase) so that the first decoded
 *  frame appears immediately without waiting for the watch status to reach
 *  ‘live’.  A semi-transparent overlay covers the canvas while media is
 *  still buffering, giving the user clear feedback about progress. */
const PreviewBody: React.FC<{
  hasSession: boolean;
  isConnected: boolean;
  isConnecting: boolean;
  hasVideoRenderer: boolean;
  watchStatus: WatchStatus;
  activeSessionId: string | null;
  isFullscreen: boolean;
  canvasRef: (el: HTMLCanvasElement | null) => void;
  canvasAspectRatio: string | undefined;
}> = React.memo(
  ({
    hasSession,
    isConnected,
    isConnecting,
    hasVideoRenderer,
    watchStatus: ws,
    activeSessionId,
    isFullscreen,
    canvasRef,
    canvasAspectRatio,
  }) => {
    if (!hasSession) {
      return <EmptyMessage>Select a session to preview output.</EmptyMessage>;
    }
    if (isConnecting) {
      return (
        <CanvasWrapper>
          <ConnectingOverlay>
            <OverlaySpinner />
            Connecting…
          </ConnectingOverlay>
        </CanvasWrapper>
      );
    }
    if (!isConnected) {
      return (
        <EmptyMessage>
          Connect to the MoQ gateway in the <strong>Stream</strong> view to preview, or click{' '}
          <strong>Preview</strong> above to tap the pipeline directly.
        </EmptyMessage>
      );
    }
    if (!hasVideoRenderer) {
      return <EmptyMessage>No video renderer. Enable Watch mode.</EmptyMessage>;
    }
    if (ws !== 'loading' && ws !== 'live') {
      return (
        <EmptyMessage>
          Waiting for video stream{activeSessionId ? ' from session' : ''}…
        </EmptyMessage>
      );
    }
    // Show the canvas during both ‘loading’ and ‘live’ so video frames
    // appear immediately.  A buffering overlay is added during loading so
    // the user knows data is still arriving (especially when audio starts
    // before video).  We also show it during early ‘live’ if no video
    // frames have been decoded yet (canvasAspectRatio is undefined until
    // the renderer writes the first frame).
    const Canvas = isFullscreen ? FullscreenCanvas : PreviewCanvas;
    const isBuffering = ws === 'loading' || (ws === 'live' && canvasAspectRatio === undefined);
    return (
      <CanvasWrapper>
        <Canvas ref={canvasRef} style={{ aspectRatio: canvasAspectRatio }} />
        {isBuffering && (
          <BufferingOverlay>
            <OverlaySpinner />
            {ws === 'loading' ? 'Buffering…' : 'Waiting for first frame…'}
          </BufferingOverlay>
        )}
      </CanvasWrapper>
    );
  }
);
PreviewBody.displayName = 'PreviewBody';

/** Header action buttons (audio controls + fullscreen toggle + collapse). */
const PanelHeaderButtons: React.FC<{
  isFullscreen: boolean;
  collapsed: boolean;
  toggleFullscreen: () => void;
  toggleCollapsed: () => void;
  hasAudio: boolean;
  muted: boolean;
  volume: number;
  onToggleMute: () => void;
  onVolumeChange: (v: number) => void;
}> = React.memo(
  ({
    isFullscreen,
    collapsed,
    toggleFullscreen,
    toggleCollapsed,
    hasAudio,
    muted,
    volume,
    onToggleMute,
    onVolumeChange,
  }) => (
    <span style={{ display: 'flex', alignItems: 'center', gap: 2 }}>
      {hasAudio && (
        <>
          <HeaderButton onClick={onToggleMute} title={muted ? 'Unmute preview' : 'Mute preview'}>
            {muted ? <VolumeX size={12} /> : <Volume2 size={12} />}
          </HeaderButton>
          <VolumeSlider
            type="range"
            min={0}
            max={1}
            step={0.05}
            value={muted ? 0 : volume}
            onChange={(e) => onVolumeChange(Number(e.target.value))}
            onPointerDown={(e) => e.stopPropagation()}
            className="nodrag nopan"
            title={`Volume: ${Math.round((muted ? 0 : volume) * 100)}%`}
          />
        </>
      )}
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
  )
);
PanelHeaderButtons.displayName = 'PanelHeaderButtons';

/** Resize edge and corner handles shown around the panel when not collapsed/fullscreen. */
const ResizeEdges: React.FC<{
  onResizeStart: (edge: ResizeEdge, e: React.PointerEvent) => void;
}> = React.memo(({ onResizeStart }) => (
  <>
    <ResizeEdgeLeft className="nodrag nopan" onPointerDown={(e) => onResizeStart('left', e)} />
    <ResizeEdgeTop className="nodrag nopan" onPointerDown={(e) => onResizeStart('top', e)} />
    <ResizeEdgeRight className="nodrag nopan" onPointerDown={(e) => onResizeStart('right', e)} />
    <ResizeEdgeBottom className="nodrag nopan" onPointerDown={(e) => onResizeStart('bottom', e)} />
    <ResizeCorner
      corner="top-left"
      className="nodrag nopan"
      onPointerDown={(e) => onResizeStart('top-left', e)}
    />
    <ResizeCorner
      corner="top-right"
      className="nodrag nopan"
      onPointerDown={(e) => onResizeStart('top-right', e)}
    />
    <ResizeCorner
      corner="bottom-left"
      className="nodrag nopan"
      onPointerDown={(e) => onResizeStart('bottom-left', e)}
    />
    <ResizeCorner
      corner="bottom-right"
      className="nodrag nopan"
      onPointerDown={(e) => onResizeStart('bottom-right', e)}
    />
  </>
));
ResizeEdges.displayName = 'ResizeEdges';

// ---------------------------------------------------------------------------
// Main component
// ---------------------------------------------------------------------------

interface OutputPreviewPanelProps {
  /** Whether a session is selected in the Monitor View */
  hasSession: boolean;
  /** When true the panel is only rendered if there is something to preview */
  conditionalRender?: boolean;
}

/** Body style when the panel is in fullscreen mode. */
const FULLSCREEN_BODY_STYLE: React.CSSProperties = {
  flex: 1,
  display: 'flex',
  alignItems: 'center',
  justifyContent: 'center',
  background: '#000',
};

const OutputPreviewPanel: React.FC<OutputPreviewPanelProps> = React.memo(
  ({ hasSession, conditionalRender = false }) => {
    const { status, watchStatus, videoRenderer, audioEmitter, activeSessionId, connectingStep } =
      useStreamStore(
        useShallow((s) => ({
          status: s.status,
          watchStatus: s.watchStatus,
          videoRenderer: s.videoRenderer,
          audioEmitter: s.audioEmitter,
          activeSessionId: s.activeSessionId,
          connectingStep: s.connectingStep,
        }))
      );

    const { canvasRef, aspectRatio: canvasAspectRatio } = useVideoCanvas(videoRenderer);
    const { muted, volume, toggleMute, changeVolume } = useAudioControls(audioEmitter);

    const {
      panelRef,
      collapsed,
      isFullscreen,
      toggleCollapsed,
      toggleFullscreen,
      panelStyle,
      handleResizeStart,
      handleDragStart,
    } = usePreviewPanelInteraction(canvasAspectRatio);

    const isConnected = status === 'connected';
    const isConnecting = status === 'connecting';
    const isLive = watchStatus === 'live';

    // Conditional rendering: placed after all hooks to satisfy rules-of-hooks.
    // Also show during 'connecting' so the user sees a spinner instead of nothing.
    const shouldShow =
      !conditionalRender || isConnecting || (isConnected && (isLive || watchStatus === 'loading'));
    if (!shouldShow) return null;

    const label = statusLabel(status, watchStatus, connectingStep);

    return (
      <FloatingPanel ref={panelRef} style={panelStyle}>
        {!isFullscreen && !collapsed && <ResizeEdges onResizeStart={handleResizeStart} />}
        <DragHeader onPointerDown={handleDragStart} onDoubleClick={toggleFullscreen}>
          <HeaderLeft>
            <StatusDot status={watchStatus} />
            Preview
            {(isConnected || status === 'connecting') && ` \u2014 ${label}`}
          </HeaderLeft>
          <PanelHeaderButtons
            isFullscreen={isFullscreen}
            collapsed={collapsed}
            toggleFullscreen={toggleFullscreen}
            toggleCollapsed={toggleCollapsed}
            hasAudio={!!audioEmitter}
            muted={muted}
            volume={volume}
            onToggleMute={toggleMute}
            onVolumeChange={changeVolume}
          />
        </DragHeader>
        {!collapsed && (
          <PanelBody
            onPointerDown={isFullscreen ? undefined : handleDragStart}
            onDoubleClick={toggleFullscreen}
            style={isFullscreen ? FULLSCREEN_BODY_STYLE : undefined}
          >
            <PreviewBody
              hasSession={hasSession}
              isConnected={isConnected}
              isConnecting={isConnecting}
              hasVideoRenderer={!!videoRenderer}
              watchStatus={watchStatus}
              activeSessionId={activeSessionId}
              isFullscreen={isFullscreen}
              canvasRef={canvasRef}
              canvasAspectRatio={canvasAspectRatio}
            />
          </PanelBody>
        )}
      </FloatingPanel>
    );
  }
);

OutputPreviewPanel.displayName = 'OutputPreviewPanel';

export { OutputPreviewPanel };
