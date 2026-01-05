// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as Tooltip from '@radix-ui/react-tooltip';
import {
  ReactFlowProvider,
  useNodesState,
  useEdgesState,
  useOnSelectionChange,
  type Node as RFNode,
  type Edge,
  type Connection as RFConnection,
  type ReactFlowInstance,
  type OnConnectEnd,
} from '@xyflow/react';
import { dump, load } from 'js-yaml';
import React, { useState, useEffect, useCallback, useRef } from 'react';
import { useLocation } from 'react-router-dom';
import { v4 as uuidv4 } from 'uuid';
import { useShallow } from 'zustand/shallow';

import ConfirmModal from '@/components/ConfirmModal';
import ContextMenu from '@/components/ContextMenu';
import { FlowCanvas } from '@/components/FlowCanvas';
import NodePalette from '@/components/NodePalette';
import PaneContextMenu from '@/components/PaneContextMenu';
import { PipelineRightPane } from '@/components/PipelineRightPane';
import { ResizableLayout } from '@/components/ResizableLayout';
import { SKTooltip } from '@/components/Tooltip';
import { Button } from '@/components/ui/Button';
import { TabsContent, TabsList, TabsRoot, TabsTrigger } from '@/components/ui/Tabs';
import { ViewTitle } from '@/components/ui/ViewTitle';
import { DnDProvider, useDnD } from '@/context/DnDContext';
import { useToast } from '@/context/ToastContext';
import { useContextMenu } from '@/hooks/useContextMenu';
import { useFitViewOnLayoutPresetChange } from '@/hooks/useFitViewOnLayoutPresetChange';
import { useReactFlowCommon } from '@/hooks/useReactFlowCommon';
import { useResolvedColorMode } from '@/hooks/useResolvedColorMode';
import { useSession } from '@/hooks/useSession';
import { useSessionList } from '@/hooks/useSessionList';
import { useSessionsPrefetch } from '@/hooks/useSessionsPrefetch';
import { useWebSocket } from '@/hooks/useWebSocket';
import { getWebSocketService } from '@/services/websocket';
import { useLayoutStore } from '@/stores/layoutStore';
import { useNodeParamsStore } from '@/stores/nodeParamsStore';
import { usePluginStore } from '@/stores/pluginStore';
import { useSchemaStore } from '@/stores/schemaStore';
import { useSessionStore } from '@/stores/sessionStore';
import {
  useStagingStore,
  type StagingData,
  type StagedChange,
  type ValidationError,
} from '@/stores/stagingStore';
import type {
  NodeDefinition,
  Connection,
  Node,
  NodeState,
  Pipeline,
  MessageType,
  BatchOperation,
  InputPin,
  OutputPin,
} from '@/types/types';
import { topoLevelsFromPipeline, orderedNamesFromLevels, verticalLayout } from '@/utils/dag';
import { deepEqual } from '@/utils/deepEqual';
import { validateValue } from '@/utils/jsonSchema';
import {
  DEFAULT_NODE_WIDTH,
  DEFAULT_NODE_HEIGHT,
  DEFAULT_HORIZONTAL_GAP,
  DEFAULT_VERTICAL_GAP,
  ESTIMATED_HEIGHT_BY_KIND,
} from '@/utils/layoutConstants';
import { viewsLogger } from '@/utils/logger';
import { validatePipeline } from '@/utils/pipelineValidation';
import { nodeTypes, defaultEdgeOptions } from '@/utils/reactFlowDefaults';
import { collectNodeHeights } from '@/utils/reactFlowInstance';
import {
  computeSessionStatus,
  getSessionStatusColor,
  getSessionStatusLabel,
} from '@/utils/sessionStatus';
import { formatUptime, formatDateTime } from '@/utils/time';

const LegendContainer = styled.div`
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

const LegendTitle = styled.div`
  font-weight: 600;
  margin-bottom: 8px;
  color: var(--sk-text);
`;

const LegendItem = styled.div`
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
  color: var(--sk-text);

  &:last-child {
    margin-bottom: 0;
  }
`;

const LegendDot = styled.div<{ color: string }>`
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background-color: ${(props) => props.color};
  border: 1px solid var(--sk-border-strong);
  flex-shrink: 0;
`;

// Memoized legend component to prevent re-renders during drag
const Legend = React.memo(() => (
  <LegendContainer>
    <LegendTitle>Node States</LegendTitle>
    <LegendItem>
      <LegendDot color="var(--sk-status-initializing)" />
      <span>Initializing</span>
    </LegendItem>
    <LegendItem>
      <LegendDot color="var(--sk-status-running)" />
      <span>Running</span>
    </LegendItem>
    <LegendItem>
      <LegendDot color="var(--sk-status-recovering)" />
      <span>Recovering</span>
    </LegendItem>
    <LegendItem>
      <LegendDot color="var(--sk-status-degraded)" />
      <span>Degraded</span>
    </LegendItem>
    <LegendItem>
      <LegendDot color="var(--sk-status-failed)" />
      <span>Failed</span>
    </LegendItem>
    <LegendItem>
      <LegendDot color="var(--sk-status-stopped)" />
      <span>Stopped</span>
    </LegendItem>
  </LegendContainer>
));

const EMPTY_PARAMS: Record<string, unknown> = {};

// Memoized view title to prevent re-renders during drag
const MonitorViewTitle = React.memo(() => <ViewTitle>Monitor</ViewTitle>);

const ConnectionStatusContainer = styled.div<{ connected: boolean }>`
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

const StatusDot = styled.div<{ connected: boolean }>`
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

// Memoized ConnectionStatus component
const ConnectionStatus = React.memo(({ connected }: { connected: boolean }) => (
  <ConnectionStatusContainer connected={connected}>
    <StatusDot connected={connected} />
    {connected ? 'Connected' : 'Disconnected'}
  </ConnectionStatusContainer>
));

const LeftPanelAside = styled.aside`
  height: 100%;
  width: 100%;
  border-right: 1px solid var(--sk-border);
  background-color: var(--sk-sidebar-bg);
  display: flex;
  flex-direction: column;
`;

const SessionsContainer = styled.div`
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  overflow: hidden;
`;

const SessionSearchInput = styled.input`
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

const SearchWrapper = styled.div`
  padding: 4px 4px 0 4px;
`;

const SessionListWrapper = styled.div`
  flex: 1;
  overflow-y: auto;
  min-height: 0;
  padding: 0 4px 4px 4px;
`;

const LoadingText = styled.p`
  font-size: 12px;
  color: var(--sk-text-muted);
`;

const SessionList = styled.ul`
  list-style: none;
  padding: 4px;
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const SessionItemWrapper = styled.div`
  position: relative;

  &:hover .session-delete-button {
    opacity: 1;
    pointer-events: auto;
  }
`;

const SessionButton = styled(Button)<{ active: boolean }>`
  width: 100%;
  padding: 8px;
  text-align: left;
  font-weight: 500;
  font-size: 13px;
  justify-content: flex-start;
  gap: 8px;
`;

const SessionStatusBadge = styled.div<{ color: string }>`
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background-color: ${(props) => props.color};
  border: 1px solid var(--sk-border-strong);
  flex-shrink: 0;
`;

const SessionButtonText = styled.span`
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
`;

const SessionDeleteButton = styled.button`
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

const SessionTooltipContent = styled(Tooltip.Content)`
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

const TooltipRow = styled.div`
  display: flex;
  gap: 8px;
  margin: 4px 0;
`;

const TooltipLabel = styled.span`
  opacity: 0.7;
  min-width: 50px;
`;

const TooltipValue = styled.span`
  font-weight: 500;
  color: var(--sk-text);
`;

const NodesLibraryContainer = styled.div`
  height: 100%;
  display: flex;
  flex-direction: column;
`;

const EmptyStateText = styled.div`
  padding: 20px;
  font-size: 12px;
  color: var(--sk-text-muted);
  text-align: center;
`;

const CenterPanelContainer = styled.div`
  width: 100%;
  height: 100%;
  position: relative;
`;

const CanvasTopBar = styled.div`
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

const TopLeftControls = styled.div`
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

const TopRightControls = styled.div`
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 8px;
  pointer-events: auto;

  @media (max-width: 900px) {
    align-items: flex-start;
  }
`;

const SessionChipContainer = styled.div`
  position: relative;
`;

const SessionChipButton = styled(Button)`
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  max-width: 100%;
  user-select: none;
`;

const SessionChipName = styled.span`
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 260px;

  @media (max-width: 900px) {
    max-width: 52vw;
  }
`;

const SessionChipMeta = styled.span`
  color: var(--sk-text-muted);
  font-size: 11px;
  white-space: nowrap;
`;

const SessionChipCaret = styled.span`
  margin-left: 2px;
  opacity: 0.7;
`;

const SessionStatusDot = styled.span<{ color: string }>`
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: ${(p) => p.color};
  box-shadow: 0 0 6px ${(p) => `${p.color}55`};
`;

const SessionDetailsPanel = styled.div`
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

const DetailsRow = styled.div`
  display: flex;
  gap: 10px;
  align-items: center;
`;

const DetailsLabel = styled.span`
  opacity: 0.7;
  min-width: 56px;
`;

const DetailsValue = styled.span`
  font-weight: 500;
  color: var(--sk-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
`;

const ButtonGroup = styled.div`
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  justify-content: flex-end;

  @media (max-width: 900px) {
    justify-content: flex-start;
  }
`;

