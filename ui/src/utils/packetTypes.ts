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
  PixelFormat,
} from '@/types/generated/api-types';
import type { PacketType } from '@/types/types';
import { deepEqual } from '@/utils/deepEqual';

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
  return template.replace(/\{(\w+)(\|\*)?\}/g, (_m, field: string, star?: string) => {
    const value = (payload as Record<string, unknown> | undefined)?.[field];
    let isWildcard = false;
    if (compat && compat.kind === 'structfieldwildcard' && star) {
      const rule = compat.fields.find((f) => f.name === field);
      if (rule && 'wildcard_value' in rule) {
        isWildcard = deepEqual(value, rule.wildcard_value);
      }
    }
    if (isWildcard) return '*';
    return String(value);
  });
}

export function formatPacketType(packetType: PacketType): string {
  const { kind, payload } = variantOf(packetType);

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

  return kind;
}

export function getPacketTypeColor(packetType: PacketType): string {
  const { kind } = variantOf(packetType);

  if (kind === 'Passthrough') {
    if (typeof window !== 'undefined') {
      return getComputedStyle(document.documentElement).getPropertyValue('--sk-text-muted').trim();
    }
    return '#95a5a6';
  }

  const meta = getPacketTypeMeta(kind);
  if (meta) {
    return meta.color;
  }

  if (typeof window !== 'undefined') {
    return getComputedStyle(document.documentElement)
      .getPropertyValue('--sk-status-stopped')
      .trim();
  }
  return '#95a5a6';
}

function canConnectPair(out: PacketType, input: PacketType): boolean {
  const a = variantOf(out);
  const b = variantOf(input);

  if (a.kind === 'Passthrough') {
    return true;
  }

  if (a.kind === 'Any' || b.kind === 'Any') {
    return true;
  }

  const ma = getPacketTypeMeta(a.kind);
  const mb = getPacketTypeMeta(b.kind);

  if (ma && mb) {
    if (a.kind !== b.kind) {
      return false;
    }

    const compat = ma.compatibility;
    if (compat.kind === 'any') return true;
    if (compat.kind === 'exact') return true;

    if (compat.kind === 'structfieldwildcard') {
      const ap = (a.payload as Record<string, unknown> | undefined) ?? {};
      const bp = (b.payload as Record<string, unknown> | undefined) ?? {};
      return compat.fields.every((f) => {
        const av = (ap as Record<string, unknown>)[f.name];
        const bv = (bp as Record<string, unknown>)[f.name];
        const wildcard = f.wildcard_value;
        const isWild = (v: unknown) => wildcard !== undefined && deepEqual(v, wildcard);
        return isWild(av) || isWild(bv) || deepEqual(av, bv);
      });
    }

    return false;
  }

  return false;
}

export function canConnect(outputType: PacketType, inputTypes: PacketType[]): boolean {
  return inputTypes.some((it) => canConnectPair(outputType, it));
}

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

  if (typeof cardinality === 'object' && 'Dynamic' in cardinality) {
    const prefix = cardinality.Dynamic.prefix;
    return `Dynamic (${prefix}_*)`;
  }

  return 'Unknown';
}

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

  if (typeof cardinality === 'object' && 'Dynamic' in cardinality) {
    return '◈';
  }

  return '○';
}

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

  if (typeof cardinality === 'object' && 'Dynamic' in cardinality) {
    const prefix = cardinality.Dynamic.prefix;
    return isInput
      ? `Pins created dynamically at runtime (${prefix}_0, ${prefix}_1, ...)`
      : `Outputs created dynamically at runtime (${prefix}_0, ${prefix}_1, ...)`;
  }

  return 'Unknown cardinality';
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

function inferCompositorOutputType(sourceNode: Node, sourceOutput: OutputPin): PacketType | null {
  const sourceKind = getNodeKind(sourceNode);
  if (sourceKind !== 'video::compositor' || sourceOutput.name !== 'out') return null;

  const params = getNodeParams(sourceNode);
  const outVariant = variantOf(sourceOutput.produces_type);
  if (outVariant.kind !== 'RawVideo') return null;

  const payload = (outVariant.payload as Record<string, unknown> | undefined) ?? {};

  const width: number | null =
    typeof params.width === 'number'
      ? params.width
      : typeof payload.width === 'number'
        ? payload.width
        : null;
  const height: number | null =
    typeof params.height === 'number'
      ? params.height
      : typeof payload.height === 'number'
        ? payload.height
        : null;

  return {
    RawVideo: {
      width,
      height,
      pixel_format: pixelFormatFromOutputFormat(params.output_format),
    },
  };
}

// Mirrors the server's `parse_pixel_format` (crates/nodes/src/video/mod.rs).
// `PixelFormat` is `#[non_exhaustive]`: any new variant must be added here too,
// otherwise it falls back to Rgba8 and connection validation wrongly rejects it.
function pixelFormatFromOutputFormat(outputFormat: unknown): PixelFormat {
  if (typeof outputFormat !== 'string') return 'Rgba8';
  switch (outputFormat.toLowerCase()) {
    case 'nv12':
      return 'Nv12';
    case 'i420':
      return 'I420';
    default:
      return 'Rgba8';
  }
}

function inferResamplerOutputType(sourceNode: Node, sourceOutput: OutputPin): PacketType | null {
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

function inferConfiguredOutputType(sourceNode: Node, sourceOutput: OutputPin): PacketType | null {
  return (
    inferCompositorOutputType(sourceNode, sourceOutput) ??
    inferResamplerOutputType(sourceNode, sourceOutput)
  );
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

/** Resolve the actual packet type for an output pin, tracing through Passthrough nodes. */
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

  // UI-only inference from node params; server remains authoritative at runtime.
  const inferred = inferConfiguredOutputType(sourceNode, sourceOutput);
  if (inferred) return inferred;

  if (sourceOutput.produces_type !== 'Passthrough') {
    return sourceOutput.produces_type;
  }

  const upstream = resolvePassthroughSource(sourceNode, nodes, edges);
  if (!upstream) return 'Any';
  return resolveOutputType(upstream.upstreamNode, upstream.upstreamHandle, nodes, edges);
}
