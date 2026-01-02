// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React from 'react';

import { useSessionStore } from '@/stores/sessionStore';
import type { NodeState, NodeStats, Pipeline } from '@/types/types';

import { SKTooltip } from './Tooltip';

const StateIndicator = styled.div<{ color: string }>`
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background-color: ${(props) => props.color};
  border: 1px solid var(--sk-border-strong);
  box-shadow: 0 0 4px ${(props) => props.color}40;
`;

const ErrorBadge = styled.div`
  position: absolute;
  top: -2px;
  right: -2px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background-color: var(--sk-danger);
  border: 1px solid var(--sk-bg);
`;

const IndicatorWrapper = styled.div`
  position: relative;
  display: inline-flex;
`;

interface NodeStateIndicatorProps {
  state: NodeState;
  stats?: NodeStats;
  showLabel?: boolean;
  nodeId?: string; // If provided, will fetch stats from session store on demand
  sessionId?: string; // Required if nodeId is provided
}

function formatNumber(num: number | bigint): string {
  return num.toLocaleString();
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return (
    value !== null && value !== undefined && typeof value === 'object' && !Array.isArray(value)
  );
}

function asStringArray(value: unknown): string[] | null {
  if (!Array.isArray(value)) return null;
  if (!value.every((v) => typeof v === 'string')) return null;
  return value;
}

type SlowInputSource = {
  slowPin: string;
  fromNode: string;
  fromPin: string;
};

type DegradedPins = {
  slowPins: string[] | null;
  newlySlowPins: string[] | null;
};

function deriveSlowInputSources(
  pipeline: Pipeline | null | undefined,
  nodeId: string,
  slowPins: string[]
): SlowInputSource[] {
  if (!pipeline || slowPins.length === 0) return [];

  const slowPinSet = new Set(slowPins);
  const sources: SlowInputSource[] = [];

  for (const c of pipeline.connections) {
    if (c.to_node !== nodeId) continue;
    if (!slowPinSet.has(c.to_pin)) continue;
    sources.push({ slowPin: c.to_pin, fromNode: c.from_node, fromPin: c.from_pin });
  }

  sources.sort(
    (a, b) => a.slowPin.localeCompare(b.slowPin) || a.fromNode.localeCompare(b.fromNode)
  );
  return sources;
}

function getDegradedPins(details: unknown): DegradedPins {
  const detailsObj = isRecord(details) ? details : null;
  const slowPins = detailsObj ? asStringArray(detailsObj['slow_pins']) : null;
  const newlySlowPins = detailsObj ? asStringArray(detailsObj['newly_slow_pins']) : null;
  return { slowPins, newlySlowPins };
}

function getSlowSourcesForDegraded(
  context: { pipeline?: Pipeline | null; nodeId?: string } | undefined,
  slowPins: string[] | null
): SlowInputSource[] {
  if (!slowPins || slowPins.length === 0) return [];
  if (!context?.pipeline || !context.nodeId) return [];
  return deriveSlowInputSources(context.pipeline, context.nodeId, slowPins);
}

function renderSlowPinsSummary(
  slowPins: string[] | null,
  newlySlowPins: string[] | null,
  slowSources: SlowInputSource[]
): React.ReactNode {
  if (!slowPins && !newlySlowPins) return null;

  const hasSources = slowSources.length > 0;
  const hasPins = !!slowPins && slowPins.length > 0;
  const hasNewlySlow = !!newlySlowPins && newlySlowPins.length > 0;

  if (!hasSources && !hasPins && !hasNewlySlow) return null;

  return (
    <div style={{ marginTop: 6, color: 'var(--sk-warning)' }}>
      {hasSources ? (
        <div className="code-font" style={{ fontSize: 11, lineHeight: '1.4' }}>
          Slow inputs:{' '}
          {slowSources.map((s) => `${s.fromNode}.${s.fromPin} → ${s.slowPin}`).join(', ')}
        </div>
      ) : hasPins ? (
        <div className="code-font" style={{ fontSize: 11, lineHeight: '1.4' }}>
          Slow pins: {slowPins.join(', ')}
        </div>
      ) : null}
      {hasPins && hasSources && (
        <div className="code-font" style={{ fontSize: 11, lineHeight: '1.4', marginTop: 2 }}>
          Pins: {slowPins.join(', ')}
        </div>
      )}
      {hasNewlySlow && (
        <div className="code-font" style={{ fontSize: 11, lineHeight: '1.4', marginTop: 2 }}>
          Newly slow: {newlySlowPins.join(', ')}
        </div>
      )}
    </div>
  );
}

function getStateColor(state: NodeState): string {
  if (typeof state === 'string') {
    switch (state) {
      case 'Initializing':
        return 'var(--sk-status-initializing)';
      case 'Running':
        return 'var(--sk-status-running)';
      default:
        return 'var(--sk-status-stopped)';
    }
  }

  if ('Recovering' in state) {
    return 'var(--sk-status-recovering)';
  }
  if ('Degraded' in state) {
    return 'var(--sk-status-degraded)';
  }
  if ('Failed' in state) {
    return 'var(--sk-status-failed)';
  }
  if ('Stopped' in state) {
    return 'var(--sk-status-stopped)';
  }

  return 'var(--sk-status-stopped)';
}

