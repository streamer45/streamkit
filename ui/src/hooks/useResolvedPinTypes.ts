// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useStore } from '@xyflow/react';
import { useMemo } from 'react';

import type { InputPin, OutputPin, PacketType } from '@/types/types';

/**
 * Hook to resolve the actual packet types for output pins of Passthrough nodes.
 * Traces back through connections to infer the actual type.
 */
export function useResolvedOutputPinType(
  nodeId: string,
  outputPin: OutputPin,
  inputPins: InputPin[]
): PacketType {
  const edges = useStore((state) => state.edges);
  const nodes = useStore((state) => state.nodeLookup);

  return useMemo(() => {
    // If not Passthrough, return as-is
    if (outputPin.produces_type !== 'Passthrough') {
      return outputPin.produces_type;
    }

    // Passthrough node - trace back to find input type
    if (inputPins.length === 0) {
      return 'Any'; // No inputs, can't infer
    }

    // Get the first input pin (passthrough nodes typically have one input)
    const inputPinName = inputPins[0].name;

    // Find the edge that feeds this input
    const incomingEdge = edges.find((e) => e.target === nodeId && e.targetHandle === inputPinName);

    if (!incomingEdge) {
      return 'Any'; // No incoming connection, can't infer
    }

    // Find the upstream node
    const upstreamNode = nodes.get(incomingEdge.source);
    if (!upstreamNode) {
      return 'Any';
    }

    // Get the upstream node's outputs
    const upstreamOutputs = ((upstreamNode.data as Record<string, unknown>)?.outputs ||
      []) as OutputPin[];
    const upstreamOutput = upstreamOutputs.find(
      (o) => o.name === (incomingEdge.sourceHandle || 'out')
    );

    if (!upstreamOutput) {
      return 'Any';
    }

    // If upstream is also Passthrough, we'd need to recursively resolve
    // For now, just use the upstream output type (could be enhanced with recursion)
    if (upstreamOutput.produces_type === 'Passthrough') {
      // Could recursively resolve here, but for simplicity, return Any for now
      // This would be rare (chained passthrough nodes)
      return 'Any';
    }

    return upstreamOutput.produces_type;
  }, [nodeId, outputPin, inputPins, edges, nodes]);
}