const EmptyMonitorState = styled.div`
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

interface SessionItemProps {
  session: { id: string; name: string | null; created_at: string };
  isActive: boolean;
  onClick: (id: string) => void;
  onDelete: (id: string) => void;
}

interface SessionInfoDisplayProps {
  session: { id: string; name: string | null; created_at: string };
}

const shortSessionId = (sessionId: string): string =>
  sessionId.split('-')[0] || sessionId.slice(0, 8);

type NodeIssue = {
  nodeId: string;
  summary: string;
};

function formatIssueDetails(details: unknown): string | null {
  if (details == null) return null;
  try {
    const serialized = JSON.stringify(details);
    if (!serialized || serialized === 'null') return null;
    return serialized.length > 180 ? `${serialized.slice(0, 180)}…` : serialized;
  } catch {
    return null;
  }
}

function formatIssueSummary(prefix: string, reason: string, details: string | null): string {
  if (!details) return `${prefix}: ${reason}`;
  return `${prefix}: ${reason} (${details})`;
}

function summarizeNodeIssues(nodeStates: Record<string, NodeState>): NodeIssue[] {
  const issues: NodeIssue[] = [];

  for (const [nodeId, state] of Object.entries(nodeStates)) {
    if (typeof state !== 'object' || state == null) continue;

    if ('Failed' in state) {
      issues.push({ nodeId, summary: `Failed: ${state.Failed.reason}` });
      continue;
    }
    if ('Degraded' in state) {
      const details = formatIssueDetails(state.Degraded.details);
      issues.push({
        nodeId,
        summary: formatIssueSummary('Degraded', state.Degraded.reason, details),
      });
      continue;
    }
    if ('Recovering' in state) {
      const details = formatIssueDetails(state.Recovering.details);
      issues.push({
        nodeId,
        summary: formatIssueSummary('Recovering', state.Recovering.reason, details),
      });
      continue;
    }
    if ('Stopped' in state) {
      issues.push({ nodeId, summary: `Stopped: ${state.Stopped.reason}` });
      continue;
    }
  }

  const priority = (issue: NodeIssue): number => {
    if (issue.summary.startsWith('Failed:')) return 0;
    if (issue.summary.startsWith('Degraded:')) return 1;
    if (issue.summary.startsWith('Recovering:')) return 2;
    if (issue.summary.startsWith('Stopped:')) return 3;
    return 4;
  };

  return issues.sort((a, b) => priority(a) - priority(b) || a.nodeId.localeCompare(b.nodeId));
}

// Isolated uptime component that only re-renders itself every second
const SessionUptime: React.FC<{ createdAt: string }> = React.memo(({ createdAt }) => {
  const [uptime, setUptime] = useState('');

  useEffect(() => {
    const updateUptime = () => {
      setUptime(formatUptime(createdAt));
    };

    updateUptime();
    const interval = setInterval(updateUptime, 1000);
    return () => clearInterval(interval);
  }, [createdAt]);

  return <>{uptime}</>;
});

const InlineCopyButton: React.FC<{ text: string; tooltip?: string; ariaLabel?: string }> =
  React.memo(({ text, tooltip = 'Copy to clipboard', ariaLabel = 'Copy to clipboard' }) => {
    const [copied, setCopied] = useState(false);

    const handleCopy = useCallback(
      async (e: React.MouseEvent) => {
        e.stopPropagation();
        try {
          await navigator.clipboard.writeText(text);
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        } catch {
          // no-op (clipboard can fail in some environments)
        }
      },
      [text]
    );

    return (
      <SKTooltip content={copied ? 'Copied!' : tooltip} side="top">
        <Button
          aria-label={ariaLabel}
          variant="icon"
          size="small"
          onClick={handleCopy}
          style={{ width: 26, height: 26, padding: 4 }}
        >
          {copied ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <polyline points="20 6 9 17 4 12" />
            </svg>
          ) : (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
              <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
            </svg>
          )}
        </Button>
      </SKTooltip>
    );
  });

const SessionInfoChip: React.FC<SessionInfoDisplayProps> = React.memo(({ session }) => {
  // Get node states from session store with shallow comparison
  const nodeStates = useSessionStore(
    useShallow((state) => state.sessions.get(session.id)?.nodeStates ?? {})
  );

  // Compute session status - memoized to prevent recalculation on every uptime update
  const sessionStatus = React.useMemo(() => computeSessionStatus(nodeStates), [nodeStates]);
  const statusColor = React.useMemo(() => getSessionStatusColor(sessionStatus), [sessionStatus]);
  const statusLabel = React.useMemo(() => getSessionStatusLabel(sessionStatus), [sessionStatus]);
  const issues = React.useMemo(() => summarizeNodeIssues(nodeStates), [nodeStates]);
  const issuesText = React.useMemo(
    () =>
      issues
        .slice(0, 3)
        .map((issue) => `${issue.nodeId}: ${issue.summary}`)
        .join('\n'),
    [issues]
  );

  const [isExpanded, setIsExpanded] = useState(false);
  const containerRef = useRef<HTMLDivElement | null>(null);

  const displayName = session.name || `session-${shortSessionId(session.id)}`;

  useEffect(() => {
    setIsExpanded(false);
  }, [session.id]);

  useEffect(() => {
    if (!isExpanded) return;
    const onMouseDown = (e: MouseEvent) => {
      if (!containerRef.current) return;
      if (e.target instanceof Node && containerRef.current.contains(e.target)) return;
      setIsExpanded(false);
    };
    document.addEventListener('mousedown', onMouseDown);
    return () => document.removeEventListener('mousedown', onMouseDown);
  }, [isExpanded]);

  return (
    <SessionChipContainer ref={containerRef}>
      <SKTooltip
        content={
          <div style={{ maxWidth: 520 }}>
            <div>
              {displayName} ({shortSessionId(session.id)}) — click to{' '}
              {isExpanded ? 'collapse' : 'expand'}
            </div>
            {issuesText && (
              <div style={{ marginTop: 6, whiteSpace: 'pre-wrap', opacity: 0.9 }}>{issuesText}</div>
            )}
          </div>
        }
        side="bottom"
      >
        <SessionChipButton
          aria-expanded={isExpanded}
          variant="secondary"
          onClick={() => setIsExpanded((v) => !v)}
        >
          <SessionStatusDot color={statusColor} />
          <SessionChipName>{displayName}</SessionChipName>
          <SessionChipMeta>{shortSessionId(session.id)}</SessionChipMeta>
          <SessionChipCaret>{isExpanded ? '▴' : '▾'}</SessionChipCaret>
        </SessionChipButton>
      </SKTooltip>
      {isExpanded && (
        <SessionDetailsPanel>
          <DetailsRow>
            <DetailsLabel>Status</DetailsLabel>
            <DetailsValue>{statusLabel}</DetailsValue>
          </DetailsRow>
          {issuesText && (
            <DetailsRow style={{ alignItems: 'flex-start' }}>
              <DetailsLabel>Issues</DetailsLabel>
              <DetailsValue style={{ whiteSpace: 'pre-wrap', overflow: 'visible' }}>
                {issuesText}
              </DetailsValue>
            </DetailsRow>
          )}
          <DetailsRow>
            <DetailsLabel>Start</DetailsLabel>
            <DetailsValue>{formatDateTime(session.created_at)}</DetailsValue>
          </DetailsRow>
          <DetailsRow>
            <DetailsLabel>Up</DetailsLabel>
            <DetailsValue>
              <SessionUptime createdAt={session.created_at} />
            </DetailsValue>
          </DetailsRow>
          <DetailsRow>
            <DetailsLabel>ID</DetailsLabel>
            <SKTooltip content={session.id} side="top">
              <DetailsValue>{session.id}</DetailsValue>
            </SKTooltip>
            <InlineCopyButton
              text={session.id}
              tooltip="Copy session id"
              ariaLabel="Copy session id"
            />
          </DetailsRow>
        </SessionDetailsPanel>
      )}
    </SessionChipContainer>
  );
});

const SessionItem: React.FC<SessionItemProps> = React.memo(
  ({ session, isActive, onClick, onDelete }) => {
    // Get node states from session store with shallow comparison
    // Direct access pattern is more reliable than curried selectors
    const nodeStates = useSessionStore(
      useShallow((state) => state.sessions.get(session.id)?.nodeStates ?? {})
    );

    // Compute session status from node states - memoized to prevent recalculation on every uptime update
    const sessionStatus = React.useMemo(() => computeSessionStatus(nodeStates), [nodeStates]);
    const statusColor = React.useMemo(() => getSessionStatusColor(sessionStatus), [sessionStatus]);
    const statusLabel = React.useMemo(() => getSessionStatusLabel(sessionStatus), [sessionStatus]);
    const issues = React.useMemo(() => summarizeNodeIssues(nodeStates), [nodeStates]);
    const issuesText = React.useMemo(
      () =>
        issues
          .slice(0, 3)
          .map((issue) => `${issue.nodeId}: ${issue.summary}`)
          .join('\n'),
      [issues]
    );

    const handleClick = React.useCallback(() => {
      onClick(session.id);
    }, [onClick, session.id]);

    const handleDelete = React.useCallback(
      (e: React.MouseEvent) => {
        e.stopPropagation(); // Prevent session selection when clicking delete
        onDelete(session.id);
      },
      [onDelete, session.id]
    );

    return (
      <SessionItemWrapper data-testid="session-item">
        <Tooltip.Provider delayDuration={300}>
          <Tooltip.Root open={isActive ? false : undefined}>
            <Tooltip.Trigger asChild>
              <SessionButton variant="secondary" onClick={handleClick} active={isActive}>
                <SessionStatusBadge color={statusColor} />
                <SessionButtonText>{session.name || shortSessionId(session.id)}</SessionButtonText>
              </SessionButton>
            </Tooltip.Trigger>
            {!isActive && (
              <Tooltip.Portal>
                <SessionTooltipContent side="right" sideOffset={8}>
                  <TooltipRow>
                    <TooltipLabel>Status:</TooltipLabel>
                    <TooltipValue>{statusLabel}</TooltipValue>
                  </TooltipRow>
                  {issuesText && (
                    <TooltipRow style={{ alignItems: 'flex-start' }}>
                      <TooltipLabel>Issues:</TooltipLabel>
                      <TooltipValue style={{ whiteSpace: 'pre-wrap' }}>{issuesText}</TooltipValue>
                    </TooltipRow>
                  )}
                  <TooltipRow>
                    <TooltipLabel>Start:</TooltipLabel>
                    <TooltipValue>{formatDateTime(session.created_at)}</TooltipValue>
                  </TooltipRow>
                  <TooltipRow>
                    <TooltipLabel>Up:</TooltipLabel>
                    <TooltipValue>
                      <SessionUptime createdAt={session.created_at} />
                    </TooltipValue>
                  </TooltipRow>
                  <Tooltip.Arrow className="tooltip-arrow" style={{ fill: 'var(--sk-border)' }} />
                </SessionTooltipContent>
              </Tooltip.Portal>
            )}
          </Tooltip.Root>
        </Tooltip.Provider>
        <Tooltip.Root delayDuration={200}>
          <Tooltip.Trigger asChild>
            <SessionDeleteButton
              className="session-delete-button"
              onClick={handleDelete}
              aria-label="Delete session"
              data-testid="session-delete-btn"
            >
              🗑️
            </SessionDeleteButton>
          </Tooltip.Trigger>
          <Tooltip.Portal>
            <SessionTooltipContent side="right" sideOffset={5}>
              Delete session
              <Tooltip.Arrow className="tooltip-arrow" style={{ fill: 'var(--sk-border)' }} />
            </SessionTooltipContent>
          </Tooltip.Portal>
        </Tooltip.Root>
      </SessionItemWrapper>
    );
  }
);

const LiveBadge = styled.span`
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 6px 10px;
  background: rgba(239, 68, 68, 0.15);
  color: rgb(239, 68, 68);
  border: 1px solid rgba(239, 68, 68, 0.3);
  border-radius: 4px;
  font-size: 13px;
  font-weight: 600;
  letter-spacing: 0.3px;
  line-height: 1;
  user-select: none;
`;

const LiveDot = styled.div`
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: rgb(239, 68, 68);
  animation: pulse 2s ease-in-out infinite;
  flex-shrink: 0;

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

// Type for TopControls props
interface TopControlsProps {
  isConnected: boolean;
  selectedSessionId: string | null;
  isInStagingMode: boolean;
  stagingData: StagingData | undefined;
  onCommit: () => void;
  onDiscard: () => void;
  onEnterStaging: () => void;
  onDelete: () => void;
}

/**
 * Helper function for TopControls memo comparison.
 * Complexity of 18 is acceptable here as it performs shallow equality checks
 * on 11 different properties to prevent unnecessary re-renders. Breaking this
 * into sub-functions would make the code harder to understand without providing
 * real benefits.
 */

const areTopControlPropsEqual = (
  prevProps: TopControlsProps,
  nextProps: TopControlsProps
): boolean => {
  // Custom comparison to prevent re-renders when stagingData.version changes
  // but the actual changes/errors arrays haven't changed
  if (prevProps.isConnected !== nextProps.isConnected) return false;
  if (prevProps.selectedSessionId !== nextProps.selectedSessionId) return false;
  if (prevProps.isInStagingMode !== nextProps.isInStagingMode) return false;
  if (prevProps.onCommit !== nextProps.onCommit) return false;
  if (prevProps.onDiscard !== nextProps.onDiscard) return false;
  if (prevProps.onEnterStaging !== nextProps.onEnterStaging) return false;
  if (prevProps.onDelete !== nextProps.onDelete) return false;

  // Compare changes array length and validation errors
  const prevChanges = prevProps.stagingData?.changes ?? [];
  const nextChanges = nextProps.stagingData?.changes ?? [];
  const prevErrors = prevProps.stagingData?.validationErrors ?? [];
  const nextErrors = nextProps.stagingData?.validationErrors ?? [];

  if (prevChanges.length !== nextChanges.length) return false;
  if (prevErrors.length !== nextErrors.length) return false;

  // If lengths are same and other props haven't changed, don't re-render
  return true;
};

// Memoized TopControls component to prevent re-renders during drag
const TopControls = React.memo(
  ({
    isConnected,
    selectedSessionId,
    isInStagingMode,
    stagingData,
    onCommit,
    onDiscard,
    onEnterStaging,
    onDelete,
  }: TopControlsProps) => {
    // Only extract the fields we need to minimize re-renders
    const changes = stagingData?.changes ?? [];
    const validationErrors = stagingData?.validationErrors ?? [];

    return (
      <TopRightControls>
        <ConnectionStatus connected={isConnected} />
        {selectedSessionId && (
          <ButtonGroup>
            {isInStagingMode && stagingData && (
              <>
                <SKTooltip
                  content="Parameters on committed nodes apply immediately. Parameters on staged nodes are queued for commit."
                  side="bottom"
                >
                  <LiveBadge>
                    <LiveDot />
                    Real-time Params
                  </LiveBadge>
                </SKTooltip>
                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: '8px',
                    padding: '6px 12px',
                    fontSize: '13px',
                    color: 'var(--sk-text-muted)',
                    borderRight: '1px solid var(--sk-border)',
                    lineHeight: '1',
                    userSelect: 'none',
                  }}
                >
                  {(() => {
                    const added = changes.filter(
                      (c: StagedChange) => c.type === 'add_node' || c.type === 'add_connection'
                    ).length;
                    const removed = changes.filter(
                      (c: StagedChange) =>
                        c.type === 'remove_node' || c.type === 'remove_connection'
                    ).length;
                    const modified = changes.filter(
                      (c: StagedChange) => c.type === 'update_params'
                    ).length;
                    const hasChanges = added > 0 || removed > 0 || modified > 0;

                    if (!hasChanges) return <span>No changes</span>;

                    const parts = [];
                    if (added > 0)
                      parts.push(
                        <span key="add" style={{ color: 'var(--sk-success)' }}>
                          +{added}
                        </span>
                      );
                    if (removed > 0)
                      parts.push(
                        <span key="rem" style={{ color: 'var(--sk-danger)' }}>
                          -{removed}
                        </span>
                      );
                    if (modified > 0)
                      parts.push(
                        <SKTooltip key="mod" content="Staged nodes with parameter changes">
                          <span style={{ color: 'var(--sk-warning)' }}>~{modified}</span>
                        </SKTooltip>
                      );

                    return <>{parts}</>;
                  })()}
                </div>
                <SKTooltip content="Commit all staged changes">
                  <Button
                    variant="primary"
                    size="small"
                    onClick={onCommit}
                    disabled={
                      !changes.length ||
                      validationErrors.filter((e: ValidationError) => e.type === 'error').length > 0
                    }
                  >
                    Commit
                  </Button>
                </SKTooltip>
              </>
            )}
            <SKTooltip
              content={isInStagingMode ? 'Discard staged changes and exit' : 'Enter Staging Mode'}
            >
              <Button
                variant={isInStagingMode ? 'danger' : 'ghost'}
                size="small"
                onClick={() => (isInStagingMode ? onDiscard() : onEnterStaging())}
                active={isInStagingMode}
                aria-pressed={isInStagingMode}
              >
                {isInStagingMode ? 'Discard' : 'Enter Staging'}
              </Button>
            </SKTooltip>
            {!isInStagingMode && (
              <SKTooltip content="Delete Session" side="bottom">
                <Button variant="danger" size="small" onClick={onDelete}>
                  Delete
                </Button>
              </SKTooltip>
            )}
          </ButtonGroup>
        )}
      </TopRightControls>
    );
  },
  areTopControlPropsEqual
);