function getStateLabel(state: NodeState): string {
  if (typeof state === 'string') {
    return state;
  }

  if ('Recovering' in state) {
    return 'Recovering';
  }
  if ('Degraded' in state) {
    return 'Degraded';
  }
  if ('Failed' in state) {
    return 'Failed';
  }
  if ('Stopped' in state) {
    return 'Stopped';
  }

  return 'Unknown';
}

function getStateDescription(state: NodeState): string {
  if (typeof state === 'string') {
    switch (state) {
      case 'Initializing':
        return 'Node is starting up and performing initialization';
      case 'Running':
        return 'Node is operating normally and processing data';
      default:
        return '';
    }
  }

  if ('Recovering' in state) {
    return 'Node encountered an issue but is actively attempting to recover';
  }
  if ('Degraded' in state) {
    return 'Node is operational but experiencing persistent issues';
  }
  if ('Failed' in state) {
    return 'Node has encountered a fatal error and stopped processing';
  }
  if ('Stopped' in state) {
    return 'Node has stopped processing and shut down';
  }

  return '';
}

const renderPacketStats = (stats?: NodeStats): React.ReactNode => {
  if (!stats) return null;

  const duration = stats.duration_secs > 0 ? stats.duration_secs : 1;
  const receivedPps = Math.round(Number(stats.received) / duration);
  const sentPps = Math.round(Number(stats.sent) / duration);
  const erroredPps = stats.errored > 0 ? Math.round(Number(stats.errored) / duration) : 0;
  const discardedPps = stats.discarded > 0 ? Math.round(Number(stats.discarded) / duration) : 0;

  return (
    <div style={{ marginTop: 8, paddingTop: 8, borderTop: '1px solid var(--sk-border)' }}>
      <div style={{ fontWeight: 600, marginBottom: 4, color: 'var(--sk-text)' }}>
        Packet Statistics
      </div>
      <div
        className="code-font"
        style={{ color: 'var(--sk-text)', lineHeight: '1.4', fontSize: 11 }}
      >
        <div style={{ marginBottom: 2 }}>
          <span style={{ color: 'var(--sk-text-muted)' }}>In:</span> {formatNumber(stats.received)}{' '}
          pkt ({receivedPps} pps)
          <span style={{ marginLeft: 8, color: 'var(--sk-text-muted)' }}>Out:</span>{' '}
          {formatNumber(stats.sent)} pkt ({sentPps} pps)
        </div>
        {(stats.discarded > 0 || stats.errored > 0) && (
          <div style={{ marginTop: 2, color: 'var(--sk-warning)' }}>
            {stats.discarded > 0 &&
              `Discarded: ${formatNumber(stats.discarded)} pkt (${discardedPps} pps)`}
            {stats.discarded > 0 && stats.errored > 0 && ' | '}
            {stats.errored > 0 && (
              <span style={{ color: 'var(--sk-danger)' }}>
                Errors: {formatNumber(stats.errored)} pkt ({erroredPps} pps)
              </span>
            )}
          </div>
        )}
      </div>
    </div>
  );
};

const renderStringStateDetails = (
  state: Extract<NodeState, string>,
  stats?: NodeStats
): React.ReactNode => {
  return (
    <div style={{ fontSize: 12 }}>
      <div style={{ fontWeight: 600, marginBottom: 4 }}>{state}</div>
      <div style={{ color: 'var(--sk-text-muted)' }}>{getStateDescription(state)}</div>
      {renderPacketStats(stats)}
    </div>
  );
};

const renderRecoveringDetails = (
  state: Extract<NodeState, { Recovering: { reason: string; details: unknown } }>,
  stats?: NodeStats
): React.ReactNode => {
  const details = state.Recovering.details;
  const hasDetails = details !== null && details !== undefined && typeof details === 'object';

  return (
    <div style={{ fontSize: 12 }}>
      <div style={{ fontWeight: 600, marginBottom: 4 }}>Recovering</div>
      <div style={{ color: 'var(--sk-text-muted)', marginBottom: 4 }}>
        {getStateDescription(state)}
      </div>
      <div style={{ color: 'var(--sk-text)' }}>{state.Recovering.reason}</div>
      {hasDetails && (
        <pre
          style={{
            marginTop: 4,
            fontSize: 10,
            background: 'var(--sk-bg)',
            padding: 4,
            borderRadius: 4,
            maxWidth: 200,
            overflow: 'auto',
          }}
        >
          {JSON.stringify(details, null, 2)}
        </pre>
      )}
      {renderPacketStats(stats)}
    </div>
  );
};

