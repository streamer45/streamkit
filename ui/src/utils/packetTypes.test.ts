// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { Edge, Node } from '@xyflow/react';
import { beforeEach, describe, expect, it } from 'vitest';

import { setPacketTypeRegistry, usePacketTypeRegistryStore } from '@/stores/packetTypeRegistry';
import type { PacketTypeMeta, PinCardinality } from '@/types/generated/api-types';
import type { PacketType } from '@/types/types';

import {
  canConnect,
  formatPacketType,
  formatPinCardinality,
  getPacketTypeColor,
  getPinCardinalityDescription,
  getPinCardinalityIcon,
  resolveOutputType,
} from './packetTypes';

// Mirrors the server-side defaults in `crates/core/src/packet_meta.rs::packet_type_registry`.
// Kept inline so these tests do not depend on store seeding side-effects from other suites.
const DEFAULT_METAS: PacketTypeMeta[] = [
  {
    id: 'Any',
    label: 'Any',
    color: '#96ceb4',
    display_template: null,
    compatibility: { kind: 'any' },
  },
  {
    id: 'Binary',
    label: 'Binary',
    color: '#45b7d1',
    display_template: null,
    compatibility: { kind: 'exact' },
  },
  {
    id: 'Text',
    label: 'Text',
    color: '#4ecdc4',
    display_template: null,
    compatibility: { kind: 'exact' },
  },
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
  {
    id: 'RawVideo',
    label: 'Raw Video',
    color: '#1abc9c',
    display_template: 'Raw Video ({width|*}x{height|*}, {pixel_format})',
    compatibility: {
      kind: 'structfieldwildcard',
      fields: [
        { name: 'width', wildcard_value: null },
        { name: 'height', wildcard_value: null },
        { name: 'pixel_format', wildcard_value: null },
      ],
    },
  },
  {
    id: 'EncodedAudio',
    label: 'Encoded Audio',
    color: '#ff6b6b',
    display_template: 'Encoded Audio ({codec})',
    compatibility: {
      kind: 'structfieldwildcard',
      fields: [
        { name: 'codec', wildcard_value: null },
        { name: 'codec_private', wildcard_value: null },
      ],
    },
  },
  {
    id: 'EncodedVideo',
    label: 'Encoded Video',
    color: '#2980b9',
    display_template: 'Encoded Video ({codec})',
    compatibility: {
      kind: 'structfieldwildcard',
      fields: [
        { name: 'codec', wildcard_value: null },
        { name: 'bitstream_format', wildcard_value: null },
        { name: 'codec_private', wildcard_value: null },
        { name: 'profile', wildcard_value: null },
        { name: 'level', wildcard_value: null },
      ],
    },
  },
  {
    id: 'Transcription',
    label: 'Transcription',
    color: '#9b59b6',
    display_template: null,
    compatibility: { kind: 'exact' },
  },
  {
    id: 'Custom',
    label: 'Custom',
    color: '#e67e22',
    display_template: 'Custom ({type_id})',
    compatibility: {
      kind: 'structfieldwildcard',
      fields: [{ name: 'type_id', wildcard_value: null }],
    },
  },
];

const CSS_TEXT_MUTED = '#95a5a6';
const CSS_STATUS_STOPPED = '#cccccc';

function seedDefaultRegistry() {
  setPacketTypeRegistry(DEFAULT_METAS);
}

function makeNode(
  id: string,
  data: {
    kind?: string;
    params?: Record<string, unknown>;
    outputs?: Array<{ name: string; produces_type: PacketType; cardinality?: PinCardinality }>;
    inputs?: Array<{
      name: string;
      accepts_types?: PacketType[];
      cardinality?: PinCardinality;
    }>;
  }
): Node {
  const outputs = (data.outputs ?? []).map((o) => ({
    cardinality: 'One' as PinCardinality,
    ...o,
  }));
  const inputs = (data.inputs ?? []).map((i) => ({
    accepts_types: ['Any'] as PacketType[],
    cardinality: 'One' as PinCardinality,
    ...i,
  }));
  return {
    id,
    position: { x: 0, y: 0 },
    data: {
      kind: data.kind ?? 'noop',
      params: data.params ?? {},
      outputs,
      inputs,
    },
  } as Node;
}

