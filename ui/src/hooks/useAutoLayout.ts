// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook that manages auto-layout and fit-view logic for the Monitor View.
 *
 * Encapsulates:
 * - `needsAutoLayout` / `needsFit` state flags
 * - `applyAutoLayout` — computes vertical DAG positions from measured node
 *   heights, patches ReactFlow nodes, saves positions to the position store,
 *   and triggers fitView
 * - `handleAutoLayout` — collects measured heights from the ReactFlow
 *   instance and delegates to `applyAutoLayout` inside `requestAnimationFrame`
 * - Two effects that fire layout / fitView when the flags are set
 */

import type { Node as RFNode, ReactFlowInstance } from '@xyflow/react';
import React, { useState, useEffect, useCallback } from 'react';

import type { Pipeline } from '@/types/types';
import { topoLevelsFromPipeline, verticalLayout } from '@/utils/dag';
import {
  DEFAULT_NODE_WIDTH,
  DEFAULT_NODE_HEIGHT,
  DEFAULT_HORIZONTAL_GAP,
  DEFAULT_VERTICAL_GAP,
  ESTIMATED_HEIGHT_BY_KIND,
} from '@/utils/layoutConstants';
import { viewsLogger } from '@/utils/logger';
import { collectNodeHeights } from '@/utils/reactFlowInstance';

export interface UseAutoLayoutOptions {
  pipeline: Pipeline | undefined | null;
  selectedSessionId: string | null;
  nodesLength: number;
  setNodes: React.Dispatch<React.SetStateAction<RFNode[]>>;
  rf: React.RefObject<ReactFlowInstance | null>;
  updateNodePosition: (sessionId: string, nodeId: string, pos: { x: number; y: number }) => void;
}

export interface UseAutoLayoutReturn {
  needsAutoLayout: boolean;
  setNeedsAutoLayout: React.Dispatch<React.SetStateAction<boolean>>;
  needsFit: boolean;
  setNeedsFit: React.Dispatch<React.SetStateAction<boolean>>;
  applyAutoLayout: (measuredHeights: Record<string, number>) => void;
  handleAutoLayout: () => void;
}

export function useAutoLayout({
  pipeline,
  selectedSessionId,
  nodesLength,
  setNodes,
  rf,
  updateNodePosition,
}: UseAutoLayoutOptions): UseAutoLayoutReturn {
  const [needsAutoLayout, setNeedsAutoLayout] = useState(false);
  const [needsFit, setNeedsFit] = useState(false);

  // Track the fitView timer so it can be cancelled on unmount
  const fitTimerRef = React.useRef<ReturnType<typeof setTimeout> | null>(null);

  const applyAutoLayout = useCallback(
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

      // Save auto-layout positions to position store so we don't need to re-run layout next time
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
      if (fitTimerRef.current !== null) clearTimeout(fitTimerRef.current);
      fitTimerRef.current = setTimeout(() => {
        fitTimerRef.current = null;
        viewsLogger.debug('Auto-layout complete, fitting view');
        // No animation for better performance on initial load
        rf.current?.fitView({ padding: 0.2, duration: 0 });
      }, 100);
    },
    [pipeline, setNodes, selectedSessionId, updateNodePosition, rf]
  );

  const handleAutoLayout = useCallback(() => {
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
  }, [pipeline, applyAutoLayout, rf]);

  // Auto-layout when selecting a session: perform once after pipeline/nodes load
  useEffect(() => {
    if (needsAutoLayout && selectedSessionId && pipeline && nodesLength > 0) {
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
  }, [needsAutoLayout, selectedSessionId, pipeline, nodesLength, handleAutoLayout]);

  // Fit view when selecting a session once nodes are present
  // Skip if auto-layout is running since it handles fitView itself
  useEffect(() => {
    if (needsFit && selectedSessionId && nodesLength > 0 && !needsAutoLayout) {
      viewsLogger.debug('FitView requested, waiting for nodes to settle');
      const t = setTimeout(() => {
        viewsLogger.debug('Fitting view to nodes');
        // No animation on initial load for better performance
        rf.current?.fitView({ padding: 0.2, duration: 0 });
        setNeedsFit(false);
      }, 150); // Increased delay to ensure nodes are positioned
      return () => clearTimeout(t);
    }
  }, [needsFit, selectedSessionId, nodesLength, needsAutoLayout, rf]);

  // Cancel any pending fitView timer on unmount
  useEffect(() => {
    return () => {
      if (fitTimerRef.current !== null) clearTimeout(fitTimerRef.current);
    };
  }, []);

  return {
    needsAutoLayout,
    setNeedsAutoLayout,
    needsFit,
    setNeedsFit,
    applyAutoLayout,
    handleAutoLayout,
  };
}
