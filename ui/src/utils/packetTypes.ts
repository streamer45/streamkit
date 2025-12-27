// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { Node, Edge } from '@xyflow/react';

import { getPacketTypeMeta } from '@/stores/packetTypeRegistry';
import type {
  Compatibility,
  PinCardinality,
  InputPin,
  OutputPin,
} from '@/types/generated/api-types';
import type { PacketType } from '@/types/types';
import { deepEqual } from '@/utils/deepEqual';

/**
 * Minimal client-side packet type registry, driven by the server.
 * This keeps the UI generic and scalable. There is no client-side fallback:
 * the UI relies on the server-provided registry as the single source of truth.
 */

// Types imported from generated bindings

function variantOf(packetType: PacketType): { kind: string; payload?: unknown } {
  if (typeof packetType === 'string') {
    return { kind: packetType };
  }
  const entries = Object.entries(packetType as Record<string, unknown>);
  if (entries.length === 1) {
    const [kind, payload] = entries[0] as [string, unknown];
    return { kind, payload };
  }
  return { kind: 'Unknown' };
}

function formatWithTemplate(
  template: string,
  payload: Record<string, unknown> | undefined,
  compat?: Compatibility
): string {
  return template.replace(/\{(\w+)(\|\*)?\}/g, (_m, field: string) => {
    const value = (payload as Record<string, unknown> | undefined)?.[field];
    let isWildcard = false;
    if (compat && compat.kind === 'structfieldwildcard') {
      const rule = compat.fields.find((f) => f.name === field);
      const wildcard = rule?.wildcard_value;
      if (wildcard !== undefined && wildcard !== null) {
        isWildcard = deepEqual(value, wildcard);
      }
    }
    if (isWildcard) return '*';
    return String(value);
  });
}

export function formatPacketType(packetType: PacketType): string {
  const { kind, payload } = variantOf(packetType);

  // Special case for Passthrough type
  if (kind === 'Passthrough') {
    return 'Passthrough (inferred from input)';
  }

  const meta = getPacketTypeMeta(kind);
  if (meta) {
    if (meta.display_template && payload && typeof payload === 'object') {
      return formatWithTemplate(
        meta.display_template,
        payload as Record<string, unknown>,
        meta.compatibility
      );
    }
    return meta.label;
  }

  // No client fallback: rely on server-provided registry only
  return kind;
}

export function getPacketTypeColor(packetType: PacketType): string {
  const { kind } = variantOf(packetType);

  // Special neutral color for Passthrough (will be resolved at connection time)
  if (kind === 'Passthrough') {
    if (typeof window !== 'undefined') {
      return getComputedStyle(document.documentElement).getPropertyValue('--sk-text-muted').trim();
    }
    return '#95a5a6'; // SSR fallback
  }

  const meta = getPacketTypeMeta(kind);
  if (meta) {
    return meta.color;
  }

  // Fallback color for unknown packet types (use CSS variable for consistency)
  // Get computed value at runtime to support theme switching
  if (typeof window !== 'undefined') {
    return getComputedStyle(document.documentElement)
      .getPropertyValue('--sk-status-stopped')
      .trim();
  }
  return '#95a5a6'; // SSR fallback
}

function canConnectPair(out: PacketType, input: PacketType): boolean {
  const a = variantOf(out);
  const b = variantOf(input);

  // Passthrough can't be validated at connection time - needs type inference
  // Allow it for now, validation will happen during pipeline validation
  if (a.kind === 'Passthrough') {
    return true;
  }

  // Any matches anything
  if (a.kind === 'Any' || b.kind === 'Any') {
    return true;
  }

  const ma = getPacketTypeMeta(a.kind);
  const mb = getPacketTypeMeta(b.kind);

  if (ma && mb) {
    // Kinds must match for these v1 strategies
    if (a.kind !== b.kind) {
      return false;
    }

    const compat = ma.compatibility; // symmetric by convention
    if (compat.kind === 'any') return true;
    if (compat.kind === 'exact') return true;

    if (compat.kind === 'structfieldwildcard') {
      const ap = (a.payload as Record<string, unknown> | undefined) ?? {};
      const bp = (b.payload as Record<string, unknown> | undefined) ?? {};
      return compat.fields.every((f) => {
        const av = (ap as Record<string, unknown>)[f.name];
        const bv = (bp as Record<string, unknown>)[f.name];
        const wildcard = f.wildcard_value;
        const isWild = (v: unknown) =>
          wildcard !== undefined && wildcard !== null && deepEqual(v, wildcard);
        return isWild(av) || isWild(bv) || deepEqual(av, bv);
      });
    }

    // Unknown strategy: be conservative
    return false;
  }

  // No client fallback: rely on server-provided registry only
  return false;
}

export function canConnect(outputType: PacketType, inputTypes: PacketType[]): boolean {
  return inputTypes.some((it) => canConnectPair(outputType, it));
}

/**
 * Formats pin cardinality for human-readable display
 */
export function formatPinCardinality(cardinality: PinCardinality): string {
  if (typeof cardinality === 'string') {
    switch (cardinality) {
      case 'One':
        return '1:1';
      case 'Broadcast':
        return '1:N';
      default:
        return cardinality;
    }
  }

  // Dynamic cardinality
  if (typeof cardinality === 'object' && 'Dynamic' in cardinality) {
    const prefix = cardinality.Dynamic.prefix;
    return `Dynamic (${prefix}_*)`;
  }

  return 'Unknown';
}

/**
 * Gets a visual icon/symbol for the cardinality type
 */