beforeEach(() => {
  usePacketTypeRegistryStore.getState().clear();
  seedDefaultRegistry();

  // happy-dom resolves `getComputedStyle(documentElement).getPropertyValue('--var')` to whatever
  // is set on documentElement; seed the two vars `getPacketTypeColor` reads so the assertions
  // can verify the live-browser code path returns a usable CSS color.
  document.documentElement.style.setProperty('--sk-text-muted', CSS_TEXT_MUTED);
  document.documentElement.style.setProperty('--sk-status-stopped', CSS_STATUS_STOPPED);
});

describe('formatPacketType', () => {
  it('returns the meta label for unit variants with no template', () => {
    expect(formatPacketType('Any')).toBe('Any');
    expect(formatPacketType('Binary')).toBe('Binary');
    expect(formatPacketType('Text')).toBe('Text');
    expect(formatPacketType('Transcription')).toBe('Transcription');
  });

  it('returns the documented Passthrough sentinel string', () => {
    expect(formatPacketType('Passthrough')).toBe('Passthrough (inferred from input)');
  });

  it('substitutes concrete field values into a struct template', () => {
    const result = formatPacketType({
      RawAudio: { sample_rate: 48000, channels: 2, sample_format: 'F32' },
    });
    expect(result).toBe('Raw Audio (48000Hz, 2ch, F32)');
  });

  it('renders wildcard fields as "*" when they equal the meta wildcard_value', () => {
    const result = formatPacketType({
      RawAudio: { sample_rate: 0, channels: 0, sample_format: 'F32' },
    });
    expect(result).toBe('Raw Audio (*Hz, *ch, F32)');
  });

  it('renders only matched wildcards while leaving concrete fields intact', () => {
    const result = formatPacketType({
      RawAudio: { sample_rate: 0, channels: 2, sample_format: 'F32' },
    });
    expect(result).toBe('Raw Audio (*Hz, 2ch, F32)');
  });

  it('renders nested RawVideo dimensions and pixel format', () => {
    expect(
      formatPacketType({
        RawVideo: { width: 1920, height: 1080, pixel_format: 'Rgba8' },
      })
    ).toBe('Raw Video (1920x1080, Rgba8)');
  });

  it('renders null-valued RawVideo wildcard fields as *', () => {
    expect(
      formatPacketType({
        RawVideo: { width: null, height: null, pixel_format: 'Rgba8' },
      })
    ).toBe('Raw Video (*x*, Rgba8)');
  });

  it('does not render wildcard when template placeholder omits |*', () => {
    setPacketTypeRegistry([
      ...DEFAULT_METAS,
      {
        id: 'TestNoStar',
        label: 'Test',
        color: '#000',
        display_template: 'Test ({width}x{height|*})',
        compatibility: {
          kind: 'structfieldwildcard',
          fields: [
            { name: 'width', wildcard_value: null },
            { name: 'height', wildcard_value: null },
          ],
        },
      },
    ]);
    const result = formatPacketType({
      TestNoStar: { width: null, height: null },
    } as unknown as PacketType);
    expect(result).toBe('Test (nullx*)');
  });

  it('renders wildcard only for |* placeholders, not plain ones', () => {
    const result = formatPacketType({
      RawAudio: { sample_rate: 0, channels: 2, sample_format: 'F32' },
    });
    expect(result).toBe('Raw Audio (*Hz, 2ch, F32)');
  });

  it('renders actual value for |* placeholder when value does not match wildcard', () => {
    const result = formatPacketType({
      RawAudio: { sample_rate: 44100, channels: 2, sample_format: 'F32' },
    });
    expect(result).toBe('Raw Audio (44100Hz, 2ch, F32)');
  });

  it('renders the Custom template with the supplied type_id', () => {
    expect(formatPacketType({ Custom: { type_id: 'my.custom.type' } })).toBe(
      'Custom (my.custom.type)'
    );
  });

  it('falls back to the raw kind when no meta is registered', () => {
    usePacketTypeRegistryStore.getState().clear();
    expect(formatPacketType('Binary')).toBe('Binary');
  });

  it('returns the meta label when payload is missing on a struct variant', () => {
    // Defensive: a unit variant where the registry has a struct meta should still produce a
    // human-readable label rather than the templated placeholders.
    expect(formatPacketType('RawAudio' as unknown as PacketType)).toBe('Raw Audio');
  });
});

