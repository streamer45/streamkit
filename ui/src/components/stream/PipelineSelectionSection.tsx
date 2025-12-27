// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React from 'react';

import { PipelineEditor } from '@/components/converter/PipelineEditor';
import { TemplateSelector } from '@/components/converter/TemplateSelector';
import type { SessionCreationStatus } from '@/hooks/useStreamViewState';
import type { NodeDefinition, SamplePipeline } from '@/types/generated/api-types';

const Section = styled.div`
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 24px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 12px;
`;

const SectionTitle = styled.h2`
  font-size: 18px;
  font-weight: 600;
  color: var(--sk-text);
  margin: 0;
`;

const InputGroup = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const Label = styled.label`
  font-size: 14px;
  font-weight: 600;
  color: var(--sk-text);
`;

const Input = styled.input`
  padding: 12px;
  font-size: 14px;
  background: var(--sk-bg);
  color: var(--sk-text);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  font-family: inherit;

  &:focus {
    outline: none;
    border-color: var(--sk-primary);
  }

  &::placeholder {
    color: var(--sk-text-muted);
  }
`;

const Button = styled.button<{ variant?: 'primary' | 'secondary'; disabled?: boolean }>`
  padding: 8px 16px;
  font-size: 14px;
  font-weight: 600;
  color: ${(props) => {
    if (props.disabled) return 'var(--sk-text-muted)';
    return props.variant === 'primary' ? 'var(--sk-primary-contrast)' : 'var(--sk-text)';
  }};
  background: ${(props) => {
    if (props.disabled) return 'var(--sk-hover-bg)';
    return props.variant === 'primary' ? 'var(--sk-primary)' : 'var(--sk-panel-bg)';
  }};
  border: 1px solid
    ${(props) => {
      if (props.disabled) return 'var(--sk-border)';
      return props.variant === 'primary' ? 'var(--sk-primary)' : 'var(--sk-border)';
    }};
  border-radius: 6px;
  cursor: ${(props) => (props.disabled ? 'not-allowed' : 'pointer')};
  transition: none;

  &:hover:not(:disabled) {
    background: ${(props) =>
      props.variant === 'primary' ? 'var(--sk-primary-hover)' : 'var(--sk-hover-bg)'};
    border-color: ${(props) =>
      props.variant === 'primary' ? 'var(--sk-primary-hover)' : 'var(--sk-border-strong)'};
  }
`;

const ErrorMessage = styled.div`
  padding: 12px 16px;
  background: rgba(244, 67, 54, 0.1);
  border: 1px solid rgba(244, 67, 54, 0.3);
  border-radius: 6px;
  color: #f44336;
  font-size: 14px;
`;

const LoadingMessage = styled.div`
  padding: 12px 16px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  color: var(--sk-text-muted);
  font-size: 14px;
`;

const ActiveSessionBadge = styled.div`
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: rgba(76, 175, 80, 0.1);
  border: 1px solid rgba(76, 175, 80, 0.3);
  border-radius: 6px;
  color: #4caf50;
  font-size: 14px;
  font-weight: 600;

  &::before {
    content: '';
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #4caf50;
  }
`;

const LiveBadge = styled.span`
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  background: rgba(239, 68, 68, 0.15);
  color: rgb(239, 68, 68);
  border: 1px solid rgba(239, 68, 68, 0.3);
  border-radius: 999px;
  font-size: 11px;
  font-weight: 800;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  user-select: none;

  &::before {
    content: '';
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: rgb(239, 68, 68);
  }
`;

const SessionButtons = styled.div`
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
`;

const SessionInfo = styled.div`
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 13px;
  color: var(--sk-text-muted);
`;

interface ActiveSessionPanelProps {
  activeSessionId: string;
  activeSessionName: string | null;
  activePipelineName: string | null;
  streamStatus: 'disconnected' | 'connecting' | 'connected';
  onDisconnect?: () => void;
  onDestroySession?: () => void;
  onViewInMonitor?: () => void;
}

