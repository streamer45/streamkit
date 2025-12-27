// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { useEffect, useState, useImperativeHandle, forwardRef, useCallback, memo } from 'react';
import { useNavigate } from 'react-router-dom';

import ConfirmModal from '@/components/ConfirmModal';
import { SKTooltip } from '@/components/Tooltip';
import { useToast } from '@/context/ToastContext';
import {
  listFragments,
  deleteFragment as deleteFragmentService,
  yamlToFragment,
} from '@/services/fragments';
import { listSamples, deleteSample } from '@/services/samples';
import { createSession } from '@/services/sessions';
import type { SamplePipeline } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

const logger = getLogger('SamplePipelinesPane');

const PaneWrapper = styled.div`
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--sk-sidebar-bg);
  color: var(--sk-text);
  overflow: hidden;
`;

const PaneHeader = styled.div`
  padding: 12px;
  border-bottom: 1px solid var(--sk-border);
  flex-shrink: 0;
`;

const PaneTitle = styled.h3`
  margin: 0 0 4px 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--sk-text);
`;

const PaneSubtitle = styled.p`
  margin: 0;
  font-size: 12px;
  color: var(--sk-text-muted);
`;

const SamplesList = styled.div`
  flex: 1;
  overflow-y: auto;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const SampleCard = styled.div`
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 12px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  text-align: left;
  color: var(--sk-text);
  position: relative;
  transition: none;

  &:hover {
    background: var(--sk-hover-bg);
    border-color: var(--sk-border-strong);
  }

  &:hover .action-button {
    opacity: 1;
  }
`;

const SampleCardButton = styled.button`
  all: unset;
  display: flex;
  flex-direction: column;
  gap: 6px;
  cursor: pointer;
  flex: 1;

  &:active {
    opacity: 0.8;
  }
`;

const ActionButton = styled.button<{ variant?: 'danger' | 'primary' }>`
  position: absolute;
  top: 8px;
  padding: 4px 8px;
  background: ${(props) => (props.variant === 'danger' ? 'var(--sk-danger)' : 'var(--sk-primary)')};
  color: white;
  border: none;
  border-radius: 4px;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.2s;

  &:hover {
    opacity: 1 !important;
    background: ${(props) =>
      props.variant === 'danger' ? 'var(--sk-danger-hover)' : 'var(--sk-primary-hover)'};
  }

  &:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
`;

const DeleteButton = styled(ActionButton)`
  right: 8px;
`;

const CreateSessionButton = styled(ActionButton)<{ hasDeleteButton?: boolean }>`
  right: ${(props) => (props.hasDeleteButton ? '90px' : '8px')};
`;

const SampleName = styled.div`
  font-weight: 600;
  font-size: 13px;
  color: var(--sk-text);
  display: flex;
  align-items: center;
  gap: 6px;
`;

const SystemBadge = styled.span`
  background: var(--sk-primary);
  color: var(--sk-text-white);
  font-size: 9px;
  font-weight: 700;
  padding: 2px 6px;
  border-radius: 999px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
`;

const SampleDescription = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
  line-height: 1.4;
`;

const LoadingState = styled.div`
  padding: 12px;
  text-align: center;
  color: var(--sk-text-muted);
  font-size: 12px;
`;

const ErrorState = styled.div`
  padding: 12px;
  color: var(--sk-danger);
  font-size: 12px;
`;

const EmptyState = styled.div`
  padding: 12px;
  text-align: center;
  color: var(--sk-text-muted);
  font-size: 12px;
`;

const SectionHeader = styled.div`
  padding: 8px 4px 4px;
  font-size: 11px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--sk-text-muted);
`;

const FragmentCard = styled.div`
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 12px;
  background: var(--sk-panel-bg);
  border: 2px dashed var(--sk-border);
  border-radius: 8px;
  cursor: grab;
  color: var(--sk-text);
  position: relative;
  transition: none;

  &:active {
    cursor: grabbing;
  }

  &:hover {
    border-color: var(--sk-border-strong);
    background: var(--sk-hover-bg);
  }

  &:hover .action-button {
    opacity: 1;
  }
`;