export function getPinCardinalityIcon(cardinality: PinCardinality): string {
  if (typeof cardinality === 'string') {
    switch (cardinality) {
      case 'One':
        return '●'; // Single dot
      case 'Broadcast':
        return '◉'; // Dot with ring (broadcast)
      default:
        return '○';
    }
  }

  // Dynamic cardinality
  if (typeof cardinality === 'object' && 'Dynamic' in cardinality) {
    return '◈'; // Diamond for dynamic
  }

  return '○';
}

/**
 * Gets a tooltip description for the cardinality
 */
export function getPinCardinalityDescription(
  cardinality: PinCardinality,
  isInput: boolean
): string {
  if (typeof cardinality === 'string') {
    switch (cardinality) {
      case 'One':
        return isInput ? 'Accepts exactly one connection' : 'Connects to one downstream pin';
      case 'Broadcast':
        return isInput
          ? 'Invalid: Broadcast is only for outputs'
          : 'Can connect to multiple downstream pins';
      default:
        return cardinality;
    }
  }

  // Dynamic cardinality
  if (typeof cardinality === 'object' && 'Dynamic' in cardinality) {
    const prefix = cardinality.Dynamic.prefix;
    return isInput
      ? `Pins created dynamically at runtime (${prefix}_0, ${prefix}_1, ...)`
      : `Outputs created dynamically at runtime (${prefix}_0, ${prefix}_1, ...)`;
  }

  return 'Unknown cardinality';
}

/**
 * Checks if a pin list contains any dynamic cardinality pins
 */
export function hasDynamicPins(pins: Array<{ cardinality: PinCardinality }>): boolean {
  return pins.some((pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality);
}

/**
 * Checks if a pin list is empty or only has dynamic pins (needs placeholder)
 */
export function needsPlaceholderPin(pins: Array<{ cardinality: PinCardinality }>): boolean {
  if (pins.length === 0) return true;
  return hasDynamicPins(pins) && pins.length === 0;
}

function getNodeKind(node: Node): string {
  return ((node.data as Record<string, unknown>).kind as string | undefined) ?? '';
}

function getNodeParams(node: Node): Record<string, unknown> {
  return (((node.data as Record<string, unknown>).params as Record<string, unknown> | undefined) ??
    {}) as Record<string, unknown>;
}

function getOutputPinForHandle(
  sourceNode: Node,
  sourceHandle: string | null
): OutputPin | undefined {
  const outputs = ((sourceNode.data as Record<string, unknown>).outputs || []) as OutputPin[];
  return outputs.find((o) => o.name === (sourceHandle || 'out'));
}

function inferConfiguredOutputType(sourceNode: Node, sourceOutput: OutputPin): PacketType | null {
  const sourceKind = getNodeKind(sourceNode);
  if (sourceKind !== 'audio::resampler' || sourceOutput.name !== 'out') return null;

  const params = getNodeParams(sourceNode);
  const targetSampleRateRaw = params.target_sample_rate;
  const targetSampleRate =
    typeof targetSampleRateRaw === 'number'
      ? targetSampleRateRaw
      : typeof targetSampleRateRaw === 'string'
        ? Number(targetSampleRateRaw)
        : null;

  if (!targetSampleRate || !Number.isFinite(targetSampleRate) || targetSampleRate <= 0) {
    return null;
  }

  const outVariant = variantOf(sourceOutput.produces_type);
  if (outVariant.kind !== 'RawAudio') return null;

  return {
    RawAudio: {
      sample_rate: targetSampleRate,
      channels: 0, // wildcard
      sample_format: 'F32',
    },
  };
}

function resolvePassthroughSource(
  sourceNode: Node,
  nodes: Node[],
  edges: Edge[]
): { upstreamNode: Node; upstreamHandle: string | null } | null {
  const sourceInputs = ((sourceNode.data as Record<string, unknown>).inputs || []) as InputPin[];
  if (sourceInputs.length === 0) return null;

  const inputPinName = sourceInputs[0].name;
  const incomingEdge = edges.find(
    (e) => e.target === sourceNode.id && e.targetHandle === inputPinName
  );
  if (!incomingEdge) return null;

  const upstreamNode = nodes.find((n) => n.id === incomingEdge.source);
  if (!upstreamNode) return null;

  return {
    upstreamNode,
    upstreamHandle: incomingEdge.sourceHandle || null,
  };
}

/**
 * Resolves the actual packet type for an output pin, handling Passthrough type inference.
 * Traces back through the pipeline to find the source type for Passthrough nodes.
 *
 * @param sourceNode - The node whose output type to resolve
 * @param sourceHandle - The name of the output pin (or null for default 'out')
 * @param nodes - All nodes in the pipeline
 * @param edges - All edges in the pipeline
 * @returns The resolved packet type
 */
export function resolveOutputType(
  sourceNode: Node,
  sourceHandle: string | null,
  nodes: Node[],
  edges: Edge[]
): PacketType {
  const sourceOutput = getOutputPinForHandle(sourceNode, sourceHandle);

  if (!sourceOutput) {
    return 'Any';
  }

  // Some nodes have output types that depend on configuration params.
  // Resolve those here so validation + edge metadata reflect the configured pipeline.
  //
  // NOTE: This is UI-only inference to improve UX (YAML import + canvas connections).
  // The server remains authoritative at runtime.
  const inferred = inferConfiguredOutputType(sourceNode, sourceOutput);
  if (inferred) return inferred;

  // If not Passthrough, return as-is
  if (sourceOutput.produces_type !== 'Passthrough') {
    return sourceOutput.produces_type;
  }

  // Passthrough node - trace back to find the input type
  const upstream = resolvePassthroughSource(sourceNode, nodes, edges);
  if (!upstream) return 'Any';
  return resolveOutputType(upstream.upstreamNode, upstream.upstreamHandle, nodes, edges);
}
