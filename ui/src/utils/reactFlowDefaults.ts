// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Shared ReactFlow configuration defaults used across all views.
 * Centralizes node types and edge options for consistency.
 */

import TypedEdge from '@/components/TypedEdge';
import AudioGainNode from '@/nodes/AudioGainNode';
import ConfigurableNode from '@/nodes/ConfigurableNode';

/**
 * Node type mappings for ReactFlow
 */
export const nodeTypes = {
  audioGain: AudioGainNode,
  configurable: ConfigurableNode,
};

/**
 * Edge type mappings for ReactFlow
 */
export const edgeTypes = {
  typed: TypedEdge,
};

/**
 * Type for default edge options
 */
export interface DefaultEdgeOptions {
  type: string;
  animated: boolean;
  style: {
    stroke: string;
    strokeWidth: number;
  };
}

/**
 * Default edge styling and behavior
 * No arrow markers - direction is clear from top-to-bottom pipeline structure
 */
export const defaultEdgeOptions: DefaultEdgeOptions = {
  type: 'typed', // Use our custom typed edge
  animated: true,
  style: {
    stroke: 'var(--sk-primary)',
    strokeWidth: 2,
  },
};
