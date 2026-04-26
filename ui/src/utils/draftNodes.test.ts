// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { NodeDefinition } from '@/types/types';

import { computeMissingRequired, defaultParamsForKind } from './draftNodes';

const def = (kind: string, schema: Record<string, unknown>): NodeDefinition =>
  ({
    kind,
    description: '',
    inputs: [],
    outputs: [],
    param_schema: schema,
  }) as unknown as NodeDefinition;

describe('computeMissingRequired', () => {
  const defs: NodeDefinition[] = [
    def('plugin::native::servo', {
      properties: {
        url: { type: 'string', description: 'Stream URL' },
        width: { type: 'integer', default: 1280 },
        height: { type: 'integer', default: 720 },
      },
      required: ['url'],
    }),
    def('audio::gain', {
      properties: { gain: { type: 'number', default: 1.0 } },
    }),
    def('plugin::native::piper', {
      properties: { model_dir: { type: 'string' }, voice: { type: 'string' } },
      required: ['model_dir'],
    }),
  ];

  it('returns the unset required keys', () => {
    expect(computeMissingRequired('plugin::native::servo', {}, defs)).toEqual(['url']);
  });

  it('treats undefined, null, and whitespace strings as missing', () => {
    expect(computeMissingRequired('plugin::native::servo', { url: undefined }, defs)).toEqual([
      'url',
    ]);
    expect(computeMissingRequired('plugin::native::servo', { url: null }, defs)).toEqual(['url']);
    expect(computeMissingRequired('plugin::native::servo', { url: '   ' }, defs)).toEqual(['url']);
  });

  it('returns an empty list once all required keys have a value', () => {
    expect(
      computeMissingRequired(
        'plugin::native::servo',
        { url: 'https://example.com/stream.m3u8' },
        defs
      )
    ).toEqual([]);
  });

  it('returns an empty list when there are no required keys', () => {
    expect(computeMissingRequired('audio::gain', {}, defs)).toEqual([]);
  });

  it('returns an empty list for unknown kinds', () => {
    expect(computeMissingRequired('does::not::exist', {}, defs)).toEqual([]);
  });

  it('treats numbers (including 0) and booleans (including false) as set', () => {
    const numberDef: NodeDefinition[] = [
      def('test::node', {
        properties: { count: { type: 'integer' }, enabled: { type: 'boolean' } },
        required: ['count', 'enabled'],
      }),
    ];
    expect(computeMissingRequired('test::node', { count: 0, enabled: false }, numberDef)).toEqual(
      []
    );
  });
});

describe('defaultParamsForKind', () => {
  const defs: NodeDefinition[] = [
    def('plugin::native::servo', {
      properties: {
        url: { type: 'string' },
        width: { type: 'integer', default: 1280 },
        height: { type: 'integer', default: 720 },
        timeout: { type: 'integer' },
      },
      required: ['url'],
    }),
  ];

  it('only fills properties with explicit defaults', () => {
    expect(defaultParamsForKind('plugin::native::servo', defs)).toEqual({
      width: 1280,
      height: 720,
    });
  });

  it('returns an empty object for kinds with no schema', () => {
    expect(defaultParamsForKind('unknown::kind', defs)).toEqual({});
  });

  it('round-trips with computeMissingRequired (defaults satisfy non-required keys only)', () => {
    const params = defaultParamsForKind('plugin::native::servo', defs);
    expect(computeMissingRequired('plugin::native::servo', params, defs)).toEqual(['url']);
  });
});
