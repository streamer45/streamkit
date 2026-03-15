// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Pure helper functions for computing the diff between a staged pipeline
 * and a live pipeline.  Used by the Monitor View's commit flow.
 */

import type { Connection, Node, Pipeline, BatchOperation } from '@/types/types';

import { viewsLogger } from './logger';

/** Find nodes that were added in staged pipeline */
export const computeAddedNodes = (
  stagedPipeline: Pipeline,
  livePipeline: Pipeline
): BatchOperation[] => {
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
export const computeRemovedNodes = (
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
export const computeConnectionChanges = (
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
export const preprocessMixerNodes = (operations: BatchOperation[]): void => {
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
