// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { setPacketTypeRegistry } from '@/stores/packetTypeRegistry';
import type { NodeDefinition, PacketType } from '@/types/types';

import { parseYamlToPipeline } from './yamlPipeline';

function makeSourceNodeDef(kind: string, producesType: PacketType): NodeDefinition {
  const nodeDef: NodeDefinition = {
    kind,
    param_schema: {},
    inputs: [{ name: 'in', accepts_types: ['Any'], cardinality: 'One' }],
    outputs: [{ name: 'out', produces_type: producesType, cardinality: 'Broadcast' }],
    categories: [],
    bidirectional: false,
  };
  return nodeDef;
}

function makeSinkNodeDef(kind: string, acceptsTypes: PacketType[] = ['Any']): NodeDefinition {
  return {
    kind,
    param_schema: {},
    inputs: [{ name: 'in', accepts_types: acceptsTypes, cardinality: 'One' }],
    outputs: [],
    categories: [],
    bidirectional: false,
  };
}

describe('parseYamlToPipeline', () => {
  it('infers resampler output type from params for validation', () => {
    setPacketTypeRegistry([
      {
        id: 'RawAudio',
        label: 'Raw Audio',
        color: '#f39c12',
        display_template: 'Raw Audio ({sample_rate|*}Hz, {channels|*}ch, {sample_format})',
        compatibility: {
          kind: 'structfieldwildcard',
          fields: [
            { name: 'sample_rate', wildcard_value: 0 },
            { name: 'channels', wildcard_value: 0 },
            { name: 'sample_format', wildcard_value: null },
          ],
        },
      },
    ]);

    const yaml = `
mode: dynamic
nodes:
  resample_for_stt:
    kind: audio::resampler
    params:
      target_sample_rate: 16000

  whisper_stt:
    kind: plugin::native::whisper
    needs: resample_for_stt
`;

    const nodeDefinitions: NodeDefinition[] = [
      {
        kind: 'audio::resampler',
        param_schema: {},
        inputs: [
          {
            name: 'in',
            accepts_types: [{ RawAudio: { sample_rate: 0, channels: 0, sample_format: 'F32' } }],
            cardinality: 'One',
          },
        ],
        outputs: [
          {
            name: 'out',
            produces_type: { RawAudio: { sample_rate: 48000, channels: 0, sample_format: 'F32' } },
            cardinality: 'Broadcast',
          },
        ],
        categories: [],
        bidirectional: false,
      },
      {
        kind: 'plugin::native::whisper',
        param_schema: {},
        inputs: [
          {
            name: 'in',
            accepts_types: [
              { RawAudio: { sample_rate: 16000, channels: 1, sample_format: 'F32' } },
            ],
            cardinality: 'One',
          },
        ],
        outputs: [{ name: 'out', produces_type: 'Transcription', cardinality: 'Broadcast' }],
        categories: [],
        bidirectional: false,
      },
    ];

    let nextId = 1;
    const result = parseYamlToPipeline(
      yaml,
      nodeDefinitions,
      () => {},
      () => {},
      () => `id_${nextId++}`,
      () => {
        nextId = 1;
      }
    );

    expect(result.error).toBeUndefined();
    expect(result.edges).toHaveLength(1);
    expect(
      (result.edges[0]?.data as { resolvedType?: PacketType } | undefined)?.resolvedType
    ).toEqual({ RawAudio: { sample_rate: 16000, channels: 0, sample_format: 'F32' } });
  });

  it('parses DAG needs object form with mode', () => {
    const yaml = `
mode: dynamic
nodes:
  whisper_stt:
    kind: plugin::native::whisper

  stt_telemetry_out:
    kind: core::telemetry_out
    needs:
      node: whisper_stt
      mode: best_effort
`;

    const nodeDefinitions: NodeDefinition[] = [
      makeSourceNodeDef('plugin::native::whisper', 'Transcription'),
      makeSinkNodeDef('core::telemetry_out'),
    ];

    let nextId = 1;
    const result = parseYamlToPipeline(
      yaml,
      nodeDefinitions,
      () => {},
      () => {},
      () => `id_${nextId++}`,
      () => {
        nextId = 1;
      }
    );

    expect(result.error).toBeUndefined();
    expect(result.edges).toHaveLength(1);
    expect((result.edges[0]?.data as { mode?: string } | undefined)?.mode).toBe('best_effort');
  });
});
