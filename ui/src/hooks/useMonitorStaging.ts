// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Staging-mode lifecycle for the Monitor view: enter, discard, commit.
 *
 * Extracted from MonitorViewContent to keep staging concerns self-contained
 * and reduce the component's statement count.
 */

import type { Node as RFNode } from '@xyflow/react';
import React, { useCallback, useRef } from 'react';
import { useShallow } from 'zustand/shallow';

import { useToast } from '@/context/ToastContext';
import { useStagingStore, type StagingData } from '@/stores/stagingStore';
import type { Pipeline, BatchOperation, Node as PipelineNode, Response } from '@/types/types';
import { viewsLogger } from '@/utils/logger';
import {
  computeAddedNodes,
  computeRemovedNodes,
  computeConnectionChanges,
  preprocessMixerNodes,
} from '@/utils/pipelineDiff';

interface UseMonitorStagingOptions {
  selectedSessionId: string | null;
  pipelineRef: React.RefObject<Pipeline | null>;
  nodesRef: React.RefObject<RFNode[]>;
  applyBatch: (operations: BatchOperation[]) => Promise<Response | undefined>;
}

export function useMonitorStaging({
  selectedSessionId,
  pipelineRef,
  nodesRef,
  applyBatch,
}: UseMonitorStagingOptions) {
  const toast = useToast();

  // Staging store selectors
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
  const setValidationErrors = useStagingStore((s) => s.setValidationErrors);
  const discardChanges = useStagingStore((s) => s.discardChanges);

  // Derived
  const isInStagingMode = stagingData?.mode === 'staging';
  const stagedPipeline = stagingData?.stagedPipeline ?? null;

  // Ref for async commit handler
  const stagingDataRef = useRef(stagingData);
  stagingDataRef.current = stagingData;

  // ── Handlers ──────────────────────────────────────────────────────────

  const handleEnterStagingMode = useCallback(() => {
    viewsLogger.info('Entering staging mode');
    if (!selectedSessionId || !pipelineRef.current) return;
    enterStagingMode(selectedSessionId, pipelineRef.current);

    // Capture all current node positions from the canvas
    nodesRef.current.forEach((node: RFNode) => {
      updateNodePosition(selectedSessionId, node.id, node.position);
    });
  }, [selectedSessionId, enterStagingMode, updateNodePosition, pipelineRef, nodesRef]);

  const handleDiscardChanges = useCallback(() => {
    viewsLogger.info('Discarding changes (exiting staging mode)');
    if (!selectedSessionId) return;
    discardChanges(selectedSessionId);
  }, [selectedSessionId, discardChanges]);

  const handleCommitChanges = useCallback(async () => {
    const currentPipeline = pipelineRef.current;
    const currentStagingData = stagingDataRef.current;

    if (!selectedSessionId || !currentPipeline || !currentStagingData?.stagedPipeline) return;

    const staged = currentStagingData.stagedPipeline;

    try {
      const operations: BatchOperation[] = [
        ...computeAddedNodes(staged, currentPipeline),
        ...computeRemovedNodes(staged, currentPipeline),
        ...computeConnectionChanges(staged, currentPipeline),
      ];

      if (operations.length === 0) {
        toast.info('No changes to commit');
        return;
      }

      preprocessMixerNodes(operations);

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
  }, [selectedSessionId, applyBatch, toast, exitStagingMode, pipelineRef]);

  const onNodeDragStop = useCallback(
    (_event: React.MouseEvent, node: RFNode) => {
      if (isInStagingMode && selectedSessionId) {
        updateNodePosition(selectedSessionId, node.id, node.position);
      }
    },
    [isInStagingMode, selectedSessionId, updateNodePosition]
  );

  return {
    stagingData,
    isInStagingMode,
    stagedPipeline,
    addStagedNode,
    removeStagedNode,
    addStagedConnection,
    removeStagedConnection,
    updateStagedNodeParams,
    updateNodePosition,
    setValidationErrors,
    handleEnterStagingMode,
    handleDiscardChanges,
    handleCommitChanges,
    onNodeDragStop,
  } as const;
}

// Re-export StagingData for convenience
export type { StagingData };

// Re-export PipelineNode type used by addStagedNode
export type { PipelineNode };