// Helper functions for commit changes to reduce complexity

/** Find nodes that were added in staged pipeline */
const computeAddedNodes = (stagedPipeline: Pipeline, livePipeline: Pipeline): BatchOperation[] => {
  const operations: BatchOperation[] = [];
  for (const [nodeId, node] of Object.entries(stagedPipeline.nodes) as [string, Node][]) {
    if (!(nodeId in livePipeline.nodes)) {
      operations.push({
        action: 'addnode',
        node_id: nodeId,
        kind: node.kind,
        params: node.params,
      });
    }
  }
  return operations;
};

/** Find nodes that were removed in staged pipeline */
const computeRemovedNodes = (
  stagedPipeline: Pipeline,
  livePipeline: Pipeline
): BatchOperation[] => {
  const operations: BatchOperation[] = [];
  for (const nodeId of Object.keys(livePipeline.nodes)) {
    if (!(nodeId in stagedPipeline.nodes)) {
      operations.push({
        action: 'removenode',
        node_id: nodeId,
      });
    }
  }
  return operations;
};

/** Create a set of connection keys for comparison */
const connectionKey = (c: Connection): string =>
  `${c.from_node}:${c.from_pin}:${c.to_node}:${c.to_pin}`;

/** Find connections that were added or removed */
const computeConnectionChanges = (
  stagedPipeline: Pipeline,
  livePipeline: Pipeline
): BatchOperation[] => {
  const operations: BatchOperation[] = [];

  const liveConnections = new Set(livePipeline.connections.map(connectionKey));
  const stagedConnections = new Set(stagedPipeline.connections.map(connectionKey));

  // Find connections that were added
  for (const conn of stagedPipeline.connections) {
    if (!liveConnections.has(connectionKey(conn))) {
      operations.push({
        action: 'connect',
        from_node: conn.from_node,
        from_pin: conn.from_pin,
        to_node: conn.to_node,
        to_pin: conn.to_pin,
        mode: conn.mode ?? 'reliable',
      });
    }
  }

  // Find connections that were removed
  for (const conn of livePipeline.connections) {
    if (!stagedConnections.has(connectionKey(conn))) {
      operations.push({
        action: 'disconnect',
        from_node: conn.from_node,
        from_pin: conn.from_pin,
        to_node: conn.to_node,
        to_pin: conn.to_pin,
      });
    }
  }

  return operations;
};

/**
 * Pre-process mixer nodes to set num_inputs based on actual connections.
 * This ensures mixers are created in fixed mode with proper pin counts.
 */
const preprocessMixerNodes = (operations: BatchOperation[]): void => {
  const mixerNodeOps = operations.filter(
    (op): op is Extract<BatchOperation, { action: 'addnode' }> =>
      op.action === 'addnode' && op.kind === 'audio::mixer'
  );

  for (const mixerOp of mixerNodeOps) {
    // Count connections to this mixer
    const connectionsToMixer = operations.filter(
      (op): op is Extract<BatchOperation, { action: 'connect' }> =>
        op.action === 'connect' && op.to_node === mixerOp.node_id
    );

    if (connectionsToMixer.length > 0) {
      // Set num_inputs to the actual connection count (overrides null or undefined)
      // Type guard: merge params only if existing params is an object
      const existingParams = mixerOp.params;
      mixerOp.params =
        existingParams && typeof existingParams === 'object' && !Array.isArray(existingParams)
          ? { ...existingParams, num_inputs: connectionsToMixer.length }
          : { num_inputs: connectionsToMixer.length };
      viewsLogger.debug(
        `Auto-configured mixer ${mixerOp.node_id} with num_inputs=${connectionsToMixer.length}`
      );
    }
  }
};

// Helper functions for topology effect to reduce complexity

/**
 * Checks if an edge connection is valid (both source and target pins exist).
 * Prevents React Flow warnings about missing handles.
 */
const isValidEdgeConnection = (conn: Connection, nodeMap: Map<string, RFNode>): boolean => {
  const sourceNode = nodeMap.get(conn.from_node);
  const targetNode = nodeMap.get(conn.to_node);

  if (!sourceNode || !targetNode) return false;

  const isDynamicTemplatePin = (pin: InputPin | OutputPin): boolean =>
    typeof pin.cardinality === 'object' && pin.cardinality !== null && 'Dynamic' in pin.cardinality;

  // Check if the output pin exists
  const sourceOutputs = (sourceNode.data.outputs || []) as OutputPin[];
  const hasSourcePin = sourceOutputs.some(
    (pin) => pin.name === conn.from_pin && !isDynamicTemplatePin(pin)
  );

  // Check if the input pin exists
  const targetInputs = (targetNode.data.inputs || []) as InputPin[];
  const hasTargetPin = targetInputs.some(
    (pin) => pin.name === conn.to_pin && !isDynamicTemplatePin(pin)
  );

  return hasSourcePin && hasTargetPin;
};

/**
 * Build edges from pipeline connections, filtering out invalid ones.
 */