describe('getPacketTypeColor', () => {
  it.each(DEFAULT_METAS.filter((m) => m.id !== 'Any').map((m) => [m.id, m.color] as const))(
    'returns the registered hex color for %s',
    (id, expected) => {
      // Build a representative variant for struct kinds; unit variants are passed as strings.
      const variant: PacketType = ((): PacketType => {
        switch (id) {
          case 'RawAudio':
            return { RawAudio: { sample_rate: 48000, channels: 2, sample_format: 'F32' } };
          case 'RawVideo':
            return { RawVideo: { width: 1920, height: 1080, pixel_format: 'Rgba8' } };
          case 'EncodedAudio':
            return { EncodedAudio: { codec: 'opus', codec_private: null } };
          case 'EncodedVideo':
            return {
              EncodedVideo: {
                codec: 'h264',
                bitstream_format: null,
                codec_private: null,
                profile: null,
                level: null,
              },
            };
          case 'Custom':
            return { Custom: { type_id: 'x' } };
          default:
            return id as PacketType;
        }
      })();
      expect(getPacketTypeColor(variant)).toBe(expected);
    }
  );

  it('returns the resolved --sk-text-muted CSS variable for Passthrough', () => {
    expect(getPacketTypeColor('Passthrough')).toBe(CSS_TEXT_MUTED);
  });

  it('returns the resolved --sk-status-stopped fallback for unknown kinds', () => {
    usePacketTypeRegistryStore.getState().clear();
    expect(getPacketTypeColor('Binary')).toBe(CSS_STATUS_STOPPED);
  });

  it('returns a value that parses as a CSS color (hex) for every registered variant', () => {
    for (const meta of DEFAULT_METAS) {
      const variant: PacketType =
        meta.id === 'RawAudio'
          ? { RawAudio: { sample_rate: 48000, channels: 2, sample_format: 'F32' } }
          : meta.id === 'RawVideo'
            ? { RawVideo: { width: 1920, height: 1080, pixel_format: 'Rgba8' } }
            : meta.id === 'EncodedAudio'
              ? { EncodedAudio: { codec: 'opus', codec_private: null } }
              : meta.id === 'EncodedVideo'
                ? {
                    EncodedVideo: {
                      codec: 'h264',
                      bitstream_format: null,
                      codec_private: null,
                      profile: null,
                      level: null,
                    },
                  }
                : meta.id === 'Custom'
                  ? { Custom: { type_id: 'x' } }
                  : (meta.id as PacketType);

      const result = getPacketTypeColor(variant);
      expect(result, `kind=${meta.id} produced ${JSON.stringify(result)}`).toMatch(
        /^#[0-9a-f]{3,8}$/i
      );
    }
  });
});

describe('formatPinCardinality', () => {
  it.each([
    ['One', '1:1'],
    ['Broadcast', '1:N'],
  ] as const)('formats unit cardinality %s as %s', (input, expected) => {
    expect(formatPinCardinality(input as PinCardinality)).toBe(expected);
  });

  it('formats Dynamic cardinality with the supplied prefix', () => {
    expect(formatPinCardinality({ Dynamic: { prefix: 'track' } })).toBe('Dynamic (track_*)');
  });
});

describe('getPinCardinalityIcon', () => {
  it.each([
    ['One' as PinCardinality, '●'],
    ['Broadcast' as PinCardinality, '◉'],
    [{ Dynamic: { prefix: 'in' } } as PinCardinality, '◈'],
  ])('returns a single glyph for cardinality %j', (input, expected) => {
    const icon = getPinCardinalityIcon(input);
    expect(icon).toBe(expected);
    expect(icon).toHaveLength(1);
  });
});

describe('getPinCardinalityDescription', () => {
  it.each([
    ['One' as PinCardinality, true, 'Accepts exactly one connection'],
    ['One' as PinCardinality, false, 'Connects to one downstream pin'],
    ['Broadcast' as PinCardinality, true, 'Invalid: Broadcast is only for outputs'],
    ['Broadcast' as PinCardinality, false, 'Can connect to multiple downstream pins'],
  ])('describes %j for isInput=%s', (cardinality, isInput, expected) => {
    expect(getPinCardinalityDescription(cardinality, isInput)).toBe(expected);
  });

  it('describes Dynamic inputs with the runtime-prefix hint', () => {
    expect(
      getPinCardinalityDescription({ Dynamic: { prefix: 'track' } } as PinCardinality, true)
    ).toBe('Pins created dynamically at runtime (track_0, track_1, ...)');
  });

  it('describes Dynamic outputs with the runtime-prefix hint', () => {
    expect(
      getPinCardinalityDescription({ Dynamic: { prefix: 'track' } } as PinCardinality, false)
    ).toBe('Outputs created dynamically at runtime (track_0, track_1, ...)');
  });
});

