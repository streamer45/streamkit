// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Styled components for the Monitor View.
 *
 * Extracted from MonitorView.tsx to keep the main view file focused
 * on behaviour rather than presentation.
 */

import styled from '@emotion/styled';
import * as Tooltip from '@radix-ui/react-tooltip';

import { Button } from '@/components/ui/Button';

// ---------------------------------------------------------------------------
// Legend
// ---------------------------------------------------------------------------

export const LegendContainer = styled.div`
  position: absolute;
  bottom: 20px;
  right: 20px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  padding: 12px;
  box-shadow: 0 4px 12px var(--sk-shadow);
  z-index: 10;
  font-size: 12px;
`;

export const LegendTitle = styled.div`
  font-weight: 600;
  margin-bottom: 8px;
  color: var(--sk-text);
`;

export const LegendItem = styled.div`
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
  color: var(--sk-text);

  &:last-child {
    margin-bottom: 0;
  }
`;

export const LegendDot = styled.div<{ color: string }>`
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background-color: ${(props) => props.color};
  border: 1px solid var(--sk-border-strong);
  flex-shrink: 0;
`;

// ---------------------------------------------------------------------------
// Connection status
// ---------------------------------------------------------------------------

export const ConnectionStatusContainer = styled.div<{ connected: boolean }>`
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 8px;
  border-radius: 4px;
  font-size: 12px;
  background: ${(props) =>
    props.connected ? 'var(--sk-overlay-medium)' : 'var(--sk-overlay-medium)'};
  color: ${(props) => (props.connected ? 'var(--sk-success)' : 'var(--sk-danger)')};
  border: 1px solid ${(props) => (props.connected ? 'var(--sk-success)' : 'var(--sk-danger)')};
  user-select: none;
`;

export const ConnectionStatusDot = styled.div<{ connected: boolean }>`
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: ${(props) => (props.connected ? 'var(--sk-success)' : 'var(--sk-danger)')};
  animation: ${(props) => (props.connected ? 'pulse 2s ease-in-out infinite' : 'none')};

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.5;
    }
  }
`;

// ---------------------------------------------------------------------------
// Left panel (session list)
// ---------------------------------------------------------------------------

export const LeftPanelAside = styled.aside`
  height: 100%;
  width: 100%;
  border-right: 1px solid var(--sk-border);
  background-color: var(--sk-sidebar-bg);
  display: flex;
  flex-direction: column;
`;

export const SessionsContainer = styled.div`
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  overflow: hidden;
`;

export const SessionSearchInput = styled.input`
  box-sizing: border-box;
  width: 100%;
  padding: 8px 12px;
  margin-bottom: 8px;
  font-size: 13px;
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  background: var(--sk-input-bg);
  color: var(--sk-text);
  outline: none;
  flex-shrink: 0;

  &::placeholder {
    color: var(--sk-text-muted);
  }

  &:focus {
    border-color: var(--sk-primary);
    box-shadow: 0 0 0 2px var(--sk-primary-alpha);
  }
`;

export const SearchWrapper = styled.div`
  padding: 4px 4px 0 4px;
`;

export const SessionListWrapper = styled.div`
  flex: 1;
  overflow-y: auto;
  min-height: 0;
  padding: 0 4px 4px 4px;
`;

export const LoadingText = styled.p`
  font-size: 12px;
  color: var(--sk-text-muted);
`;

export const SessionList = styled.ul`
  list-style: none;
  padding: 4px;
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

export const SessionItemWrapper = styled.div`
  position: relative;

  &:hover .session-delete-button {
    opacity: 1;
    pointer-events: auto;
  }
`;

export const SessionButton = styled(Button)<{ active: boolean }>`
  width: 100%;
  padding: 8px;
  text-align: left;
  font-weight: 500;
  font-size: 13px;
  justify-content: flex-start;
  gap: 8px;
`;

export const SessionStatusBadge = styled.div<{ color: string }>`
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background-color: ${(props) => props.color};
  border: 1px solid var(--sk-border-strong);
  flex-shrink: 0;
`;

export const SessionButtonText = styled.span`
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
`;

export const SessionDeleteButton = styled.button`
  position: absolute;
  right: 8px;
  top: 50%;
  transform: translateY(-50%);
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.15s ease;
  background: var(--sk-danger);
  color: var(--sk-text-inverse);
  border: none;
  border-radius: 4px;
  padding: 4px 8px;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1;
  font-size: 16px;
  line-height: 1;

  &:hover {
    background: var(--sk-danger-hover);
  }

  &:active {
    transform: translateY(-50%) scale(0.95);
  }
