// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook to register a fitView callback that responds to layout preset changes.
 * Automatically cleans up on unmount.
 */

import { useEffect } from 'react';

import { registerFitViewCallback } from '@/stores/layoutStore';

export interface UseFitViewOnLayoutPresetChangeOptions {
  /**
   * ReactFlow instance ref (accepts any ReactFlowInstance type)
   */
  reactFlowInstance: React.RefObject<{
    fitView: (options: { padding: number; duration: number }) => void;
  } | null>;
  /**
   * Number of nodes currently in the flow (used as dependency to re-register callback)
   */
  nodesCount: number;
  /**
   * Padding for fitView (default: 0.2)
   */
  padding?: number;
  /**
   * Animation duration in ms (default: 300)
   */
  duration?: number;
}

/**
 * Registers a fitView callback that triggers when layout presets change.
 * The callback will fit the view to all nodes with the specified padding and duration.
 */
export const useFitViewOnLayoutPresetChange = (
  options: UseFitViewOnLayoutPresetChangeOptions
): void => {
  const { reactFlowInstance, nodesCount, padding = 0.2, duration = 300 } = options;

  useEffect(() => {
    const unregister = registerFitViewCallback(() => {
      if (reactFlowInstance.current && nodesCount > 0) {
        reactFlowInstance.current.fitView({ padding, duration });
      }
    });
    return unregister;
  }, [reactFlowInstance, nodesCount, padding, duration]);
};