describe('canConnect — truth table', () => {
  const rawAudio48k2: PacketType = {
    RawAudio: { sample_rate: 48000, channels: 2, sample_format: 'F32' },
  };
  const rawAudio16k1: PacketType = {
    RawAudio: { sample_rate: 16000, channels: 1, sample_format: 'F32' },
  };
  const rawAudioWild: PacketType = {
    RawAudio: { sample_rate: 0, channels: 0, sample_format: 'F32' },
  };
  const rawVideoHD: PacketType = {
    RawVideo: { width: 1920, height: 1080, pixel_format: 'Rgba8' },
  };
  const opus: PacketType = { EncodedAudio: { codec: 'opus', codec_private: null } };
  const aac: PacketType = { EncodedAudio: { codec: 'aac', codec_private: null } };
  const opusWithPriv: PacketType = {
    EncodedAudio: { codec: 'opus', codec_private: [1, 2, 3] },
  };
  const h264: PacketType = {
    EncodedVideo: {
      codec: 'h264',
      bitstream_format: null,
      codec_private: null,
      profile: null,
      level: null,
    },
  };
  const vp9: PacketType = {
    EncodedVideo: {
      codec: 'vp9',
      bitstream_format: null,
      codec_private: null,
      profile: null,
      level: null,
    },
  };

  it.each<[string, PacketType, PacketType[], boolean]>([
    // Any wildcards both directions.
    ['Any -> Any', 'Any', ['Any'], true],
    ['Any -> RawAudio', 'Any', [rawAudio48k2], true],
    ['RawAudio -> Any', rawAudio48k2, ['Any'], true],

    // Passthrough on the output side is the inference sentinel and matches anything.
    ['Passthrough -> RawAudio', 'Passthrough', [rawAudio48k2], true],
    ['Passthrough -> Binary', 'Passthrough', ['Binary'], true],

    // Passthrough only makes sense as an output; on an input pin it has no registered meta
    // and therefore fails the kind-equality check.
    ['RawAudio -> Passthrough', rawAudio48k2, ['Passthrough'], false],

    // Exact-compatibility unit variants must match exactly.
    ['Binary -> Binary', 'Binary', ['Binary'], true],
    ['Text -> Text', 'Text', ['Text'], true],
    ['Transcription -> Transcription', 'Transcription', ['Transcription'], true],
    ['Binary -> Text', 'Binary', ['Text'], false],
    ['Text -> Transcription', 'Text', ['Transcription'], false],

    // Different kinds never connect (Audio vs Video, raw vs encoded).
    ['RawAudio -> RawVideo', rawAudio48k2, [rawVideoHD], false],
    ['EncodedAudio -> RawAudio', opus, [rawAudio48k2], false],
    ['RawAudio -> EncodedAudio', rawAudio48k2, [opus], false],
    ['EncodedVideo -> RawVideo', h264, [rawVideoHD], false],

    // Struct compatibility: exact field-value matches connect.
    ['RawAudio(48k,2ch) -> RawAudio(48k,2ch)', rawAudio48k2, [rawAudio48k2], true],

    // Wildcards on EITHER side erase the field mismatch.
    ['RawAudio(wildcard) -> RawAudio(48k,2ch)', rawAudioWild, [rawAudio48k2], true],
    ['RawAudio(48k,2ch) -> RawAudio(wildcard)', rawAudio48k2, [rawAudioWild], true],

    // Real differences (no wildcard) block the connection.
    ['RawAudio(48k,2ch) -> RawAudio(16k,1ch)', rawAudio48k2, [rawAudio16k1], false],

    // Encoded audio: codec has no wildcard, codec_private does.
    ['Opus -> Opus', opus, [opus], true],
    ['Opus(null priv) -> Opus(with priv)', opus, [opusWithPriv], true],
    ['Opus(with priv) -> Opus(null priv)', opusWithPriv, [opus], true],
    ['Opus -> AAC', opus, [aac], false],
    ['AAC -> Opus', aac, [opus], false],

    // Encoded video: codec must match.
    ['H264 -> H264', h264, [h264], true],
    ['H264 -> VP9', h264, [vp9], false],
  ])('%s', (_label, out, accepts, expected) => {
    expect(canConnect(out, accepts)).toBe(expected);
  });

  it('returns true when ANY accepted input type matches', () => {
    expect(canConnect(rawAudio48k2, ['Binary', 'Text', rawAudio48k2])).toBe(true);
  });

  it('returns false when no accepted input type matches', () => {
    expect(canConnect(rawAudio48k2, ['Binary', 'Text', rawVideoHD])).toBe(false);
  });

  it('returns false on an empty accepted-types list', () => {
    expect(canConnect(rawAudio48k2, [])).toBe(false);
  });

  it('treats unregistered kinds as incompatible', () => {
    usePacketTypeRegistryStore.getState().clear();
    expect(canConnect('Binary', ['Binary'])).toBe(false);
  });
});