`;

// ---------------------------------------------------------------------------
// Session tooltip
// ---------------------------------------------------------------------------

export const SessionTooltipContent = styled(Tooltip.Content)`
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  padding: 8px 12px;
  box-shadow: 0 4px 12px var(--sk-shadow);
  font-size: 11px;
  z-index: 1000;
  font-family:
    'JetBrains Mono', 'SF Mono', 'Monaco', 'Inconsolata', 'Fira Code', 'Droid Sans Mono',
    'Courier New', monospace;
`;

export const TooltipRow = styled.div`
  display: flex;
  gap: 8px;
  margin: 4px 0;
`;

export const TooltipLabel = styled.span`
  opacity: 0.7;
  min-width: 50px;
`;

export const TooltipValue = styled.span`
  font-weight: 500;
  color: var(--sk-text);
`;

// ---------------------------------------------------------------------------
// Nodes library
// ---------------------------------------------------------------------------

export const NodesLibraryContainer = styled.div`
  height: 100%;
  display: flex;
  flex-direction: column;
`;

export const EmptyStateText = styled.div`
  padding: 20px;
  font-size: 12px;
  color: var(--sk-text-muted);
  text-align: center;
`;

// ---------------------------------------------------------------------------
// Center panel / canvas overlay
// ---------------------------------------------------------------------------

export const CenterPanelContainer = styled.div`
  width: 100%;
  height: 100%;
  position: relative;
`;

export const CanvasTopBar = styled.div`
  position: absolute;
  top: 12px;
  left: 12px;
  right: 12px;
  z-index: 11;
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  pointer-events: none;

  @media (max-width: 900px) {
    flex-direction: column;
    align-items: stretch;
  }
`;

export const TopLeftControls = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
  align-items: flex-start;
  pointer-events: auto;
  max-width: min(520px, 60vw);

  @media (max-width: 900px) {
    max-width: 100%;
  }
`;

export const TopRightControls = styled.div`
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 8px;
  pointer-events: auto;

  @media (max-width: 900px) {
    align-items: flex-start;
  }
`;

// ---------------------------------------------------------------------------
// Session chip (top-left of canvas)
// ---------------------------------------------------------------------------

export const SessionChipContainer = styled.div`
  position: relative;
`;

export const SessionChipButton = styled(Button)`
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  max-width: 100%;
  user-select: none;
`;

export const SessionChipName = styled.span`
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 260px;

  @media (max-width: 900px) {
    max-width: 52vw;
  }
`;

export const SessionChipMeta = styled.span`
  color: var(--sk-text-muted);
  font-size: 11px;
  white-space: nowrap;
`;

export const SessionChipCaret = styled.span`
  margin-left: 2px;
  opacity: 0.7;
`;

export const SessionStatusDot = styled.span<{ color: string }>`
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: ${(p) => p.color};
  box-shadow: 0 0 6px ${(p) => `${p.color}55`};
`;

// ---------------------------------------------------------------------------
// Session details popover
// ---------------------------------------------------------------------------

export const SessionDetailsPanel = styled.div`
  position: absolute;
  top: calc(100% + 8px);
  left: 0;
  z-index: 12;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  padding: 10px 12px;
  box-shadow: 0 2px 12px var(--sk-shadow);
  font-family:
    'JetBrains Mono', 'SF Mono', 'Monaco', 'Inconsolata', 'Fira Code', 'Droid Sans Mono',
    'Courier New', monospace;
  font-size: 11px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 280px;
  max-width: min(520px, 80vw);
`;

export const DetailsRow = styled.div`
  display: flex;
  gap: 10px;
  align-items: center;
`;

export const DetailsLabel = styled.span`
  opacity: 0.7;
  min-width: 56px;
`;

export const DetailsValue = styled.span`
  font-weight: 500;
  color: var(--sk-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
`;

// ---------------------------------------------------------------------------
// Action buttons
// ---------------------------------------------------------------------------

export const ButtonGroup = styled.div`
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  justify-content: flex-end;

  @media (max-width: 900px) {
    justify-content: flex-start;
  }
`;

// ---------------------------------------------------------------------------
// Empty state
// ---------------------------------------------------------------------------

export const EmptyMonitorState = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  gap: 12px;
  color: var(--sk-text-muted);
  font-size: 14px;
  text-align: center;
`;
