// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Top-right control bar for the Monitor View canvas.
 *
 * Shows connection status, preview button, staging-mode controls
 * (commit / discard / enter staging), and session delete.
 */

import React from 'react';

import { TopRightControls, ButtonGroup } from '@/components/monitor/MonitorView.styles';
import { SKTooltip } from '@/components/Tooltip';
import { Button } from '@/components/ui/Button';
import { LiveBadge, LiveDot } from '@/components/ui/LiveIndicator';
import type { StagingData, StagedChange, ValidationError } from '@/stores/stagingStore';

import { ConnectionStatus } from './ConnectionStatus';

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

export interface TopControlsProps {
  isConnected: boolean;
  selectedSessionId: string | null;
  isInStagingMode: boolean;
  stagingData: StagingData | undefined;
  onCommit: () => void;
  onDiscard: () => void;
  onEnterStaging: () => void;
  onDelete: () => void;
  onStartPreview: () => void;
  isPreviewConnected: boolean;
}

// ---------------------------------------------------------------------------
// Custom memo comparison
// ---------------------------------------------------------------------------

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
  if (prevProps.onStartPreview !== nextProps.onStartPreview) return false;
  if (prevProps.isPreviewConnected !== nextProps.isPreviewConnected) return false;

  // Compare changes array length and validation errors
  const prevChanges = prevProps.stagingData?.changes ?? [];
  const nextChanges = nextProps.stagingData?.changes ?? [];
  const prevErrors = prevProps.stagingData?.validationErrors ?? [];
  const nextErrors = nextProps.stagingData?.validationErrors ?? [];

  if (prevChanges.length !== nextChanges.length) return false;
  if (prevErrors.length !== nextErrors.length) return false;

  // Compare the number of blocking errors, not just total length.
  // The commit button is disabled when any error-type validation entry exists,
  // so a warning→error swap at the same length must trigger a re-render.
  const countErrors = (arr: readonly ValidationError[]) =>
    arr.filter((e) => e.type === 'error').length;
  if (countErrors(prevErrors) !== countErrors(nextErrors)) return false;

  // If lengths are same and other props haven't changed, don't re-render
  return true;
};

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

// Memoized TopControls component to prevent re-renders during drag
export const TopControls = React.memo(
  ({
    isConnected,
    selectedSessionId,
    isInStagingMode,
    stagingData,
    onCommit,
    onDiscard,
    onEnterStaging,
    onDelete,
    onStartPreview,
    isPreviewConnected,
  }: TopControlsProps) => {
    // Only extract the fields we need to minimize re-renders
    const changes = stagingData?.changes ?? [];
    const validationErrors = stagingData?.validationErrors ?? [];

    return (
      <TopRightControls>
        <ButtonGroup>
          <ConnectionStatus connected={isConnected} />
          {selectedSessionId && !isPreviewConnected && (
            <SKTooltip content="Connect to MoQ gateway and start watching the output preview">
              <Button variant="ghost" size="small" onClick={onStartPreview}>
                Preview
              </Button>
            </SKTooltip>
          )}
        </ButtonGroup>
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