describe('resolveOutputType', () => {
  it('returns "Any" when the source pin does not exist on the node', () => {
    const node = makeNode('n1', {
      outputs: [{ name: 'out', produces_type: 'Binary' }],
    });
    expect(resolveOutputType(node, 'does_not_exist', [node], [])).toBe('Any');
  });

  it('returns the declared produces_type for an ordinary output pin', () => {
    const node = makeNode('n1', {
      outputs: [{ name: 'out', produces_type: 'Binary' }],
    });
    expect(resolveOutputType(node, 'out', [node], [])).toBe('Binary');
  });

  it('defaults the source handle to "out" when null is passed', () => {
    const node = makeNode('n1', {
      outputs: [{ name: 'out', produces_type: 'Text' }],
    });
    expect(resolveOutputType(node, null, [node], [])).toBe('Text');
  });

  it('infers compositor output from width/height params', () => {
    const compositor = makeNode('cmp', {
      kind: 'video::compositor',
      params: { width: 1280, height: 720 },
      outputs: [
        {
          name: 'out',
          produces_type: { RawVideo: { width: null, height: null, pixel_format: 'Rgba8' } },
        },
      ],
    });

    expect(resolveOutputType(compositor, 'out', [compositor], [])).toEqual({
      RawVideo: { width: 1280, height: 720, pixel_format: 'Rgba8' },
    });
  });

  it('falls back to compositor output pin payload when params are missing', () => {
    const compositor = makeNode('cmp', {
      kind: 'video::compositor',
      params: {},
      outputs: [
        {
          name: 'out',
          produces_type: { RawVideo: { width: 640, height: 480, pixel_format: 'Rgba8' } },
        },
      ],
    });

    expect(resolveOutputType(compositor, 'out', [compositor], [])).toEqual({
      RawVideo: { width: 640, height: 480, pixel_format: 'Rgba8' },
    });
  });

  it('infers resampler RawAudio output from target_sample_rate', () => {
    const resampler = makeNode('rs', {
      kind: 'audio::resampler',
      params: { target_sample_rate: 16000 },
      outputs: [
        {
          name: 'out',
          produces_type: { RawAudio: { sample_rate: 0, channels: 0, sample_format: 'F32' } },
        },
      ],
    });

    expect(resolveOutputType(resampler, 'out', [resampler], [])).toEqual({
      RawAudio: { sample_rate: 16000, channels: 0, sample_format: 'F32' },
    });
  });

  it('coerces string target_sample_rate params (forms emit strings)', () => {
    const resampler = makeNode('rs', {
      kind: 'audio::resampler',
      params: { target_sample_rate: '48000' },
      outputs: [
        {
          name: 'out',
          produces_type: { RawAudio: { sample_rate: 0, channels: 0, sample_format: 'F32' } },
        },
      ],
    });

    expect(resolveOutputType(resampler, 'out', [resampler], [])).toEqual({
      RawAudio: { sample_rate: 48000, channels: 0, sample_format: 'F32' },
    });
  });

  it('falls back to the declared produces_type when resampler param is missing or invalid', () => {
    const declared: PacketType = {
      RawAudio: { sample_rate: 8000, channels: 1, sample_format: 'F32' },
    };
    const cases: Array<Record<string, unknown>> = [
      {},
      { target_sample_rate: 0 },
      { target_sample_rate: -1 },
      { target_sample_rate: 'not-a-number' },
    ];

    for (const params of cases) {
      const resampler = makeNode('rs', {
        kind: 'audio::resampler',
        params,
        outputs: [{ name: 'out', produces_type: declared }],
      });
      expect(resolveOutputType(resampler, 'out', [resampler], [])).toEqual(declared);
    }
  });

  it('traces a Passthrough output back to its upstream produces_type', () => {
    const upstream = makeNode('src', {
      kind: 'source',
      outputs: [{ name: 'out', produces_type: 'Binary' }],
    });
    const passthrough = makeNode('pt', {
      kind: 'core::passthrough',
      inputs: [{ name: 'in' }],
      outputs: [{ name: 'out', produces_type: 'Passthrough' }],
    });
    const edges: Edge[] = [
      {
        id: 'e1',
        source: 'src',
        sourceHandle: 'out',
        target: 'pt',
        targetHandle: 'in',
      },
    ];

    expect(resolveOutputType(passthrough, 'out', [upstream, passthrough], edges)).toBe('Binary');
  });

  it('returns "Any" for a Passthrough output with no incoming edge', () => {
    const passthrough = makeNode('pt', {
      kind: 'core::passthrough',
      inputs: [{ name: 'in' }],
      outputs: [{ name: 'out', produces_type: 'Passthrough' }],
    });
    expect(resolveOutputType(passthrough, 'out', [passthrough], [])).toBe('Any');
  });

  it('traces through a chain of Passthrough nodes to the originating producer', () => {
    const source = makeNode('src', {
      kind: 'source',
      outputs: [
        {
          name: 'out',
          produces_type: { RawAudio: { sample_rate: 48000, channels: 2, sample_format: 'F32' } },
        },
      ],
    });
    const pt1 = makeNode('pt1', {
      kind: 'core::passthrough',
      inputs: [{ name: 'in' }],
      outputs: [{ name: 'out', produces_type: 'Passthrough' }],
    });
    const pt2 = makeNode('pt2', {
      kind: 'core::passthrough',
      inputs: [{ name: 'in' }],
      outputs: [{ name: 'out', produces_type: 'Passthrough' }],
    });
    const edges: Edge[] = [
      { id: 'e1', source: 'src', sourceHandle: 'out', target: 'pt1', targetHandle: 'in' },
      { id: 'e2', source: 'pt1', sourceHandle: 'out', target: 'pt2', targetHandle: 'in' },
    ];

    expect(resolveOutputType(pt2, 'out', [source, pt1, pt2], edges)).toEqual({
      RawAudio: { sample_rate: 48000, channels: 2, sample_format: 'F32' },
    });
  });

  it('respects sourceHandle when the upstream node has multiple outputs', () => {
    const upstream = makeNode('src', {
      kind: 'splitter',
      outputs: [
        { name: 'audio', produces_type: 'Binary' },
        { name: 'video', produces_type: 'Text' },
      ],
    });
    const passthrough = makeNode('pt', {
      kind: 'core::passthrough',
      inputs: [{ name: 'in' }],
      outputs: [{ name: 'out', produces_type: 'Passthrough' }],
    });
    const edges: Edge[] = [
      {
        id: 'e1',
        source: 'src',
        sourceHandle: 'video',
        target: 'pt',
        targetHandle: 'in',
      },
    ];

    expect(resolveOutputType(passthrough, 'out', [upstream, passthrough], edges)).toBe('Text');
  });

  it('does not apply compositor inference to non-compositor nodes that produce RawVideo', () => {
    // The width/height params would be ignored: only `video::compositor` triggers inference.
    const fakeCompositor = makeNode('fake', {
      kind: 'video::passthrough',
      params: { width: 9999, height: 9999 },
      outputs: [
        {
          name: 'out',
          produces_type: { RawVideo: { width: 640, height: 480, pixel_format: 'Rgba8' } },
        },
      ],
    });

    expect(resolveOutputType(fakeCompositor, 'out', [fakeCompositor], [])).toEqual({
      RawVideo: { width: 640, height: 480, pixel_format: 'Rgba8' },
    });
  });
});
