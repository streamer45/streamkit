// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Session list item and session info chip components for the Monitor View.
 *
 * Includes:
 * - SessionItem — sidebar list entry with status badge, tooltip, and delete button
 * - SessionInfoChip — expandable chip shown on the canvas top bar
 * - SessionUptime — self-updating uptime counter
 * - InlineCopyButton — small copy-to-clipboard button
 */

import * as Tooltip from '@radix-ui/react-tooltip';
import React, { useState, useEffect, useCallback, useRef } from 'react';

import {
  SessionItemWrapper,
  SessionButton,
  SessionStatusBadge,
  SessionButtonText,
  SessionDeleteButton,
  SessionTooltipContent,
  TooltipRow,
  TooltipLabel,
  TooltipValue,
  SessionChipContainer,
  SessionChipButton,
  SessionChipName,
  SessionChipMeta,
  SessionChipCaret,
  SessionStatusDot,
  SessionDetailsPanel,
  DetailsRow,
  DetailsLabel,
  DetailsValue,
} from '@/components/monitor/MonitorView.styles';
import { SKTooltip } from '@/components/Tooltip';
import { Button } from '@/components/ui/Button';
import { useSessionNodeStates } from '@/hooks/useSessionNodeStates';
import { shortSessionId, summarizeNodeIssues } from '@/utils/nodeIssues';
import {
  computeSessionStatus,
  getSessionStatusColor,
  getSessionStatusLabel,
} from '@/utils/sessionStatus';
import { formatUptime, formatDateTime } from '@/utils/time';

// Shared types

export interface SessionItemProps {
  session: { id: string; name: string | null; created_at: string };
  isActive: boolean;
  onClick: (id: string) => void;
  onDelete: (id: string) => void;
}

export interface SessionInfoDisplayProps {
  session: { id: string; name: string | null; created_at: string };
}

// SessionUptime — isolated 1 s re-render

export const SessionUptime: React.FC<{ createdAt: string }> = React.memo(({ createdAt }) => {
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

// InlineCopyButton

export const InlineCopyButton: React.FC<{
  text: string;
  tooltip?: string;
  ariaLabel?: string;
}> = React.memo(({ text, tooltip = 'Copy to clipboard', ariaLabel = 'Copy to clipboard' }) => {
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

// SessionInfoChip — expandable chip on canvas top bar

export const SessionInfoChip: React.FC<SessionInfoDisplayProps> = React.memo(({ session }) => {
  const nodeStates = useSessionNodeStates(session.id);

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

// SessionItem — sidebar list entry

export const SessionItem: React.FC<SessionItemProps> = React.memo(
  ({ session, isActive, onClick, onDelete }) => {
    const nodeStates = useSessionNodeStates(session.id);

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
