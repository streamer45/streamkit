// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import AudioGainNode from '@/nodes/AudioGainNode';
import CompositorNode from '@/nodes/CompositorNode';
import ConfigurableNode from '@/nodes/ConfigurableNode';

export const nodeTypes = {
  audioGain: AudioGainNode,
  compositor: CompositorNode,
  configurable: ConfigurableNode,
};

export interface DefaultEdgeOptions {
  type: string;
  animated: boolean;
  style: {
    stroke: string;
    strokeWidth: number;
  };
}

export const defaultEdgeOptions: DefaultEdgeOptions = {
  type: 'typed',
  animated: true,
  style: {
    stroke: 'var(--sk-primary)',
    strokeWidth: 2,
  },
};