const buildEdgesFromConnections = (connections: Connection[], nodes: RFNode[]): Edge[] => {
  const nodeMap = new Map(nodes.map((n) => [n.id, n]));

  return connections
    .filter((conn) => isValidEdgeConnection(conn, nodeMap))
    .map((conn) => ({
      id: `${conn.from_node}_${conn.from_pin}-${conn.to_node}_${conn.to_pin}`,
      source: conn.from_node,
      sourceHandle: conn.from_pin,
      target: conn.to_node,
      targetHandle: conn.to_pin,
    }));
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
  value !== null && value !== undefined && typeof value === 'object' && !Array.isArray(value);

type SlowTimeoutDetails = {
  slowPins: string[];
  newlySlowPins: string[];
  syncTimeoutMs: number | null;
};

const extractSlowTimeoutDetailsFromNodeState = (
  state: NodeState | null | undefined
): SlowTimeoutDetails | null => {
  if (!state || typeof state === 'string') return null;
  if (!('Degraded' in state)) return null;
  if (state.Degraded.reason !== 'slow_input_timeout') return null;

  const details = state.Degraded.details;
  if (!isRecord(details)) return null;

  const slowPinsRaw = details['slow_pins'];
  const newlySlowPinsRaw = details['newly_slow_pins'];
  const syncTimeoutRaw = details['sync_timeout_ms'];

  const slowPins = Array.isArray(slowPinsRaw)
    ? slowPinsRaw.filter((p): p is string => typeof p === 'string')
    : [];
  const newlySlowPins = Array.isArray(newlySlowPinsRaw)
    ? newlySlowPinsRaw.filter((p): p is string => typeof p === 'string')
    : [];
  const syncTimeoutMs = typeof syncTimeoutRaw === 'number' ? syncTimeoutRaw : null;

  return { slowPins, newlySlowPins, syncTimeoutMs };
};

const describeSlowInputs = (pipeline: Pipeline, nodeId: string, slowPins: string[]): string[] => {
  if (slowPins.length === 0) return [];
  const slowPinSet = new Set(slowPins);

  const sources = pipeline.connections
    .filter((c) => c.to_node === nodeId && slowPinSet.has(c.to_pin))
    .map((c) => `${c.from_node}.${c.from_pin} → ${c.to_pin}`);

  sources.sort();
  return sources;
};

/**
 * Generate YAML representation of the pipeline ordered by topological sort.
 */
const generatePipelineYaml = (pipeline: Pipeline, orderedNames: string[]): string => {
  const yamlObject: { nodes: Record<string, unknown> } = { nodes: {} };

  for (const nodeName of orderedNames) {
    const apiNode = pipeline.nodes[nodeName];
    if (!apiNode) continue;

    const needs = pipeline.connections
      .filter((c: Connection) => c.to_node === nodeName)
      .map((c: Connection) => c.from_node);

    const nodeConfig: Record<string, unknown> = { kind: apiNode.kind };
    if (apiNode.params && Object.keys(apiNode.params).length > 0) {
      nodeConfig['params'] = apiNode.params;
    }
    if (needs.length === 1) {
      nodeConfig['needs'] = needs[0];
    } else if (needs.length > 1) {
      nodeConfig['needs'] = needs;
    }
    yamlObject.nodes[nodeName] = nodeConfig;
  }

  return dump(yamlObject, { skipInvalid: true });
};

/**
 * Build a single Node object from pipeline data.
 * Helper for topology effect to reduce complexity.
 */
interface BuildNodeParams {
  nodeName: string;
  apiNode: Node;
  position: { x: number; y: number };
  nodeState: unknown; // Can be string | null or NodeState enum
  isStaged: boolean;
  finalInputs: InputPin[];
  finalOutputs: OutputPin[];
  nodeDef: NodeDefinition | undefined;
  stableOnParamChange: (nodeId: string, paramName: string, value: unknown) => void;
  selectedSessionId: string | null;
}

const buildNodeObject = (params: BuildNodeParams): RFNode => {
  return {
    id: params.nodeName,
    type: params.apiNode.kind === 'audio::gain' ? 'audioGain' : 'configurable',
    position: params.position,
    dragHandle: '.drag-handle',
    data: {
      label: params.nodeName,
      kind: params.apiNode.kind,
      params: params.apiNode.params || {},
      inputs: params.finalInputs,
      outputs: params.finalOutputs,
      paramSchema: params.nodeDef?.param_schema,
      nodeDefinition: params.nodeDef,
      definition: { bidirectional: params.nodeDef?.bidirectional },
      state: params.nodeState,
      // Stats are NOT included here to prevent re-renders when they update
      // NodeStateIndicator will fetch them directly from session store on hover
      // Use stable callback that checks staging mode at call-time
      onParamChange: params.stableOnParamChange,
      sessionId: params.selectedSessionId || undefined,
      isStaged: params.isStaged,
    },
  };
};

const LeftPanel = React.memo(
  ({
    isLoadingSessions,
    sessions,
    selectedSessionId,
    onSessionClick,
    onSessionDelete,
    editMode,
    nodeDefinitions,
    onDragStart,
    pluginKinds,
    pluginTypes,
  }: {
    isLoadingSessions: boolean;
    sessions: { id: string; name: string | null; created_at: string }[];
    selectedSessionId: string | null;
    onSessionClick: (id: string) => void;
    onSessionDelete: (id: string) => void;
    editMode: boolean;
    nodeDefinitions: NodeDefinition[];
    onDragStart: (event: React.DragEvent, nodeType: string) => void;
    pluginKinds: Set<string>;
    pluginTypes: Map<string, 'wasm' | 'native'>;
  }) => {
    const [activeTab, setActiveTab] = useState<'sessions' | 'add'>('sessions');
    const [searchQuery, setSearchQuery] = useState('');

    useEffect(() => {
      if (!editMode && activeTab === 'add') {
        setActiveTab('sessions');
      }
    }, [editMode, activeTab]);

    const filteredSessions = React.useMemo(() => {
      if (!searchQuery.trim()) {
        return sessions;
      }
      const query = searchQuery.toLowerCase();
      return sessions.filter(
        (session) =>
          session.id.toLowerCase().includes(query) ||
          (session.name && session.name.toLowerCase().includes(query))
      );
    }, [sessions, searchQuery]);

    return (
      <LeftPanelAside>
        <TabsRoot
          value={activeTab}
          onValueChange={(value) => setActiveTab(value as 'sessions' | 'add')}
        >
          <TabsList>
            <TabsTrigger value="sessions">Sessions</TabsTrigger>
            {editMode && (
              <TabsTrigger value="add" disabled={!selectedSessionId}>
                Nodes Library
              </TabsTrigger>
            )}
          </TabsList>

          <TabsContent value="sessions">
            <SessionsContainer data-testid="sessions-list">
              {isLoadingSessions ? (
                <LoadingText>Loading sessions...</LoadingText>
              ) : sessions.length === 0 ? (
                <EmptyStateText>No active sessions</EmptyStateText>
              ) : (
                <>
                  {sessions.length >= 5 && (
                    <SearchWrapper>
                      <SessionSearchInput
                        type="text"
                        placeholder="Search sessions..."
                        value={searchQuery}
                        onChange={(e) => setSearchQuery(e.target.value)}
                      />
                    </SearchWrapper>
                  )}
                  <SessionListWrapper>
                    {filteredSessions.length === 0 ? (
                      <EmptyStateText>No matching sessions</EmptyStateText>
                    ) : (
                      <SessionList>
                        {filteredSessions.map((session) => (
                          <li key={session.id}>
                            <SessionItem
                              session={session}
                              isActive={selectedSessionId === session.id}
                              onClick={onSessionClick}
                              onDelete={onSessionDelete}
                            />
                          </li>
                        ))}
                      </SessionList>
                    )}
                  </SessionListWrapper>
                </>
              )}
            </SessionsContainer>
          </TabsContent>

          <TabsContent value="add">
            {editMode && (
              <NodesLibraryContainer>
                {selectedSessionId ? (
                  nodeDefinitions.length === 0 ? (
                    <EmptyStateText>Loading node definitions…</EmptyStateText>
                  ) : (
                    <NodePalette
                      nodeDefinitions={nodeDefinitions}
                      onDragStart={onDragStart}
                      pluginKinds={pluginKinds}
                      pluginTypes={pluginTypes}
                    />
                  )
                ) : (
                  <EmptyStateText>Select a session to add nodes</EmptyStateText>
                )}
              </NodesLibraryContainer>
            )}
          </TabsContent>
        </TabsRoot>
      </LeftPanelAside>
    );
  }
);

/**
 * Main content component for the Monitor view.
 * This component has 114 statements which exceeds the max-statements limit.
 * However, breaking it down would require significant architectural changes
 * and may reduce code cohesion. The complexity is managed through:
 * - Extracted helper functions for complex operations
 * - useCallback/useMemo hooks to optimize re-renders
 * - Clear separation of concerns via comments
 */
// eslint-disable-next-line max-statements -- Main view component with many hooks and state management
const MonitorViewContent: React.FC = () => {
  const location = useLocation();
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [nodes, setNodes, onNodesChangeInternal] = useNodesState<RFNode>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const [yamlString, setYamlString] = useState<string>('');
  // Track if user is actively editing YAML to prevent automatic updates from overwriting edits
  const isEditingYamlRef = useRef(false);
  const nodeDefinitions = useSchemaStore(useShallow((s) => s.nodeDefinitions));
  const plugins = usePluginStore(useShallow((s) => s.plugins));
  const pluginKinds = React.useMemo(() => new Set(plugins.map((p) => p.kind)), [plugins]);
  const pluginTypes = React.useMemo(
    () => new Map(plugins.map((p) => [p.kind, p.plugin_type])),
    [plugins]
  );

  // Staging mode state - select the specific session's staging data
  // Version counter in staging data ensures changes are detected
  // Use shallow comparison to ensure we get updates when nested properties change
  const stagingData: StagingData | undefined = useStagingStore(
    useShallow((s) => (selectedSessionId ? s.staging[selectedSessionId] : undefined))
  );
  const enterStagingMode = useStagingStore((s) => s.enterStagingMode);
  const exitStagingMode = useStagingStore((s) => s.exitStagingMode);
  const addStagedNode = useStagingStore((s) => s.addStagedNode);
  const removeStagedNode = useStagingStore((s) => s.removeStagedNode);
  const addStagedConnection = useStagingStore((s) => s.addStagedConnection);
  const removeStagedConnection = useStagingStore((s) => s.removeStagedConnection);
  const updateStagedNodeParams = useStagingStore((s) => s.updateStagedNodeParams);
  const updateNodePosition = useStagingStore((s) => s.updateNodePosition);
  const getNodePositions = useStagingStore((s) => s.getNodePositions);
  const setValidationErrors = useStagingStore((s) => s.setValidationErrors);
  const discardChanges = useStagingStore((s) => s.discardChanges);

  // Derive computed values from staging data
  const isInStagingMode = stagingData?.mode === 'staging';
  const stagedPipeline = stagingData?.stagedPipeline ?? null;

  // Save node positions when drag stops
  const onNodeDragStop = useCallback(
    (_event: React.MouseEvent, node: RFNode) => {
      if (isInStagingMode && selectedSessionId) {
        updateNodePosition(selectedSessionId, node.id, node.position);
      }
    },
    [isInStagingMode, selectedSessionId, updateNodePosition]
  );

  // For backward compatibility, editMode now means "staging mode"
  const editMode = isInStagingMode;
  const [needsAutoLayout, setNeedsAutoLayout] = useState(false);
  const [needsFit, setNeedsFit] = useState(false);
  const [selectedNodes, setSelectedNodes] = useState<string[]>([]);
  const [rightPaneView, setRightPaneView] = useState<'yaml' | 'inspector' | 'telemetry'>('yaml');
  const [showDeleteModal, setShowDeleteModal] = useState(false);
  const [sessionToDelete, setSessionToDelete] = useState<string | null>(null);
  const [isDeletingSession, setIsDeletingSession] = useState(false);
  const colorMode = useResolvedColorMode();
  const { rightCollapsed, setRightCollapsed } = useLayoutStore(
    useShallow((state) => ({
      rightCollapsed: state.rightCollapsed,
      setRightCollapsed: state.setRightCollapsed,
    }))
  );
  const [type, setType] = useDnD();
  const toast = useToast();
  // Cache for positions of nodes that are being added (to preserve drop location)
  const pendingNodePositions = React.useRef<Map<string, { x: number; y: number }>>(new Map());

  // Auto-select session from navigation state (e.g., from Stream view)
  useEffect(() => {
    const state = location.state as { sessionId?: string } | null;
    if (state?.sessionId && !selectedSessionId) {
      const sessionId = state.sessionId;
      setSelectedSessionId(sessionId);

      // Check if this session has saved positions in staging store
      const savedPos = getNodePositions(sessionId);
      const hasPositions = Object.keys(savedPos).length > 0;

      // Trigger auto-layout if no positions are saved
      setNeedsAutoLayout(!hasPositions);
      setNeedsFit(true);

      // Clear the state to avoid auto-selecting on subsequent visits
      window.history.replaceState({}, document.title);
    }
  }, [location.state, selectedSessionId, getNodePositions]);

  // Use shared React Flow logic
  const {
    onInit: baseOnInit,
    isValidConnection,
    createOnConnect,
    createOnConnectEnd,
  } = useReactFlowCommon();
  const rf = React.useRef<ReactFlowInstance | null>(null);
  const onInit = (instance: ReactFlowInstance) => {
    rf.current = instance;
    baseOnInit(instance);
  };
  const screenToFlow = (pt: { x: number; y: number }) => {
    return rf.current?.screenToFlowPosition(pt) ?? pt;
  };

  // Keep refs to avoid recreating callbacks on every drag
  const nodesRefForCallbacks = React.useRef(nodes);
  const edgesRefForCallbacks = React.useRef(edges);
  React.useEffect(() => {
    nodesRefForCallbacks.current = nodes;
    edgesRefForCallbacks.current = edges;
  }, [nodes, edges]);

  // Use shared context menu logic
  const { menu, paneMenu, reactFlowWrapper, onNodeContextMenu, onPaneContextMenu, onPaneClick } =
    useContextMenu();

  useOnSelectionChange({
    onChange: ({ nodes: selNodes }) => {
      const nextIds = selNodes.map((n) => n.id);
      setSelectedNodes((prev) =>
        prev.length === nextIds.length && prev.every((v, i) => v === nextIds[i]) ? prev : nextIds
      );
    },
  });

  // Keep YAML view as default when nodes are selected
  // Inspector only opens on double-click
  useEffect(() => {
    if (selectedNodes.length === 0) {
      // No selection - keep YAML view
      setRightPaneView('yaml');
    } else if (selectedNodes.length > 1) {
      // Multiple selection - show YAML view
      setRightPaneView('yaml');
    } else if (selectedNodes.length === 1) {
      // Single selection - switch to YAML view (with highlighting)
      setRightPaneView('yaml');
    }
  }, [selectedNodes]);

  // Double-click handler to open inspector
  const handleNodeDoubleClick = React.useCallback(() => {
    setRightPaneView('inspector');
    // Expand right pane if collapsed
    if (rightCollapsed) {
      setRightCollapsed(false);
    }
  }, [rightCollapsed, setRightCollapsed]);

  // Memoize selectedNode with custom comparison (ignore position changes)
  const selectedNodeId = selectedNodes.length === 1 ? selectedNodes[0] : null;
  const selectedNode = React.useMemo(() => {
    if (!selectedNodeId) return null;
    return nodes.find((node) => node.id === selectedNodeId) ?? null;
  }, [selectedNodeId, nodes]);

  // Create a stable reference for selectedNode that only changes when data (not position) changes
  const selectedNodeRef = React.useRef(selectedNode);

  // React Flow triggers renders on position changes; we only want to re-render inspector on data changes.
  const stableSelectedNode = React.useMemo(() => {
    if (!selectedNode) {
      selectedNodeRef.current = null;
      return null;
    }
    const prev = selectedNodeRef.current;
    const prevData = (prev?.data as Record<string, unknown> | undefined) ?? undefined;
    const nextData = selectedNode.data as Record<string, unknown>;
    // Check if meaningful properties have changed (not just position)
    if (
      !prev ||
      prev.id !== selectedNode.id ||
      prev.type !== selectedNode.type ||
      prevData?.['kind'] !== nextData['kind'] ||
      prevData?.['label'] !== nextData['label'] ||
      prevData?.['sessionId'] !== nextData['sessionId'] ||
      prevData?.['isStaged'] !== nextData['isStaged'] ||
      !deepEqual(prevData?.['state'], nextData['state']) ||
      !deepEqual(prevData?.['params'], nextData['params'])
    ) {
      selectedNodeRef.current = selectedNode;
    }
    return selectedNodeRef.current;
  }, [selectedNode]);

  // Map definitions by kind for quick lookup (must be declared before use)
  const defByKind = React.useMemo(() => {
    const map = new Map<string, NodeDefinition>();
    for (const def of nodeDefinitions) map.set(def.kind, def);
    return map;
  }, [nodeDefinitions]);

  const selectedNodeDefinition = (() => {
    if (!selectedNode) return null;
    const kind = (selectedNode.data as { kind?: string }).kind;
    if (!kind) return null;
    return defByKind.get(kind) ?? null;
  })();

  // Run validation whenever staged pipeline changes
  useEffect(() => {
    if (!selectedSessionId || !isInStagingMode || !stagedPipeline) return;

    viewsLogger.debug('Running validation');
    const errors = validatePipeline(stagedPipeline, defByKind);
    setValidationErrors(selectedSessionId, errors);

    // Show toast for validation errors
    const errorCount = errors.filter((e) => e.type === 'error').length;
    if (errorCount > 0) {
      toast.error(
        `Validation failed: ${errorCount} error${errorCount > 1 ? 's' : ''} found. Fix them before committing.`
      );
    }
  }, [selectedSessionId, isInStagingMode, stagedPipeline, defByKind, setValidationErrors, toast]);

  // Keep a ref to the latest nodes for callbacks that need them
  const nodesRef = React.useRef(nodes);
  React.useEffect(() => {
    nodesRef.current = nodes;
  }, [nodes]);

  // Get global WebSocket connection status
  const { isConnected: globalIsConnected } = useWebSocket();

  // Fetch session list
  const { data: sessions = [], isLoading: isLoadingSessions } = useSessionList();

  // Memoize the selected session to prevent unnecessary re-renders
  // Uses a ref to store previous value and only updates when data actually changes (deep comparison)
  const prevSelectedSessionRef = React.useRef<
    { id: string; name: string | null; created_at: string } | undefined
  >(undefined);
  const selectedSession = React.useMemo(() => {
    const found = sessions.find((s) => s.id === selectedSessionId);
    const prev = prevSelectedSessionRef.current;

    // If both are undefined/null, return undefined
    if (!found && !prev) return undefined;

    // If one is undefined, update and return the new value
    if (!found || !prev) {
      prevSelectedSessionRef.current = found;
      return found;
    }

    // Deep comparison: if all fields match, return previous reference to prevent re-renders
    if (found.id === prev.id && found.name === prev.name && found.created_at === prev.created_at) {
      return prev;
    }

    // Data changed, update ref and return new value
    prevSelectedSessionRef.current = found;
    return found;
  }, [sessions, selectedSessionId]);

  // Prefetch pipeline data for all sessions to enable status display
  useSessionsPrefetch(sessions);

  // Subscribe to selected session
  const {
    pipeline,
    nodeStates,
    // nodeStats not used here - NodeStateIndicator fetches directly from session store
    isConnected: sessionIsConnected,
    isLoading: isLoadingPipeline,
    tuneNode,
    addNode,
    removeNode,
    connectPins,
    disconnectPins,
    applyBatch,
  } = useSession(selectedSessionId);

  // Use session-specific connection status if a session is selected, otherwise use global
  const isConnected = selectedSessionId ? sessionIsConnected : globalIsConnected;

  // Handle entering staging mode
  // Use ref to avoid recreating callback when pipeline changes
  const pipelineRef = useRef(pipeline);
  pipelineRef.current = pipeline;

  const handleEnterStagingMode = useCallback(() => {
    viewsLogger.info('Entering staging mode');
    if (!selectedSessionId || !pipelineRef.current) return;
    enterStagingMode(selectedSessionId, pipelineRef.current);

    // Capture all current node positions from the canvas
    nodesRef.current.forEach((node) => {
      updateNodePosition(selectedSessionId, node.id, node.position);
    });
  }, [selectedSessionId, enterStagingMode, updateNodePosition]);

  // Handle discarding staged changes
  const handleDiscardChanges = useCallback(() => {
    viewsLogger.info('Discarding changes (exiting staging mode)');
    if (!selectedSessionId) return;
    discardChanges(selectedSessionId);
  }, [selectedSessionId, discardChanges]);

  // Handle committing staged changes
  // Use ref to avoid recreating callback when pipeline/stagingData changes
  const stagingDataRef = useRef(stagingData);
  stagingDataRef.current = stagingData;

  const handleCommitChanges = useCallback(async () => {
    const currentPipeline = pipelineRef.current;
    const currentStagingData = stagingDataRef.current;

    if (!selectedSessionId || !currentPipeline || !currentStagingData?.stagedPipeline) return;

    const stagedPipeline = currentStagingData.stagedPipeline;

    try {
      // Compute the differences between live and staged pipelines using helper functions
      const operations: BatchOperation[] = [
        ...computeAddedNodes(stagedPipeline, currentPipeline),
        ...computeRemovedNodes(stagedPipeline, currentPipeline),
        ...computeConnectionChanges(stagedPipeline, currentPipeline),
      ];

      if (operations.length === 0) {
        toast.info('No changes to commit');
        return;
      }

      // Pre-process mixer nodes to set num_inputs based on actual connections
      preprocessMixerNodes(operations);

      // Apply the batch
      const response = await applyBatch(operations);

      if (response?.payload?.action === 'batchapplied') {
        if (response.payload.success) {
          toast.success(`Successfully applied ${operations.length} changes`);
          exitStagingMode(selectedSessionId);
        } else {
          const errors = response.payload.errors || ['Unknown error'];
          toast.error(`Failed to apply changes: ${errors.join(', ')}`);
        }
      } else {
        toast.error('Unexpected response from server');
      }
    } catch (error) {
      viewsLogger.error('Failed to commit changes:', error);
      toast.error('Failed to commit changes');
    }
  }, [selectedSessionId, applyBatch, toast, exitStagingMode]);

  // Helper to validate parameter value against schema
  const validateParamValue = useCallback(
    (nodeId: string, paramKey: string, value: unknown): string | null => {
      const node = pipeline?.nodes[nodeId];
      if (!node) return null;

      const nodeDef = nodeDefinitions.find((d) => d.kind === node.kind);
      if (!nodeDef) return null;

      const schema = nodeDef.param_schema as
        | {
            properties?: Record<
              string,
              { type?: string; minimum?: number; maximum?: number; multipleOf?: number }
            >;
          }
        | undefined;
      const propSchema = schema?.properties?.[paramKey];
      if (!propSchema) return null;

      return validateValue(value, propSchema);
    },
    [pipeline, nodeDefinitions]
  );

  // Memoized param change handler for right pane
  const handleRightPaneParamChange = useCallback(
    (nodeId: string, key: string, value: unknown) => {
      const isStaged = isInStagingMode && stagingData?.stagedNodes.has(nodeId);
      if (isStaged && selectedSessionId) {
        updateStagedNodeParams(selectedSessionId, nodeId, { [key]: value });
        return;
      }

      // Validate before sending to server
      const error = validateParamValue(nodeId, key, value);
      if (error) {
        toast.error(`Invalid value for ${key}: ${error}`);
        return;
      }

      tuneNode(nodeId, key, value);
    },
    [
      isInStagingMode,
      stagingData?.stagedNodes,
      selectedSessionId,
      updateStagedNodeParams,
      validateParamValue,
      toast,
      tuneNode,
    ]
  );

  // Memoized label change handler (currently no-op)
  const handleRightPaneLabelChange = useCallback(() => {}, []);

  // Memoized handlers for TopControls to prevent re-renders
  const handleDeleteModalOpen = useCallback(() => {
    setShowDeleteModal(true);
  }, []);

  // Handle YAML changes in staging mode
  const handleYamlChange = useCallback(
    // eslint-disable-next-line max-statements -- YAML edits must preserve existing pin/handle ids (e.g. mixer `in_0`), which requires a multi-step transform.
    (yaml: string) => {
      if (!isInStagingMode || !selectedSessionId || !stagingData) return;

      // Mark that user is editing YAML to prevent automatic regeneration
      isEditingYamlRef.current = true;

      try {
        const parsed = load(yaml) as {
          nodes?: Record<
            string,
            {
              kind: string;
              params?: Record<string, unknown>;
              needs?: string | string[];
              ui?: unknown;
            }
          >;
        };

        if (!parsed || !parsed.nodes || typeof parsed.nodes !== 'object') {
          toast.error('Invalid YAML: Must contain a "nodes" object');
          return;
        }

        // Build nodes map
        const nodes: Record<string, Node> = {};
        Object.entries(parsed.nodes).forEach(([nodeName, nodeConfig]) => {
          nodes[nodeName] = {
            kind: nodeConfig.kind,
            params: nodeConfig.params || {},
            state: null,
          };
        });

        // Build connections from "needs" fields while preserving existing pin ids.
        // The YAML format intentionally omits pin names (for readability), but React Flow needs
        // concrete handle IDs (e.g. mixers use `in_0`, `in_1`, ... not `in`).
        const basePipelineForPins = stagingData.stagedPipeline ?? pipeline;
        const existingConnections = basePipelineForPins?.connections ?? [];

        const existingByPair = new Map<string, Connection[]>();
        for (const c of existingConnections) {
          const key = `${c.from_node}→${c.to_node}`;
          const arr = existingByPair.get(key);
          if (arr) arr.push(c);
          else existingByPair.set(key, [c]);
        }

        const parseDynamicIndex = (pin: string, prefix: string): number | null => {
          if (!pin.startsWith(prefix)) return null;
          const rest = pin.slice(prefix.length);
          if (!/^\d+$/.test(rest)) return null;
          const n = Number(rest);
          return Number.isFinite(n) ? n : null;
        };

        type DynamicPinCardinality = Extract<
          InputPin['cardinality'],
          { Dynamic: { prefix: string } }
        >;
        const isDynamicCardinality = (
          c: InputPin['cardinality'] | OutputPin['cardinality']
        ): c is DynamicPinCardinality => typeof c === 'object' && c !== null && 'Dynamic' in c;

        const getNodeDef = (nodeName: string) => {
          const kind = nodes[nodeName]?.kind;
          if (!kind) return undefined;
          return nodeDefinitions.find((d) => d.kind === kind);
        };

        const pickSourcePin = (sourceNode: string): string => {
          const def = getNodeDef(sourceNode);
          const outputs = def?.outputs ?? [];

          // Prefer a concrete `out` pin when present.
          const outPin = outputs.find(
            (p) => p.name === 'out' && !isDynamicCardinality(p.cardinality)
          );
          if (outPin) return outPin.name;

          // Otherwise, if there's exactly one concrete output pin, use it.
          const concreteOutputs = outputs.filter((p) => !isDynamicCardinality(p.cardinality));
          if (concreteOutputs.length === 1) return concreteOutputs[0].name;

          // Fallback for unknown defs.
          return 'out';
        };

        const usedDynamicInputsByNode = new Map<string, Set<number>>();
        const noteExistingDynamicInput = (toNode: string, pinName: string) => {
          const def = getNodeDef(toNode);
          const dyn = def?.inputs.find((p) => isDynamicCardinality(p.cardinality));
          if (!dyn) return;
          if (!isDynamicCardinality(dyn.cardinality)) return;
          const prefix = dyn.cardinality.Dynamic.prefix;
          const idx = parseDynamicIndex(pinName, prefix);
          if (idx === null) return;
          let set = usedDynamicInputsByNode.get(toNode);
          if (!set) {
            set = new Set();
            usedDynamicInputsByNode.set(toNode, set);
          }
          set.add(idx);
        };

        for (const c of existingConnections) {
          noteExistingDynamicInput(c.to_node, c.to_pin);
        }

        const allocateTargetPin = (targetNode: string): string => {
          const def = getNodeDef(targetNode);
          const inputs = def?.inputs ?? [];

          // Prefer concrete `in` when present.
          const inPin = inputs.find((p) => p.name === 'in' && !isDynamicCardinality(p.cardinality));
          if (inPin) return inPin.name;

          // If the node has dynamic inputs, allocate `prefix<index>` (e.g. `in_0`).
          const dyn = inputs.find((p) => isDynamicCardinality(p.cardinality));
          if (dyn && isDynamicCardinality(dyn.cardinality)) {
            const prefix = dyn.cardinality.Dynamic.prefix;
            let used = usedDynamicInputsByNode.get(targetNode);
            if (!used) {
              used = new Set();
              usedDynamicInputsByNode.set(targetNode, used);
            }
            let i = 0;
            while (used.has(i)) i++;
            used.add(i);
            return `${prefix}${i}`;
          }

          // Otherwise, if there's exactly one concrete input pin, use it.
          const concreteInputs = inputs.filter((p) => !isDynamicCardinality(p.cardinality));
          if (concreteInputs.length === 1) return concreteInputs[0].name;

          // Fallback for unknown defs.
          return 'in';
        };

        const connections: Connection[] = [];
        const consumedPerPair = new Map<string, number>();
        Object.entries(parsed.nodes).forEach(([nodeName, nodeConfig]) => {
          if (!nodeConfig.needs) return;
          const needs = Array.isArray(nodeConfig.needs) ? nodeConfig.needs : [nodeConfig.needs];
          needs.forEach((sourceNode) => {
            // Only connect nodes that exist in this YAML snapshot
            if (!(sourceNode in nodes) || !(nodeName in nodes)) return;

            const pairKey = `${sourceNode}→${nodeName}`;
            const existing = existingByPair.get(pairKey);
            const consumed = consumedPerPair.get(pairKey) ?? 0;

            // Reuse existing pin names when possible to keep the graph stable on param-only edits.
            if (existing && consumed < existing.length) {
              const reused = existing[consumed];
              consumedPerPair.set(pairKey, consumed + 1);
              connections.push(reused);
              noteExistingDynamicInput(nodeName, reused.to_pin);
              return;
            }

            const from_pin = pickSourcePin(sourceNode);
            const to_pin = allocateTargetPin(nodeName);
            connections.push({
              from_node: sourceNode,
              from_pin,
              to_node: nodeName,
              to_pin,
            });
          });
        });

        const newPipeline: Pipeline = {
          name: null,
          description: null,
          mode: 'dynamic',
          nodes,
          connections,
        };

        // Determine which nodes are new (not in original live pipeline)
        const liveNodeNames = new Set(Object.keys(pipeline?.nodes || {}));
        const stagedNodes = new Set<string>();
        Object.keys(nodes).forEach((name) => {
          if (!liveNodeNames.has(name)) {
            stagedNodes.add(name);
          }
        });

        // Sync params to nodeParamsStore for immediate UI updates
        const paramsStore = useNodeParamsStore.getState();
        Object.entries(nodes).forEach(([nodeName, node]) => {
          if (node.params) {
            Object.entries(node.params).forEach(([key, value]) => {
              paramsStore.setParam(nodeName, key, value, selectedSessionId ?? undefined);
            });
          }
        });

        // Update staging store
        useStagingStore.setState((state) => {
          const data = state.staging[selectedSessionId];
          if (!data) return state;

          return {
            staging: {
              ...state.staging,
              [selectedSessionId]: {
                ...data,
                stagedPipeline: newPipeline,
                stagedNodes,
                version: data.version + 1,
              },
            },
          };
        });

        // After all state updates, check for tunable param changes in live nodes and dispatch tune events
        // We do this after staging store update to ensure state is consistent
        Object.entries(nodes).forEach(([nodeName, newNode]) => {
          // Skip staged nodes - they'll be applied when staging is committed
          if (stagedNodes.has(nodeName)) return;

          const oldNode = pipeline?.nodes[nodeName];
          if (!oldNode) return;

          // Get the node definition to check which params are tunable
          const nodeDef = nodeDefinitions.find((d) => d.kind === newNode.kind);
          if (!nodeDef) return;

          const schema = nodeDef.param_schema as
            | { properties?: Record<string, { tunable?: boolean }> }
            | undefined;
          const properties = schema?.properties;
          if (!properties) return;

          // Compare old and new params
          // Type guard: ensure params are objects with explicit typing
          const oldParams: Record<string, unknown> =
            oldNode.params && typeof oldNode.params === 'object' && !Array.isArray(oldNode.params)
              ? (oldNode.params as Record<string, unknown>)
              : {};
          const newParams: Record<string, unknown> =
            newNode.params && typeof newNode.params === 'object' && !Array.isArray(newNode.params)
              ? (newNode.params as Record<string, unknown>)
              : {};

          Object.entries(newParams).forEach(([paramKey, newValue]) => {
            // Check if this param is tunable
            const propSchema = properties[paramKey];
            if (!propSchema?.tunable) return;

            // Check if value changed
            const oldValue = oldParams[paramKey];
            if (!deepEqual(oldValue, newValue)) {
              // Validate before sending
              const validationError = validateValue(newValue, propSchema);
              if (validationError) {
                toast.error(`Invalid value for ${nodeName}.${paramKey}: ${validationError}`);
                return;
              }

              // Dispatch tune event for this live, tunable param
              viewsLogger.debug(
                `YAML edit: tuning live node ${nodeName}.${paramKey} from ${JSON.stringify(oldValue)} to ${JSON.stringify(newValue)}`
              );
              tuneNode(nodeName, paramKey, newValue);
            }
          });
        });

        // Clear the editing flag after a short delay to allow automatic updates to resume
        // This prevents the YAML from being overwritten during active editing
        setTimeout(() => {
          isEditingYamlRef.current = false;
        }, 1000);
      } catch (error) {
        viewsLogger.error('Failed to parse YAML:', error);
        toast.error(`Invalid YAML: ${error instanceof Error ? error.message : String(error)}`);
        // Clear editing flag even on error
        isEditingYamlRef.current = false;
      }
    },
    [isInStagingMode, selectedSessionId, stagingData, pipeline, toast, nodeDefinitions, tuneNode]
  );

  // Topology signature: only changes when nodes/kinds or connections change
  // Use staged pipeline when in staging mode, otherwise use live pipeline
  // Memoize based on the actual pipeline content to avoid re-renders when references change
  const topoKey = React.useMemo(() => {
    const activePipeline = isInStagingMode && stagedPipeline ? stagedPipeline : pipeline;
    if (!activePipeline) return '';
    const names = Object.keys(activePipeline.nodes).sort();
    const kinds = names.map((n) => `${n}:${activePipeline.nodes[n].kind}`);
    const conns = activePipeline.connections
      .map((c: Connection) => `${c.from_node}:${c.from_pin}>${c.to_node}:${c.to_pin}`)
      .sort();
    const key = JSON.stringify([kinds, conns]);
    viewsLogger.debug(
      'topoKey recalculated:',
      key.substring(0, 100),
      'isInStagingMode:',
      isInStagingMode
    );
    return key;
  }, [stagedPipeline, pipeline, isInStagingMode]);

  const onConnect = React.useCallback(
    (connection: RFConnection) => {
      return createOnConnect(
        nodesRefForCallbacks.current,
        setEdges,
        (conn: RFConnection) => {
          const from_pin = conn.sourceHandle || 'out';
          const to_pin = conn.targetHandle || 'in';

          if (isInStagingMode && selectedSessionId) {
            // Add to staging store instead of sending to server
            addStagedConnection(selectedSessionId, {
              from_node: conn.source,
              from_pin,
              to_node: conn.target,
              to_pin,
            });
          } else {
            // Send to server immediately (monitor mode)
            connectPins(conn.source, from_pin, conn.target, to_pin);
          }
        },
        edgesRefForCallbacks.current,
        setNodes
      )(connection);
    },
    [
      createOnConnect,
      setEdges,
      isInStagingMode,
      selectedSessionId,
      addStagedConnection,
      connectPins,
      setNodes,
    ]
  );

  const onEdgesDelete = (deleted: Edge[]) => {
    deleted.forEach((e) => {
      const from_pin = e.sourceHandle || 'out';
      const to_pin = e.targetHandle || 'in';

      if (isInStagingMode && selectedSessionId) {
        // Remove from staging store
        removeStagedConnection(selectedSessionId, {
          from_node: e.source,
          from_pin,
          to_node: e.target,
          to_pin,
        });
      } else {
        // Send to server immediately (monitor mode)
        disconnectPins(e.source, from_pin, e.target, to_pin);
      }
    });
  };

  const onNodesDelete = (deleted: RFNode[]) => {
    deleted.forEach((n) => {
      if (isInStagingMode && selectedSessionId) {
        // Remove from staging store
        removeStagedNode(selectedSessionId, n.id);
      } else {
        // Send to server immediately (monitor mode)
        removeNode(n.id);
      }
    });
  };

  // Deletion is handled by React Flow's built-in delete key via onNodesDelete/onEdgesDelete.

  // Helpers to add nodes with sensible defaults
  const generateName = (kind: string) => {
    const existing = pipeline ? Object.keys(pipeline.nodes) : [];
    let i = 1;
    let candidate = `${kind}_${i}`;
    while (existing.includes(candidate)) {
      i += 1;
      candidate = `${kind}_${i}`;
    }
    return candidate;
  };

  const defaultParamsForKind = (kind: string): Record<string, unknown> => {
    const def = nodeDefinitions.find((d) => d.kind === kind);
    const params: Record<string, unknown> = {};
    const schema = def?.param_schema as Record<string, unknown> | undefined;
    const props = schema?.properties as Record<string, Record<string, unknown>> | undefined;
    if (props) {
      Object.entries(props).forEach(([key, propSchema]) => {
        if (propSchema && typeof propSchema === 'object' && 'default' in propSchema) {
          const defVal = propSchema.default;
          if (defVal !== undefined) {
            params[key] = defVal;
          }
        }
      });
    }
    return params;
  };

  const onDragStart = useCallback(
    (event: React.DragEvent, nodeType: string) => {
      setType(nodeType);
      event.dataTransfer.setData('text/plain', nodeType);
      event.dataTransfer.effectAllowed = 'move';
    },
    [setType]
  );

  // Track previous topoKey to avoid unnecessary rebuilds
  const prevTopoKeyForTopologyRef = useRef<string>('');

  // Helper: Resolve node position from various sources (previous, pending, saved, or default)
  const resolveNodePosition = useCallback(
    (
      nodeName: string,
      prevPositions: Map<string, { x: number; y: number }>,
      savedPositions: Record<string, { x: number; y: number }>
    ): { position: { x: number; y: number }; fromPending: boolean } => {
      let pos = prevPositions.get(nodeName);
      let fromPending = false;

      // Check pending positions from node drops
      if (!pos && pendingNodePositions.current.has(nodeName)) {
        pos = pendingNodePositions.current.get(nodeName)!;
        pendingNodePositions.current.delete(nodeName);
        fromPending = true;
      }

      // Check saved positions from staging store
      if (!pos && savedPositions[nodeName]) {
        pos = savedPositions[nodeName];
      }

      return {
        position: pos ?? { x: 0, y: 0 },
        fromPending,
      };
    },
    []
  );

  /**
   * Helper to reconstruct dynamic input pins from connections and previous state.
   * Reduces nesting by extracting pin reconstruction logic.
   */
  const reconstructDynamicInputs = useCallback(
    (
      nodeName: string,
      dynamicTemplate: InputPin,
      activePipeline: Pipeline,
      prevNode: RFNode | undefined
    ): InputPin[] => {
      const dynamicPins = new Map<string, InputPin>();

      // Add pins from active connections
      const incomingConnections = activePipeline.connections.filter(
        (conn) => conn.to_node === nodeName
      );
      for (const conn of incomingConnections) {
        if (/^in_\d+$/.test(conn.to_pin)) {
          dynamicPins.set(conn.to_pin, {
            name: conn.to_pin,
            accepts_types: dynamicTemplate.accepts_types,
            cardinality: 'One',
          });
        }
      }

      // Preserve disconnected dynamic pins from previous state
      const prevInputs = prevNode?.data.inputs as InputPin[] | undefined;
      if (prevInputs) {
        for (const pin of prevInputs) {
          if (pin.cardinality === 'One' && /^in_\d+$/.test(pin.name)) {
            if (!dynamicPins.has(pin.name)) {
              dynamicPins.set(pin.name, pin);
            }
          }
        }
      }

      return Array.from(dynamicPins.values());
    },
    []
  );

  /**
   * Helper to reconstruct dynamic output pins from connections and previous state.
   * Reduces nesting by extracting pin reconstruction logic.
   */
  const reconstructDynamicOutputs = useCallback(
    (
      nodeName: string,
      dynamicTemplate: OutputPin,
      activePipeline: Pipeline,
      prevNode: RFNode | undefined
    ): OutputPin[] => {
      const dynamicPins = new Map<string, OutputPin>();

      // Add pins from active connections
      const outgoingConnections = activePipeline.connections.filter(
        (conn) => conn.from_node === nodeName
      );
      for (const conn of outgoingConnections) {
        if (/^out_\d+$/.test(conn.from_pin)) {
          dynamicPins.set(conn.from_pin, {
            name: conn.from_pin,
            produces_type: dynamicTemplate.produces_type,
            cardinality: 'One',
          });
        }
      }

      // Preserve disconnected dynamic pins from previous state
      const prevOutputs = prevNode?.data.outputs as OutputPin[] | undefined;
      if (prevOutputs) {
        for (const pin of prevOutputs) {
          if (/^out_\d+$/.test(pin.name)) {
            if (!dynamicPins.has(pin.name)) {
              dynamicPins.set(pin.name, pin);
            }
          }
        }
      }

      return Array.from(dynamicPins.values());
    },
    []
  );

  // Helper: Resolve dynamic pins for nodes that support them
  const resolveDynamicPins = useCallback(
    (
      nodeDefinition: NodeDefinition | undefined,
      nodeName: string,
      activePipeline: Pipeline,
      baseInputs: InputPin[],
      baseOutputs: OutputPin[]
    ): { finalInputs: InputPin[]; finalOutputs: OutputPin[] } => {
      const hasDynamicInputs =
        nodeDefinition?.inputs.some(
          (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
        ) ?? false;
      const hasDynamicOutputs =
        nodeDefinition?.outputs.some(
          (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
        ) ?? false;

      let finalInputs = baseInputs;
      let finalOutputs = baseOutputs;

      // Reconstruct dynamic input pins from connections
      if (hasDynamicInputs) {
        const dynamicTemplate = nodeDefinition?.inputs.find(
          (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
        );

        if (dynamicTemplate) {
          const prevNode = nodes.find((n) => n.id === nodeName);
          const dynamicInputs = reconstructDynamicInputs(
            nodeName,
            dynamicTemplate,
            activePipeline,
            prevNode
          );
          finalInputs = [...baseInputs, ...dynamicInputs];
        }
      }

      // Reconstruct dynamic output pins from connections
      if (hasDynamicOutputs) {
        const dynamicTemplate = nodeDefinition?.outputs.find(
          (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
        );

        if (dynamicTemplate) {
          const prevNode = nodes.find((n) => n.id === nodeName);
          const dynamicOutputs = reconstructDynamicOutputs(
            nodeName,
            dynamicTemplate,
            activePipeline,
            prevNode
          );
          finalOutputs = [...baseOutputs, ...dynamicOutputs];
        }
      }

      return { finalInputs, finalOutputs };
    },
    [nodes, reconstructDynamicInputs, reconstructDynamicOutputs]
  );

  // Update nodes and edges when pipeline topology changes (nodes added/removed/reconnected)
  // Other state updates (nodeStates, nodeStats, params) are handled by separate lightweight effects
  /**
   * This effect has 38 statements and complexity of 21, which exceeds limits.
   * The complexity is inherent to the task of building a React Flow graph from a pipeline:
   * - Early returns for optimization (topoKey check, no pipeline case)
   * - Topological sorting to get node order
   * - Iterating through nodes to build Node objects with position, state, pins
   * - Building edges with validation
   * - Generating YAML representation
   * Helper functions have been extracted where possible, but further breakdown
   * would fragment the graph-building logic across multiple locations.
   */
  // eslint-disable-next-line max-statements -- Core graph-building logic
  useEffect(() => {
    viewsLogger.debug(
      'Topology effect check, prev:',
      prevTopoKeyForTopologyRef.current.substring(0, 30),
      'curr:',
      topoKey.substring(0, 30),
      'match:',
      prevTopoKeyForTopologyRef.current === topoKey
    );

    // Skip if topoKey hasn't actually changed (entering/exiting staging with same topology)
    if (prevTopoKeyForTopologyRef.current === topoKey && nodes.length > 0) {
      viewsLogger.debug('Skipping topology effect, topoKey unchanged');
      return;
    }
    prevTopoKeyForTopologyRef.current = topoKey;

    // Use staged pipeline when in staging mode, otherwise use live pipeline
    const activePipeline =
      isInStagingMode && stagingData?.stagedPipeline ? stagingData.stagedPipeline : pipeline;

    if (!activePipeline) {
      viewsLogger.debug('Topology effect: No pipeline, clearing nodes');
      setNodes([]);
      setEdges([]);
      setYamlString('');
      return;
    }

    viewsLogger.debug('Topology effect triggered, topoKey:', topoKey.substring(0, 50) + '...');

    // Preserve existing node positions; do not auto-layout during edits.
    const { levels, sortedLevels } = topoLevelsFromPipeline(activePipeline);
    const orderedNames = orderedNamesFromLevels(levels, sortedLevels);

    const prevPositions = new Map(nodes.map((n) => [n.id, n.position]));

    // Get saved positions from staging store if in staging mode
    const savedPositions =
      isInStagingMode && selectedSessionId ? getNodePositions(selectedSessionId) : {};

    const newNodes: RFNode[] = [];
    for (const nodeName of orderedNames) {
      const apiNode = activePipeline.nodes[nodeName];
      if (!apiNode) continue;

      // Resolve node position from various sources
      const { position: pos, fromPending: positionFromPending } = resolveNodePosition(
        nodeName,
        prevPositions,
        savedPositions
      );

      // Save position to staging store if it came from pending (newly dropped) and we're in staging mode
      if (positionFromPending && isInStagingMode && selectedSessionId) {
        updateNodePosition(selectedSessionId, nodeName, pos);
      }

      // Use real-time state from Zustand if available, otherwise use pipeline state
      const nodeState = nodeStates[nodeName] || apiNode.state;

      // Determine if this node is staged (for visual distinction)
      const isStaged = isInStagingMode && stagingData?.stagedNodes.has(nodeName);

      // Get base pins from definition and resolve dynamic pins
      const baseInputs = defByKind.get(apiNode.kind)?.inputs ?? [];
      const baseOutputs = defByKind.get(apiNode.kind)?.outputs ?? [];
      const nodeDefinition = defByKind.get(apiNode.kind);

      const { finalInputs, finalOutputs } = resolveDynamicPins(
        nodeDefinition,
        nodeName,
        activePipeline,
        baseInputs,
        baseOutputs
      );

      const nodeDef = defByKind.get(apiNode.kind);

      // Build node object using helper function
      const node = buildNodeObject({
        nodeName,
        apiNode,
        position: pos,
        nodeState,
        isStaged,
        finalInputs,
        finalOutputs,
        nodeDef,
        stableOnParamChange,
        selectedSessionId,
      });

      newNodes.push(node);
    }

    // Build edges using helper function
    const newEdges = buildEdgesFromConnections(activePipeline.connections, newNodes);

    viewsLogger.debug('Setting', newNodes.length, 'nodes and', newEdges.length, 'edges');
    // Batch node and edge updates to prevent double render
    React.startTransition(() => {
      setNodes(newNodes);
      setEdges(newEdges);
      topoEffectRanRef.current = true;
    });

    // Generate YAML using helper function
    const yamlString = generatePipelineYaml(activePipeline, orderedNames);
    setYamlString(yamlString);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [topoKey, defByKind, selectedSessionId, tuneNode]);

  // Track previous topoKey to avoid redundant patch effect when topology changes
  const prevTopoKeyRef = useRef<string>('');
  const topoEffectRanRef = useRef(false);
  const isInitialMountRef = useRef(true);

  // Lightweight patch: update node params/state without rebuilding layout or edges
  // Skip when topology effect will run (topoKey changed)
  useEffect(() => {
    if (!pipeline) return;

    // Skip on initial mount - let topology effect handle everything
    if (isInitialMountRef.current) {
      viewsLogger.debug('Skipping patch effect on initial mount');
      isInitialMountRef.current = false;
      prevTopoKeyRef.current = topoKey;
      return;
    }

    // If topoKey changed, the topology effect will handle the full rebuild, skip this patch
    if (prevTopoKeyRef.current !== topoKey) {
      viewsLogger.debug(
        'Skipping patch effect, topology changed (prev:',
        prevTopoKeyRef.current.substring(0, 30),
        'new:',
        topoKey.substring(0, 30),
        ')'
      );
      prevTopoKeyRef.current = topoKey;
      return;
    }

    // Also skip if topology effect hasn't run yet
    if (!topoEffectRanRef.current) {
      viewsLogger.debug('Skipping patch effect, waiting for topology effect');
      return;
    }

    viewsLogger.debug('Running patch effect');

    // Use startTransition to mark this as low-priority update
    React.startTransition(() => {
      setNodes((prev) => {
        const updatesById = new Map<
          string,
          { nextState: unknown; nextParams: Record<string, unknown> }
        >();

        for (const n of prev) {
          const apiNode = pipeline.nodes[n.id];
          if (!apiNode) continue;

          const nextState = nodeStates[n.id] ?? apiNode.state;
          // Type guard: ensure params is an object of type Record<string, unknown>
          const nextParams: Record<string, unknown> =
            apiNode.params && typeof apiNode.params === 'object' && !Array.isArray(apiNode.params)
              ? (apiNode.params as Record<string, unknown>)
              : EMPTY_PARAMS;

          // Deep comparison: check if state or params actually changed
          const stateChanged = !deepEqual(n.data.state, nextState);
          const paramsChanged = !deepEqual(n.data.params, nextParams);

          if (stateChanged || paramsChanged) {
            updatesById.set(n.id, { nextState, nextParams });
          }
        }

        // Only create new array if there are actual changes
        if (updatesById.size === 0) {
          viewsLogger.debug('Patch effect: no changes, returning same array');
          return prev; // Return same array reference - no re-render!
        }

        // Create updated array
        const updated = prev.map((n) => {
          const updateInfo = updatesById.get(n.id);
          if (!updateInfo) return n;

          return {
            ...n,
            data: {
              ...n.data,
              state: updateInfo.nextState,
              params: updateInfo.nextParams,
              // Note: stats are NOT updated here to prevent re-renders
              // NodeStateIndicator fetches them directly from the session store
            },
          };
        });

        viewsLogger.debug('Patch effect updated', updatesById.size, 'nodes');
        return updated;
      });
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    pipeline,
    // stagingData removed - not needed for state/params updates
    // isInStagingMode removed - topology effect handles mode changes
    nodeStates,
    topoKey, // Need this to know when topology changed
    // nodeStats removed from dependencies - no longer causes re-renders
    // Note: setNodes, tuneNode, updateStagedNodeParams are stable and don't need to be dependencies
  ]);

  // Lightweight patch: update edge alerts based on node degraded details.
  useEffect(() => {
    if (!pipeline) return;

    const slowPinsByNode = new Map<string, Set<string>>();
    const slowDetailsByNode = new Map<string, SlowTimeoutDetails>();
    for (const [nodeId, apiNode] of Object.entries(pipeline.nodes)) {
      const state = (nodeStates as Record<string, NodeState>)[nodeId] ?? apiNode.state ?? null;
      const details = extractSlowTimeoutDetailsFromNodeState(state);
      const slowPins = details?.slowPins ?? [];
      if (slowPins.length > 0) {
        slowPinsByNode.set(nodeId, new Set(slowPins));
      }
      if (details) {
        slowDetailsByNode.set(nodeId, details);
      }
    }

    React.startTransition(() => {
      setEdges((prev) => {
        let changed = false;

        const next = prev.map((edge) => {
          const targetPin = edge.targetHandle ?? '';
          const shouldWarn = slowPinsByNode.get(edge.target)?.has(targetPin) ?? false;
          const currentAlert = isRecord(edge.data) ? edge.data['alert'] : undefined;
          const currentAlertKind =
            isRecord(currentAlert) && typeof currentAlert['kind'] === 'string'
              ? currentAlert['kind']
              : null;
          const isCurrentlyWarned = currentAlertKind === 'slow_input_timeout';

          if (shouldWarn === isCurrentlyWarned) return edge;

          changed = true;
          const nextData: Record<string, unknown> = { ...(edge.data || {}) };

          if (shouldWarn) {
            const details = slowDetailsByNode.get(edge.target);
            const slowPins = details?.slowPins ?? [];
            const slowInputs = pipeline ? describeSlowInputs(pipeline, edge.target, slowPins) : [];

            const lines: string[] = [];
            if (slowInputs.length > 0) {
              lines.push(`Slow inputs: ${slowInputs.join(', ')}`);
            } else if (slowPins.length > 0) {
              lines.push(`Slow pins: ${slowPins.join(', ')}`);
            }

            const sourceHandle = edge.sourceHandle ?? '';
            lines.push(`This: ${edge.source}.${sourceHandle} → ${edge.targetHandle ?? ''}`);

            if (details?.newlySlowPins && details.newlySlowPins.length > 0) {
              lines.push(`Newly slow: ${details.newlySlowPins.join(', ')}`);
            }
            if (details?.syncTimeoutMs !== null && details?.syncTimeoutMs !== undefined) {
              lines.push(`Timeout: ${details.syncTimeoutMs}ms`);
            }

            nextData.alert = {
              kind: 'slow_input_timeout',
              severity: 'warning',
              tooltip: {
                title: `${edge.target} degraded`,
                lines,
              },
            };
          } else if (isCurrentlyWarned) {
            delete nextData.alert;
          }

          return { ...edge, data: nextData };
        });

        return changed ? next : prev;
      });
    });
  }, [pipeline, nodeStates, setEdges]);

  // Create a stable callback that handles both staged and live param changes
  // This avoids recreating callbacks for each node, which would break React.memo
  const stableOnParamChange = useCallback(
    (nodeId: string, paramName: string, value: unknown) => {
      // Check at call time if we're in staging mode and if this node is staged
      const currentStagingData = useStagingStore.getState().staging[selectedSessionId || ''];
      const isCurrentlyInStagingMode = currentStagingData?.mode === 'staging';
      const isNodeStaged = isCurrentlyInStagingMode && currentStagingData?.stagedNodes.has(nodeId);

      if (isNodeStaged && selectedSessionId) {
        updateStagedNodeParams(selectedSessionId, nodeId, { [paramName]: value });
        return;
      }

      // Validate before sending to server
      const error = validateParamValue(nodeId, paramName, value);
      if (error) {
        toast.error(`Invalid value for ${paramName}: ${error}`);
        return;
      }

      tuneNode(nodeId, paramName, value);
    },
    [selectedSessionId, updateStagedNodeParams, validateParamValue, toast, tuneNode]
  );

  // In staging mode, keep each node's `isStaged` flag in sync with `stagedNodes`.
  useEffect(() => {
    if (nodes.length === 0 || !isInStagingMode) return;

    // Only update if there are actually staged nodes
    if (!stagingData?.stagedNodes || stagingData.stagedNodes.size === 0) return;

    viewsLogger.debug('Updating isStaged flags for', stagingData.stagedNodes.size, 'nodes');
    setNodes((prev) =>
      prev.map((n) => {
        const isNodeStaged = stagingData.stagedNodes.has(n.id);
        if (n.data.isStaged === isNodeStaged) {
          return n; // No change
        }
        return {
          ...n,
          data: {
            ...n.data,
            isStaged: isNodeStaged,
          },
        };
      })
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stagingData?.stagedNodes, nodes.length, isInStagingMode]);

  // NOTE: fitView is triggered only by:
  // 1. Auto-layout effect (when needsAutoLayout is true)
  // 2. needsFit effect (when needsFit is true)
  // Avoid auto-fitting on every node change to prevent disruption during editing.

  // Keep YAML up to date with live (Zustand) param overrides
  // Only runs when params change, not when nodes move
  useEffect(() => {
    // Skip YAML regeneration if user is actively editing to prevent overwriting their changes
    if (isEditingYamlRef.current) {
      return;
    }

    // Use staged pipeline when in staging mode, otherwise use live pipeline
    const activePipeline = isInStagingMode && stagedPipeline ? stagedPipeline : pipeline;

    if (!activePipeline) {
      setYamlString('');
      return;
    }

    const yamlObject: { nodes: Record<string, unknown> } = { nodes: {} };

    // Use topological order to keep YAML stable (not affected by canvas positions)
    const { levels, sortedLevels } = topoLevelsFromPipeline(activePipeline);
    const sortedNames = orderedNamesFromLevels(levels, sortedLevels);

    for (const nodeName of sortedNames) {
      const apiNode = activePipeline.nodes[nodeName];
      if (!apiNode) continue;

      const needs = activePipeline.connections
        .filter((c: Connection) => c.to_node === nodeName)
        .map((c: Connection) => c.from_node);

      const nodeConfig: Record<string, unknown> = { kind: apiNode.kind };

      const overrides = useNodeParamsStore
        .getState()
        .getParamsForNode(nodeName, selectedSessionId ?? undefined);
      const mergedParams = { ...(apiNode.params || {}), ...(overrides || {}) };
      if (Object.keys(mergedParams).length > 0) {
        nodeConfig['params'] = mergedParams;
      }

      if (needs.length === 1) {
        nodeConfig['needs'] = needs[0];
      } else if (needs.length > 1) {
        nodeConfig['needs'] = needs;
      }

      yamlObject.nodes[nodeName] = nodeConfig;
    }

    setYamlString(dump(yamlObject, { skipInvalid: true }));
  }, [pipeline, stagedPipeline, isInStagingMode, selectedSessionId]);

  const onConnectEnd: OnConnectEnd = React.useCallback(
    (event, connectionState) => {
      return createOnConnectEnd(nodesRefForCallbacks.current, edgesRefForCallbacks.current)(
        event,
        connectionState
      );
    },
    [createOnConnectEnd]
  );

  const handleDuplicateNode = (nodeId: string) => {
    // In monitor mode, we could potentially duplicate via WebSocket
    viewsLogger.debug('Duplicate node:', nodeId);
  };

  const handleDeleteNode = (nodeId: string) => {
    removeNode(nodeId);
  };

  const onDragOver = (event: React.DragEvent) => {
    event.preventDefault();
    event.dataTransfer.dropEffect = 'move';
  };

  const onDrop = (event: React.DragEvent) => {
    event.preventDefault();
    if (!type) {
      return;
    }

    // Calculate drop position in flow coordinates
    const position = screenToFlow({
      x: event.clientX,
      y: event.clientY,
    });

    const kind = type;
    const nodeId = generateName(kind);
    const params = defaultParamsForKind(kind);

    // Cache the position for when the node appears in the pipeline
    pendingNodePositions.current.set(nodeId, position);

    if (isInStagingMode && selectedSessionId) {
      // Add to staging store
      addStagedNode(selectedSessionId, nodeId, {
        kind,
        params: params as Record<string, unknown>,
        state: null,
      });
    } else {
      // Send to server immediately (monitor mode)
      addNode(nodeId, kind, params);
    }

    setType(null);
  };

  const handleSessionClick = useCallback(
    (sessionId: string) => {
      // Use startTransition to make session loading non-blocking
      // This allows the UI to stay responsive while loading heavy pipelines
      React.startTransition(() => {
        setSelectedSessionId(sessionId);
        // Check if this session has saved positions in staging store
        const savedPos = getNodePositions(sessionId);
        const hasPositions = Object.keys(savedPos).length > 0;

        viewsLogger.debug('Session click, hasPositions:', hasPositions);
        // Only auto-layout if no positions are saved
        setNeedsAutoLayout(!hasPositions);
        setNeedsFit(true);
      });
    },
    [getNodePositions]
  );

  const handleQuickDeleteSession = useCallback((sessionId: string) => {
    // Store which session to delete and show confirmation modal
    setSessionToDelete(sessionId);
  }, []);

  const handleConfirmQuickDelete = async () => {
    if (!sessionToDelete) return;

    setIsDeletingSession(true);

    try {
      const wsService = getWebSocketService();

      const response = await wsService.send({
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'destroysession' as const,
          session_id: sessionToDelete,
        },
      });

      if (response.payload.action === 'sessiondestroyed') {
        toast.success(`Session deleted successfully`);
        // If the deleted session was selected, clear selection
        if (selectedSessionId === sessionToDelete) {
          setSelectedSessionId(null);
        }
        setSessionToDelete(null);
      } else if (response.payload.action === 'error') {
        throw new Error(response.payload.message);
      }
    } catch (error) {
      viewsLogger.error('Failed to delete session:', error);
      toast.error(error instanceof Error ? error.message : 'Failed to delete session');
    } finally {
      setIsDeletingSession(false);
    }
  };

  const handleDeleteSession = async () => {
    if (!selectedSessionId) return;

    setIsDeletingSession(true);

    try {
      const wsService = getWebSocketService();

      const response = await wsService.send({
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'destroysession' as const,
          session_id: selectedSessionId,
        },
      });

      if (response.payload.action === 'sessiondestroyed') {
        toast.success(`Session ${selectedSessionId} deleted successfully`);
        setSelectedSessionId(null);
        setShowDeleteModal(false);
      } else if (response.payload.action === 'error') {
        throw new Error(response.payload.message);
      }
    } catch (error) {
      viewsLogger.error('Failed to delete session:', error);
      toast.error(error instanceof Error ? error.message : 'Failed to delete session');
    } finally {
      setIsDeletingSession(false);
    }
  };

  const applyAutoLayout = React.useCallback(
    (measuredHeights: Record<string, number>) => {
      if (!pipeline) return;

      const nodeWidth = DEFAULT_NODE_WIDTH;
      const hGap = DEFAULT_HORIZONTAL_GAP;
      const vGap = DEFAULT_VERTICAL_GAP;

      const { levels, sortedLevels } = topoLevelsFromPipeline(pipeline);

      const perNodeHeights: Record<string, number> = {};
      for (const name of Object.keys(pipeline.nodes)) {
        const measured = measuredHeights[name];
        if (typeof measured === 'number' && Number.isFinite(measured)) {
          perNodeHeights[name] = measured;
        } else {
          const kind = pipeline.nodes[name].kind;
          perNodeHeights[name] = ESTIMATED_HEIGHT_BY_KIND[kind] ?? DEFAULT_NODE_HEIGHT;
        }
      }

      const positions = verticalLayout(levels, sortedLevels, {
        nodeWidth,
        nodeHeight: DEFAULT_NODE_HEIGHT,
        hGap,
        vGap,
        heights: perNodeHeights,
        edges: pipeline.connections.map((c) => ({ source: c.from_node, target: c.to_node })),
      });

      viewsLogger.debug(
        'Applying auto-layout positions to',
        Object.keys(positions).length,
        'nodes'
      );

      setNodes((prev) =>
        prev.map((n) => {
          const newPos = positions[n.id];
          if (!newPos) return n;

          // Only create new object if position actually changed
          if (n.position.x === newPos.x && n.position.y === newPos.y) {
            return n;
          }

          return {
            ...n,
            position: newPos,
          };
        })
      );

      // Save auto-layout positions to staging store so we don't need to re-run layout next time
      if (selectedSessionId) {
        Object.entries(positions).forEach(([nodeId, position]) => {
          updateNodePosition(selectedSessionId, nodeId, position);
        });
        viewsLogger.debug(
          'Saved auto-layout positions for',
          Object.keys(positions).length,
          'nodes'
        );
      }

      // Wait for nodes to be positioned and rendered before fitting
      setTimeout(() => {
        viewsLogger.debug('Auto-layout complete, fitting view');
        // No animation for better performance on initial load
        rf.current?.fitView({ padding: 0.2, duration: 0 });
      }, 100);
    },
    [pipeline, setNodes, selectedSessionId, updateNodePosition]
  );

  const handleAutoLayout = React.useCallback(() => {
    if (!pipeline) return;

    const runLayout = () => {
      const measuredHeights = collectNodeHeights(rf.current);
      applyAutoLayout(measuredHeights);
    };

    if (typeof window !== 'undefined' && typeof window.requestAnimationFrame === 'function') {
      window.requestAnimationFrame(runLayout);
    } else {
      runLayout();
    }
  }, [pipeline, applyAutoLayout]);

  // Auto-layout when selecting a session: perform once after pipeline/nodes load
  useEffect(() => {
    if (needsAutoLayout && selectedSessionId && pipeline && nodes.length > 0) {
      viewsLogger.debug('Auto-layout requested');
      // Use requestIdleCallback to defer layout until browser is idle
      // This prevents blocking the UI during heavy renders
      const hasRequestIdleCallback = 'requestIdleCallback' in window;
      const idleCallback = hasRequestIdleCallback
        ? window.requestIdleCallback(
            () => {
              handleAutoLayout();
              setNeedsAutoLayout(false);
              setNeedsFit(false); // Auto-layout handles fitView, so clear this flag too
            },
            { timeout: 200 }
          )
        : setTimeout(() => {
            handleAutoLayout();
            setNeedsAutoLayout(false);
            setNeedsFit(false);
          }, 100);
      return () => {
        if (hasRequestIdleCallback) {
          window.cancelIdleCallback(idleCallback as number);
        } else {
          clearTimeout(idleCallback as number);
        }
      };
    }
  }, [needsAutoLayout, selectedSessionId, pipeline, nodes.length, handleAutoLayout]);

  // Fit view when selecting a session once nodes are present
  // Skip if auto-layout is running since it handles fitView itself
  useEffect(() => {
    if (needsFit && selectedSessionId && nodes.length > 0 && !needsAutoLayout) {
      viewsLogger.debug('FitView requested, waiting for nodes to settle');
      const t = setTimeout(() => {
        viewsLogger.debug('Fitting view to nodes');
        // No animation on initial load for better performance
        rf.current?.fitView({ padding: 0.2, duration: 0 });
        setNeedsFit(false);
      }, 150); // Increased delay to ensure nodes are positioned
      return () => clearTimeout(t);
    }
  }, [needsFit, selectedSessionId, nodes.length, needsAutoLayout]);

  // Register fitView callback for layout preset changes
  useFitViewOnLayoutPresetChange({
    reactFlowInstance: rf,
    nodesCount: nodes.length,
  });

  // Memoize left panel to prevent ResizableLayout from re-rendering
  const leftPanel = React.useMemo(
    () => (
      <LeftPanel
        isLoadingSessions={isLoadingSessions}
        sessions={sessions}
        selectedSessionId={selectedSessionId}
        onSessionClick={handleSessionClick}
        onSessionDelete={handleQuickDeleteSession}
        editMode={editMode}
        nodeDefinitions={nodeDefinitions}
        onDragStart={onDragStart}
        pluginKinds={pluginKinds}
        pluginTypes={pluginTypes}
      />
    ),
    [
      isLoadingSessions,
      sessions,
      selectedSessionId,
      handleSessionClick,
      handleQuickDeleteSession,
      editMode,
      nodeDefinitions,
      onDragStart,
      pluginKinds,
      pluginTypes,
    ]
  );

  // Memoize center panel to prevent ResizableLayout from re-rendering

  // - Only track nodes.length, not full nodes array (FlowCanvas handles position updates internally)
  // - Only track stagingData lengths, not full object (TopControls has its own memo optimization)
  // - Handlers are stable via refs and don't need to be tracked
  // - selectedSession used instead of sessions array to prevent unnecessary re-renders
  const centerPanel = React.useMemo(
    () => (
      <CenterPanelContainer>
        <CanvasTopBar>
          <TopLeftControls>
            <MonitorViewTitle />
            {selectedSession && <SessionInfoChip session={selectedSession} />}
          </TopLeftControls>
          <TopControls
            isConnected={isConnected}
            selectedSessionId={selectedSessionId}
            isInStagingMode={isInStagingMode}
            stagingData={stagingData}
            onCommit={handleCommitChanges}
            onDiscard={handleDiscardChanges}
            onEnterStaging={handleEnterStagingMode}
            onDelete={handleDeleteModalOpen}
          />
        </CanvasTopBar>
        {selectedSessionId && nodes.length > 0 ? (
          <>
            <FlowCanvas
              nodes={nodes}
              edges={edges}
              nodeTypes={nodeTypes}
              onNodesChange={onNodesChangeInternal}
              onEdgesChange={onEdgesChange}
              colorMode={colorMode}
              onInit={onInit}
              defaultEdgeOptions={defaultEdgeOptions}
              editMode={editMode}
              onNodeDragStop={onNodeDragStop}
              onNodeDoubleClick={handleNodeDoubleClick}
              isValidConnection={
                isValidConnection
                  ? (conn) =>
                      isValidConnection(
                        conn,
                        nodesRefForCallbacks.current,
                        edgesRefForCallbacks.current
                      )
                  : undefined
              }
              onConnect={onConnect}
              onConnectEnd={onConnectEnd}
              onEdgesDelete={onEdgesDelete}
              onNodesDelete={onNodesDelete}
              onPaneClick={onPaneClick}
              onPaneContextMenu={onPaneContextMenu}
              onNodeContextMenu={onNodeContextMenu}
              onDrop={onDrop}
              onDragOver={onDragOver}
              reactFlowWrapper={reactFlowWrapper}
            />
            <Legend />
          </>
        ) : (
          <EmptyMonitorState>
            {isLoadingPipeline ? (
              <p>Loading pipeline...</p>
            ) : (
              <p>Select a session from the left panel to inspect its pipeline.</p>
            )}
          </EmptyMonitorState>
        )}
      </CenterPanelContainer>
    ),
    // Intentional sparse dependencies for performance optimization:
    // - Only track nodes.length, not full nodes array (FlowCanvas handles position updates internally)
    // - Only track stagingData lengths, not full object (TopControls has its own memo optimization)
    // - Handlers are stable via refs and don't need to be tracked
    // - selectedSession used instead of sessions array to prevent unnecessary re-renders
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [
      selectedSessionId,
      selectedSession,
      isConnected,
      isInStagingMode,
      stagingData?.changes.length,
      stagingData?.validationErrors.length,
      nodes.length,
      colorMode,
      onInit,
      editMode,
      isLoadingPipeline,
    ]
  );

  // Extract selected node label for YAML highlighting
  const selectedNodeLabel = React.useMemo(() => {
    return stableSelectedNode?.data?.label as string | undefined;
  }, [stableSelectedNode]);

  // Memoize right panel to prevent ResizableLayout from re-rendering
  const rightPanel = React.useMemo(
    () =>
      selectedSessionId && pipeline ? (
        <PipelineRightPane
          selectedNode={
            stableSelectedNode as RFNode<{
              label: string;
              kind: string;
              params: Record<string, unknown>;
            }> | null
          }
          selectedNodeDefinition={selectedNodeDefinition}
          selectedNodeLabel={selectedNodeLabel}
          rightPaneView={rightPaneView}
          setRightPaneView={setRightPaneView}
          yamlString={yamlString}
          onYamlChange={isInStagingMode ? handleYamlChange : undefined}
          onParamChange={handleRightPaneParamChange}
          onLabelChange={handleRightPaneLabelChange}
          nodeDefinitions={nodeDefinitions}
          readOnly={!editMode}
          yamlReadOnly={!isInStagingMode}
          isMonitorView={true}
          sessionId={selectedSessionId}
        />
      ) : undefined,
    [
      selectedSessionId,
      pipeline,
      stableSelectedNode,
      selectedNodeDefinition,
      selectedNodeLabel,
      rightPaneView,
      setRightPaneView,
      yamlString,
      isInStagingMode,
      handleYamlChange,
      handleRightPaneParamChange,
      handleRightPaneLabelChange,
      nodeDefinitions,
      editMode,
    ]
  );

  return (
    <div style={{ height: '100%' }} data-testid="monitor-view">
      <ResizableLayout
        left={leftPanel}
        center={centerPanel}
        right={rightPanel}
        leftLabel="Sessions"
        centerLabel="Pipeline"
        rightLabel="Inspector"
      />
      {menu && (
        <ContextMenu
          onClick={onPaneClick}
          onDuplicate={handleDuplicateNode}
          onDelete={handleDeleteNode}
          {...menu}
        />
      )}
      {paneMenu && (
        <PaneContextMenu onClick={onPaneClick} onAutoLayout={handleAutoLayout} {...paneMenu} />
      )}
      <ConfirmModal
        isOpen={showDeleteModal}
        title="Delete Session"
        message={`Are you sure you want to delete session "${selectedSessionId}"? This action cannot be undone.`}
        confirmLabel="Delete"
        cancelLabel="Cancel"
        onConfirm={handleDeleteSession}
        onCancel={() => setShowDeleteModal(false)}
        isLoading={isDeletingSession}
      />
      <ConfirmModal
        isOpen={sessionToDelete !== null}
        title="Delete Session"
        message={`Are you sure you want to delete session "${sessionToDelete}"? This will stop the pipeline and all running nodes. This action cannot be undone.`}
        confirmLabel="Delete"
        cancelLabel="Cancel"
        onConfirm={handleConfirmQuickDelete}
        onCancel={() => setSessionToDelete(null)}
        isLoading={isDeletingSession}
      />
    </div>
  );
};

const MonitorView: React.FC = () => {
  return (
    <ReactFlowProvider>
      <DnDProvider>
        <MonitorViewContent />
      </DnDProvider>
    </ReactFlowProvider>
  );
};

export default MonitorView;