const FragmentName = styled.div`
  font-weight: 600;
  font-size: 13px;
  color: var(--sk-accent);
`;

const FragmentDescription = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
  line-height: 1.4;
`;

const FragmentMeta = styled.div`
  display: flex;
  gap: 8px;
  font-size: 10px;
  color: var(--sk-text-muted);
`;

const TagsContainer = styled.div`
  display: flex;
  gap: 4px;
  flex-wrap: wrap;
  margin-top: 2px;
`;

const Tag = styled.span`
  background: var(--sk-accent-alpha);
  color: var(--sk-accent);
  font-size: 9px;
  font-weight: 600;
  padding: 2px 6px;
  border-radius: 4px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
`;

export interface FragmentSample extends SamplePipeline {
  tags: string[];
}

interface SamplePipelinesPaneProps {
  onLoadSample: (yaml: string, name: string, description: string) => void;
  mode?: 'oneshot' | 'dynamic';
  onFragmentDragStart?: (event: React.DragEvent, fragment: FragmentSample) => void;
  onFragmentInsert?: (fragment: FragmentSample) => void;
}

export interface SamplePipelinesPaneRef {
  refresh: () => void;
}

const SamplePipelinesPane = forwardRef<SamplePipelinesPaneRef, SamplePipelinesPaneProps>(
  ({ onLoadSample, mode, onFragmentDragStart, onFragmentInsert }, ref) => {
    const [samples, setSamples] = useState<SamplePipeline[]>([]);
    const [fragments, setFragments] = useState<FragmentSample[]>([]);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const [showDeleteModal, setShowDeleteModal] = useState(false);
    const [sampleToDelete, setSampleToDelete] = useState<SamplePipeline | null>(null);
    const [showFragmentDeleteModal, setShowFragmentDeleteModal] = useState(false);
    const [fragmentToDelete, setFragmentToDelete] = useState<FragmentSample | null>(null);
    const [isDeleting, setIsDeleting] = useState(false);
    const [creatingSessionId, setCreatingSessionId] = useState<string | null>(null);
    const toast = useToast();
    const navigate = useNavigate();

    const loadSamples = useCallback(async () => {
      try {
        setLoading(true);
        setError(null);
        const [samplesData, fragmentsData] = await Promise.all([listSamples(), listFragments()]);
        setSamples(samplesData);
        setFragments(fragmentsData);
      } catch (err) {
        logger.error('Failed to load sample pipelines:', err);
        setError(err instanceof Error ? err.message : 'Failed to load samples');
        toast.error('Failed to load sample pipelines');
      } finally {
        setLoading(false);
      }
    }, [toast]);

    useEffect(() => {
      loadSamples();
    }, [loadSamples]);

    // Expose refresh method to parent
    useImperativeHandle(ref, () => ({
      refresh: loadSamples,
    }));

    const handleLoadSample = (sample: SamplePipeline) => {
      onLoadSample(sample.yaml, sample.name, sample.description);
      toast.success(`Loaded pipeline: ${sample.name}`);
    };

    const handleDeleteClick = (sample: SamplePipeline, e: React.MouseEvent) => {
      e.stopPropagation();
      setSampleToDelete(sample);
      setShowDeleteModal(true);
    };

    const handleConfirmDelete = async () => {
      if (!sampleToDelete) return;

      try {
        setIsDeleting(true);
        await deleteSample(sampleToDelete.id);
        toast.success(`Deleted pipeline: ${sampleToDelete.name}`);

        // Refresh the samples list
        await loadSamples();

        setShowDeleteModal(false);
        setSampleToDelete(null);
      } catch (err) {
        logger.error('Failed to delete sample:', err);
        toast.error(err instanceof Error ? err.message : 'Failed to delete sample');
      } finally {
        setIsDeleting(false);
      }
    };

    const handleCancelDelete = () => {
      setShowDeleteModal(false);
      setSampleToDelete(null);
    };

    // Fragment handlers
    const handleFragmentDragStart = (event: React.DragEvent, fragment: FragmentSample) => {
      if (onFragmentDragStart) {
        onFragmentDragStart(event, fragment);
      }
    };

    const handleFragmentClick = (fragment: FragmentSample) => {
      if (onFragmentInsert) {
        onFragmentInsert(fragment);
      }
    };

    const handleFragmentDeleteClick = (fragment: FragmentSample, e: React.MouseEvent) => {
      e.stopPropagation();
      setFragmentToDelete(fragment);
      setShowFragmentDeleteModal(true);
    };

    const handleConfirmFragmentDelete = async () => {
      if (!fragmentToDelete) return;

      try {
        setIsDeleting(true);
        await deleteFragmentService(fragmentToDelete.id);
        toast.success(`Deleted fragment: ${fragmentToDelete.name}`);

        // Refresh the list
        await loadSamples();

        setShowFragmentDeleteModal(false);
        setFragmentToDelete(null);
      } catch (err) {
        logger.error('Failed to delete fragment:', err);
        toast.error(err instanceof Error ? err.message : 'Failed to delete fragment');
      } finally {
        setIsDeleting(false);
      }
    };

    const handleCancelFragmentDelete = () => {
      setShowFragmentDeleteModal(false);
      setFragmentToDelete(null);
    };

    const handleCreateSession = async (sample: SamplePipeline, e: React.MouseEvent) => {
      e.stopPropagation();

      try {
        setCreatingSessionId(sample.id);
        const result = await createSession(sample.name, sample.yaml);
        const sessionDisplayName = result.name || result.session_id;

        toast.success(`Session created: ${sessionDisplayName}`);

        // Navigate to monitor view
        navigate('/monitor');
      } catch (err) {
        logger.error('Failed to create session:', err);
        toast.error(err instanceof Error ? err.message : 'Failed to create session');
      } finally {
        setCreatingSessionId(null);
      }
    };

    // Filter samples by mode if specified
    const filteredSamples = mode ? samples.filter((s) => s.mode === mode) : samples;

    const systemSamples = filteredSamples.filter((s) => s.is_system);
    const userSamples = filteredSamples.filter((s) => !s.is_system);

    return (
      <PaneWrapper>
        <PaneHeader>
          <PaneTitle>Samples & Fragments</PaneTitle>
          <PaneSubtitle>Click to load samples or drag fragments to canvas</PaneSubtitle>
        </PaneHeader>

        {loading && <LoadingState>Loading samples...</LoadingState>}

        {error && <ErrorState>{error}</ErrorState>}

        {!loading && !error && samples.length === 0 && (
          <EmptyState>No sample pipelines available</EmptyState>
        )}

        {!loading && !error && samples.length > 0 && (
          <SamplesList>
            {systemSamples.length > 0 && (
              <>
                <SectionHeader>System Samples</SectionHeader>
                {systemSamples.map((sample) => (
                  <SampleCard key={sample.id}>
                    <SampleCardButton onClick={() => handleLoadSample(sample)}>
                      <SampleName>
                        {sample.name}
                        <SystemBadge>System</SystemBadge>
                      </SampleName>
                      {sample.description && (
                        <SampleDescription>{sample.description}</SampleDescription>
                      )}
                    </SampleCardButton>
                    {sample.mode === 'dynamic' && (
                      <SKTooltip content="Create session from this pipeline" side="top">
                        <CreateSessionButton
                          className="action-button"
                          variant="primary"
                          onClick={(e) => handleCreateSession(sample, e)}
                          disabled={creatingSessionId === sample.id}
                          hasDeleteButton={false}
                        >
                          {creatingSessionId === sample.id ? '⏳' : '▶️'}{' '}
                          {creatingSessionId === sample.id ? 'Creating...' : 'Create Session'}
                        </CreateSessionButton>
                      </SKTooltip>
                    )}
                  </SampleCard>
                ))}
              </>
            )}

            {userSamples.length > 0 && (
              <>
                <SectionHeader>User Samples</SectionHeader>
                {userSamples.map((sample) => (
                  <SampleCard key={sample.id}>
                    <SampleCardButton onClick={() => handleLoadSample(sample)}>
                      <SampleName>{sample.name}</SampleName>
                      {sample.description && (
                        <SampleDescription>{sample.description}</SampleDescription>
                      )}
                    </SampleCardButton>
                    {sample.mode === 'dynamic' && (
                      <SKTooltip content="Create session from this pipeline" side="top">
                        <CreateSessionButton
                          className="action-button"
                          variant="primary"
                          onClick={(e) => handleCreateSession(sample, e)}
                          disabled={creatingSessionId === sample.id}
                          hasDeleteButton={true}
                        >
                          {creatingSessionId === sample.id ? '⏳' : '▶️'}{' '}
                          {creatingSessionId === sample.id ? 'Creating...' : 'Create Session'}
                        </CreateSessionButton>
                      </SKTooltip>
                    )}
                    <SKTooltip content="Delete this sample pipeline" side="top">
                      <DeleteButton
                        className="action-button"
                        variant="danger"
                        onClick={(e) => handleDeleteClick(sample, e)}
                      >
                        🗑️ Delete
                      </DeleteButton>
                    </SKTooltip>
                  </SampleCard>
                ))}
              </>
            )}

            {fragments.length > 0 && (
              <>
                <SectionHeader>Fragments</SectionHeader>
                {fragments.map((fragment) => {
                  const { nodes } = yamlToFragment(fragment.yaml);
                  const nodeCount = Object.keys(nodes).length;
                  // Count connections from needs dependencies
                  const connectionCount = Object.values(nodes).reduce((count, node) => {
                    if (node.needs) {
                      return count + (Array.isArray(node.needs) ? node.needs.length : 1);
                    }
                    return count;
                  }, 0);
                  return (
                    <FragmentCard
                      key={fragment.id}
                      draggable={!!onFragmentDragStart}
                      onDragStart={(e) => handleFragmentDragStart(e, fragment)}
                      onClick={() => handleFragmentClick(fragment)}
                    >
                      <FragmentName>{fragment.name}</FragmentName>
                      {fragment.description && (
                        <FragmentDescription>{fragment.description}</FragmentDescription>
                      )}
                      <FragmentMeta>
                        <span>
                          {nodeCount} node{nodeCount !== 1 ? 's' : ''}
                        </span>
                        <span>•</span>
                        <span>
                          {connectionCount} connection{connectionCount !== 1 ? 's' : ''}
                        </span>
                      </FragmentMeta>
                      {fragment.tags.length > 0 && (
                        <TagsContainer>
                          {fragment.tags.map((tag) => (
                            <Tag key={tag}>{tag}</Tag>
                          ))}
                        </TagsContainer>
                      )}
                      <SKTooltip content="Delete this fragment" side="top">
                        <DeleteButton
                          className="action-button"
                          variant="danger"
                          onClick={(e) => handleFragmentDeleteClick(fragment, e)}
                        >
                          🗑️ Delete
                        </DeleteButton>
                      </SKTooltip>
                    </FragmentCard>
                  );
                })}
              </>
            )}
          </SamplesList>
        )}
        <ConfirmModal
          isOpen={showDeleteModal}
          title="Delete Sample Pipeline"
          message={`Are you sure you want to delete "${sampleToDelete?.name}"? This action cannot be undone.`}
          confirmLabel="Delete"
          cancelLabel="Cancel"
          onConfirm={handleConfirmDelete}
          onCancel={handleCancelDelete}
          isLoading={isDeleting}
        />
        <ConfirmModal
          isOpen={showFragmentDeleteModal}
          title="Delete Fragment"
          message={`Are you sure you want to delete fragment "${fragmentToDelete?.name}"? This action cannot be undone.`}
          confirmLabel="Delete"
          cancelLabel="Cancel"
          onConfirm={handleConfirmFragmentDelete}
          onCancel={handleCancelFragmentDelete}
          isLoading={isDeleting}
        />
      </PaneWrapper>
    );
  }
);

SamplePipelinesPane.displayName = 'SamplePipelinesPane';

export default memo(SamplePipelinesPane);
