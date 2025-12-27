// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { Position, useUpdateNodeInternals, useStore } from '@xyflow/react';
import React, { useEffect, useMemo } from 'react';

import type { InputPin, OutputPin, PacketType } from '@/types/types';

import { PinHandle } from './PinHandle';

const GUTTER_PCT = 8; // keep pins away from the corners

function percent(i: number, total: number) {
  if (total <= 1) return 50;
  const usable = 100 - 2 * GUTTER_PCT;
  return GUTTER_PCT + (i * usable) / (total - 1);
}

// Helper: Find upstream output pin for a node's first input
function findUpstreamOutputType(
  nodeId: string,
  nodes: Map<string, unknown>,
  edges: Array<{
    source: string;
    target: string;
    sourceHandle?: string | null;
    targetHandle?: string | null;
  }>
): PacketType | null {
  // Find the first input pin on this node
  const node = nodes.get(nodeId);
  const inputPins = (node as { data?: { inputs?: InputPin[] } })?.data?.inputs;
  if (!inputPins || inputPins.length === 0) return null;

  // Find the edge connecting to this input
  const inputPinName = inputPins[0].name;
  const incomingEdge = edges.find((e) => e.target === nodeId && e.targetHandle === inputPinName);
  if (!incomingEdge) return null;

  // Find the upstream node producing data
  const upstreamNode = nodes.get(incomingEdge.source);
  if (!upstreamNode) return null;

  // Find the specific output pin on the upstream node
  const upstreamOutputs = ((upstreamNode as { data?: { outputs?: OutputPin[] } })?.data?.outputs ||
    []) as OutputPin[];
  const upstreamOutput = upstreamOutputs.find(
    (o) => o.name === (incomingEdge.sourceHandle || 'out')
  );

  // Return type if upstream output has explicit type (not another Passthrough)
  if (upstreamOutput && upstreamOutput.produces_type !== 'Passthrough') {
    return upstreamOutput.produces_type;
  }

  return null;
}

type PinRowProps = {
  nodeId: string;
  side: 'top' | 'bottom' | 'left' | 'right';
  pins: InputPin[] | OutputPin[];
  isInput: boolean;
  totalPins?: number; // Total pins including ghost pins for proper spacing
};

export const PinRow: React.FC<PinRowProps> = ({ nodeId, side, pins, isInput, totalPins }) => {
  const update = useUpdateNodeInternals();
  const edges = useStore((state) => state.edges);
  const nodes = useStore((state) => state.nodeLookup);

  useEffect(() => {
    update(nodeId);
  }, [nodeId, pins.length, side, update]);

  const pos =
    side === 'top'
      ? Position.Top
      : side === 'bottom'
        ? Position.Bottom
        : side === 'left'
          ? Position.Left
          : Position.Right;

  const total = totalPins ?? pins.length;

  // Resolve Passthrough output pin types based on connections
  const resolvedOutputTypes = useMemo(() => {
    if (isInput) return new Map<string, PacketType>();

    const resolved = new Map<string, PacketType>();
    for (const pin of pins) {
      const outputPin = pin as OutputPin;

      // Skip non-Passthrough pins - they have explicit types
      if (outputPin.produces_type !== 'Passthrough') continue;

      // Resolve the type by finding upstream connection
      const resolvedType = findUpstreamOutputType(nodeId, nodes, edges);
      if (resolvedType) {
        resolved.set(outputPin.name, resolvedType);
      }
    }
    return resolved;
  }, [isInput, pins, edges, nodes, nodeId]);

  return (
    <>
      {pins.map((p, i: number) => {
        const name = p.name;
        let packetType: PacketType;

        if (isInput) {
          packetType = ((p as InputPin).accepts_types?.[0] ?? 'Any') as PacketType;
        } else {
          const outputPin = p as OutputPin;
          // Use resolved type if available, otherwise use declared type
          packetType = resolvedOutputTypes.get(name) || outputPin.produces_type;
        }

        const cardinality = p.cardinality;

        return (
          <PinHandle
            key={`${side}-${name}`}
            id={name}
            name={name}
            packetType={packetType}
            cardinality={cardinality}
            type={isInput ? 'target' : 'source'}
            position={pos}
            leftPercent={side === 'top' || side === 'bottom' ? percent(i, total) : undefined}
            topPercent={side === 'left' || side === 'right' ? percent(i, total) : undefined}
          />
        );
      })}
    </>
  );
};