const ActiveSessionPanel: React.FC<ActiveSessionPanelProps> = ({
  activeSessionId,
  activeSessionName,
  activePipelineName,
  streamStatus,
  onDisconnect,
  onDestroySession,
  onViewInMonitor,
}) => {
  const disconnectLabel = streamStatus === 'connecting' ? 'Cancel Connect' : 'Disconnect';
  const disconnectDisabled = streamStatus === 'disconnected';

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '12px' }}>
      <ActiveSessionBadge>Session Active</ActiveSessionBadge>
      <SessionInfo>
        <div>
          <strong>Session:</strong> {activeSessionName}
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
          <div>
            <strong>Pipeline:</strong> {activePipelineName}
          </div>
          {streamStatus === 'connected' && <LiveBadge>LIVE</LiveBadge>}
        </div>
        <div style={{ fontSize: '12px', marginTop: '4px' }}>Session ID: {activeSessionId}</div>
        <div style={{ fontSize: 12, marginTop: 4 }}>
          Tip: Destroy the session to create a new pipeline.
        </div>
      </SessionInfo>
      <SessionButtons>
        {onDisconnect && (
          <Button variant="secondary" onClick={onDisconnect} disabled={disconnectDisabled}>
            {disconnectLabel}
          </Button>
        )}
        {onDestroySession && (
          <Button variant="secondary" onClick={onDestroySession}>
            Destroy Session
          </Button>
        )}
        {onViewInMonitor && (
          <Button variant="secondary" onClick={onViewInMonitor}>
            View in Monitor →
          </Button>
        )}
      </SessionButtons>
    </div>
  );
};

interface PipelineSelectionSectionProps {
  samples: SamplePipeline[];
  samplesLoading: boolean;
  samplesError: string | null;
  selectedTemplateId: string;
  pipelineYaml: string;
  sessionName: string;
  sessionCreationStatus: SessionCreationStatus;
  sessionCreationError: string | null;
  activeSessionId: string | null;
  activeSessionName: string | null;
  activePipelineName: string | null;
  streamStatus?: 'disconnected' | 'connecting' | 'connected';
  onTemplateSelect: (templateId: string) => void;
  onPipelineYamlChange: (yaml: string) => void;
  onSessionNameChange: (name: string) => void;
  onCreateSession: () => void;
  onDisconnect?: () => void;
  onDestroySession?: () => void;
  onViewInMonitor?: () => void;
  nodeDefinitions?: NodeDefinition[];
}

export const PipelineSelectionSection: React.FC<PipelineSelectionSectionProps> = ({
  samples,
  samplesLoading,
  samplesError,
  selectedTemplateId,
  pipelineYaml,
  sessionName,
  sessionCreationStatus,
  sessionCreationError,
  activeSessionId,
  activeSessionName,
  activePipelineName,
  streamStatus = 'disconnected',
  onTemplateSelect,
  onPipelineYamlChange,
  onSessionNameChange,
  onCreateSession,
  onDisconnect,
  onDestroySession,
  onViewInMonitor,
  nodeDefinitions,
}) => {
  const showEmptyState = !samplesLoading && samples.length === 0;
  const showTemplates = !samplesLoading && samples.length > 0;

  return (
    <Section>
      <SectionTitle>Pipeline Selection</SectionTitle>

      {samplesLoading && <LoadingMessage>Loading pipeline templates...</LoadingMessage>}

      {samplesError && <ErrorMessage>{samplesError}</ErrorMessage>}

      {showEmptyState && <LoadingMessage>No dynamic pipeline templates available</LoadingMessage>}

      {showTemplates && (
        <>
          <TemplateSelector
            templates={samples}
            selectedTemplateId={selectedTemplateId}
            onTemplateSelect={onTemplateSelect}
          />

          <PipelineEditor
            value={pipelineYaml}
            onChange={onPipelineYamlChange}
            nodeDefinitions={nodeDefinitions}
          />

          <InputGroup>
            <Label htmlFor="session-name">Session Name (Optional)</Label>
            <Input
              id="session-name"
              type="text"
              value={sessionName}
              onChange={(e) => onSessionNameChange(e.target.value)}
              placeholder="My MoQ Stream"
              disabled={sessionCreationStatus === 'creating' || activeSessionId !== null}
            />
          </InputGroup>

          {sessionCreationError && <ErrorMessage>{sessionCreationError}</ErrorMessage>}

          {activeSessionId ? (
            <ActiveSessionPanel
              activeSessionId={activeSessionId}
              activeSessionName={activeSessionName}
              activePipelineName={activePipelineName}
              streamStatus={streamStatus}
              onDisconnect={onDisconnect}
              onDestroySession={onDestroySession}
              onViewInMonitor={onViewInMonitor}
            />
          ) : (
            <Button
              variant="primary"
              onClick={onCreateSession}
              disabled={sessionCreationStatus === 'creating' || !pipelineYaml}
            >
              {sessionCreationStatus === 'creating' ? 'Creating Session...' : 'Create Session'}
            </Button>
          )}
        </>
      )}
    </Section>
  );
};