const renderDegradedDetails = (
  state: Extract<NodeState, { Degraded: { reason: string; details: unknown } }>,
  stats?: NodeStats,
  context?: { pipeline?: Pipeline | null; nodeId?: string }
): React.ReactNode => {
  const { slowPins, newlySlowPins } = getDegradedPins(state.Degraded.details);
  const slowSources = getSlowSourcesForDegraded(context, slowPins);
  const slowSummary = renderSlowPinsSummary(slowPins, newlySlowPins, slowSources);

  return (
    <div style={{ fontSize: 12 }}>
      <div style={{ fontWeight: 600, marginBottom: 4 }}>Degraded</div>
      <div style={{ color: 'var(--sk-text-muted)', marginBottom: 4 }}>
        {getStateDescription(state)}
      </div>
      <div style={{ color: 'var(--sk-text)' }}>{state.Degraded.reason}</div>
      {slowSummary}
      {renderPacketStats(stats)}
    </div>
  );
};

const renderFailedDetails = (
  state: Extract<NodeState, { Failed: { reason: string } }>,
  stats?: NodeStats
): React.ReactNode => {
  return (
    <div style={{ fontSize: 12 }}>
      <div style={{ fontWeight: 600, marginBottom: 4 }}>Failed</div>
      <div style={{ color: 'var(--sk-text-muted)', marginBottom: 4 }}>
        {getStateDescription(state)}
      </div>
      <div style={{ color: 'var(--sk-danger)' }}>{state.Failed.reason}</div>
      {renderPacketStats(stats)}
    </div>
  );
};

const renderStoppedDetails = (
  state: Extract<NodeState, { Stopped: { reason: unknown } }>,
  stats?: NodeStats
): React.ReactNode => {
  return (
    <div style={{ fontSize: 12 }}>
      <div style={{ fontWeight: 600, marginBottom: 4 }}>Stopped</div>
      <div style={{ color: 'var(--sk-text-muted)', marginBottom: 4 }}>
        {getStateDescription(state)}
      </div>
      <div style={{ color: 'var(--sk-text)' }}>{String(state.Stopped.reason)}</div>
      {renderPacketStats(stats)}
    </div>
  );
};

function getStateDetails(state: NodeState, stats?: NodeStats): React.ReactNode {
  if (typeof state === 'string') return renderStringStateDetails(state, stats);
  if ('Recovering' in state) return renderRecoveringDetails(state, stats);
  if ('Degraded' in state) return renderDegradedDetails(state, stats);
  if ('Failed' in state) return renderFailedDetails(state, stats);
  if ('Stopped' in state) return renderStoppedDetails(state, stats);
  return null;
}

const NodeStateTooltipContent = React.memo(
  ({ state, stats }: { state: NodeState; stats?: NodeStats }) => getStateDetails(state, stats)
);

const LiveNodeStateTooltipContent = React.memo(
  ({
    state,
    nodeId,
    sessionId,
    fallbackStats,
  }: {
    state: NodeState;
    nodeId: string;
    sessionId: string;
    fallbackStats?: NodeStats;
  }) => {
    const liveStats = useSessionStore(
      React.useCallback((s) => s.sessions.get(sessionId)?.nodeStats[nodeId], [nodeId, sessionId])
    );
    const pipeline = useSessionStore(
      React.useCallback((s) => s.sessions.get(sessionId)?.pipeline, [sessionId])
    );

    if (typeof state === 'object' && 'Degraded' in state) {
      return renderDegradedDetails(state, liveStats ?? fallbackStats, { pipeline, nodeId });
    }

    return getStateDetails(state, liveStats ?? fallbackStats);
  }
);

export const NodeStateIndicator: React.FC<NodeStateIndicatorProps> = ({
  state,
  stats: propStats,
  showLabel = false,
  nodeId,
  sessionId,
}) => {
  // Get live stats for error badge display
  const liveStats = useSessionStore(
    React.useCallback(
      (s) => (nodeId && sessionId ? s.sessions.get(sessionId)?.nodeStats[nodeId] : undefined),
      [nodeId, sessionId]
    )
  );
  const stats = liveStats ?? propStats;
  const hasErrors = stats && stats.errored > 0;

  const color = getStateColor(state);
  const label = getStateLabel(state);

  const content =
    nodeId && sessionId ? (
      <LiveNodeStateTooltipContent
        state={state}
        nodeId={nodeId}
        sessionId={sessionId}
        fallbackStats={propStats}
      />
    ) : (
      <NodeStateTooltipContent state={state} stats={propStats} />
    );

  return (
    <SKTooltip content={content} side="top">
      <div
        className="nodrag"
        style={{ display: 'flex', alignItems: 'center', gap: 6, cursor: 'help' }}
      >
        <IndicatorWrapper>
          <StateIndicator color={color} />
          {hasErrors && <ErrorBadge />}
        </IndicatorWrapper>
        {showLabel && <span style={{ color: 'var(--sk-text-muted)', fontSize: 11 }}>{label}</span>}
      </div>
    </SKTooltip>
  );
};

export default NodeStateIndicator;
