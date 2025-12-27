// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React from 'react';

import {
  useStagingStore,
  type StagingData,
  type StagedChange,
  type ValidationError,
} from '@/stores/stagingStore';

const Banner = styled.div`
  position: absolute;
  top: 70px;
  left: 0;
  right: 0;
  z-index: 100;
  background: linear-gradient(135deg, var(--sk-warning) 0%, var(--sk-warning-hover) 100%);
  color: var(--sk-text);
  padding: 10px 20px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
  border-bottom: 2px solid var(--sk-border-strong);
`;

const LeftSection = styled.div`
  display: flex;
  align-items: center;
  gap: 16px;
`;

const RightSection = styled.div`
  display: flex;
  align-items: center;
  gap: 12px;
`;

const StatusBadge = styled.div`
  display: flex;
  align-items: center;
  gap: 8px;
  font-weight: 600;
  font-size: 14px;
`;

const StatusIcon = styled.div`
  width: 12px;
  height: 12px;
  border-radius: 50%;
  background-color: var(--sk-text);
  animation: pulse 2s ease-in-out infinite;

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

const ChangesSummary = styled.div`
  display: flex;
  align-items: center;
  gap: 12px;
  font-size: 13px;
  color: var(--sk-text-muted);
`;

const ChangeBadge = styled.span`
  padding: 2px 8px;
  border-radius: 4px;
  background: rgba(0, 0, 0, 0.2);
  font-weight: 500;
  font-size: 12px;
`;

const Button = styled.button<{ variant?: 'primary' | 'secondary' | 'danger' }>`
  padding: 6px 16px;
  border-radius: 6px;
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.2s;
  border: 1px solid transparent;

  ${(props) => {
    switch (props.variant) {
      case 'primary':
        return `
          background: var(--sk-primary);
          color: var(--sk-text-strong);
          border-color: var(--sk-primary);

          &:hover {
            background: var(--sk-primary-hover);
            border-color: var(--sk-primary-hover);
          }

          &:disabled {
            background: var(--sk-bg-strong);
            color: var(--sk-text-muted);
            border-color: var(--sk-border);
            cursor: not-allowed;
          }
        `;
      case 'danger':
        return `
          background: var(--sk-danger);
          color: var(--sk-text-strong);
          border-color: var(--sk-danger);

          &:hover {
            background: var(--sk-danger-hover);
            border-color: var(--sk-danger-hover);
          }
        `;
      default:
        return `
          background: rgba(255, 255, 255, 0.15);
          color: var(--sk-text);
          border-color: rgba(255, 255, 255, 0.25);

          &:hover {
            background: rgba(255, 255, 255, 0.25);
            border-color: rgba(255, 255, 255, 0.35);
          }
        `;
    }
  }}
`;

const ErrorList = styled.div`
  max-width: 400px;
  font-size: 12px;
  color: var(--sk-danger);
  display: flex;
  flex-direction: column;
  gap: 4px;
`;

interface StagingModeIndicatorProps {
  sessionId: string;
  onCommit: () => void;
  onDiscard: () => void;
}

// Helper: Compute changes summary
interface ChangesSummary {
  added: number;
  removed: number;
  modified: number;
  hasChanges: boolean;
  hasErrors: boolean;
  hasWarnings: boolean;
}

function computeChangesSummary(stagingData: StagingData): ChangesSummary {
  const added = stagingData.changes.filter(
    (c: StagedChange) => c.type === 'add_node' || c.type === 'add_connection'
  ).length;
  const removed = stagingData.changes.filter(
    (c: StagedChange) => c.type === 'remove_node' || c.type === 'remove_connection'
  ).length;
  const modified = stagingData.changes.filter(
    (c: StagedChange) => c.type === 'update_params'
  ).length;

  const hasChanges = added > 0 || removed > 0 || modified > 0;
  const hasErrors =
    stagingData.validationErrors.filter((e: ValidationError) => e.type === 'error').length > 0;
  const hasWarnings =
    stagingData.validationErrors.filter((e: ValidationError) => e.type === 'warning').length > 0;

  return { added, removed, modified, hasChanges, hasErrors, hasWarnings };
}

// Helper: Get commit button text
function getCommitButtonText(hasErrors: boolean, hasWarnings: boolean): string {
  if (hasErrors) return 'Fix Errors to Commit';
  if (hasWarnings) return 'Commit (with warnings)';
  return 'Commit Changes';
}

export const StagingModeIndicator: React.FC<StagingModeIndicatorProps> = ({
  sessionId,
  onCommit,
  onDiscard,
}) => {
  const stagingData = useStagingStore((state) => state.staging[sessionId]);

  if (!stagingData || stagingData.mode !== 'staging') {
    return null;
  }

  const { added, removed, modified, hasChanges, hasErrors, hasWarnings } =
    computeChangesSummary(stagingData);

  return (
    <Banner>
      <LeftSection>
        <StatusBadge>
          <StatusIcon />
          Staging Mode
        </StatusBadge>
        {hasChanges && (
          <ChangesSummary>
            {added > 0 && <ChangeBadge>+{added} added</ChangeBadge>}
            {removed > 0 && <ChangeBadge>-{removed} removed</ChangeBadge>}
            {modified > 0 && <ChangeBadge>~{modified} modified</ChangeBadge>}
          </ChangesSummary>
        )}
        {!hasChanges && (
          <span style={{ color: 'var(--sk-text-muted)', fontSize: 13 }}>No changes yet</span>
        )}
        {hasErrors && (
          <ErrorList>
            {stagingData.validationErrors
              .filter((e) => e.type === 'error')
              .slice(0, 3)
              .map((error, i) => (
                <div key={i}>⚠ {error.message}</div>
              ))}
            {stagingData.validationErrors.filter((e) => e.type === 'error').length > 3 && (
              <div>
                ... and {stagingData.validationErrors.filter((e) => e.type === 'error').length - 3}{' '}
                more
              </div>
            )}
          </ErrorList>
        )}
      </LeftSection>

      <RightSection>
        <Button variant="secondary" onClick={onDiscard}>
          Discard Changes
        </Button>
        <Button variant="primary" onClick={onCommit} disabled={!hasChanges || hasErrors}>
          {getCommitButtonText(hasErrors, hasWarnings)}
        </Button>
      </RightSection>
    </Banner>
  );
};
