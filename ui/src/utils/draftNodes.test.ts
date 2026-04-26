// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { NodeDefinition } from '@/types/types';

import { computeMissingRequired, defaultParamsForKind, mergeDraftParam } from './draftNodes';

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

describe('mergeDraftParam', () => {
  it('replaces a flat top-level key', () => {
    expect(mergeDraftParam({ width: 1280, height: 720 }, 'url', 'https://x')).toEqual({
      width: 1280,
      height: 720,
      url: 'https://x',
    });
  });

  it('overwrites an existing flat key with the new value (regression: stale-value bug)', () => {
    // Simulates the second keystroke into the same field.  The
    // returned object must reflect the new value so the inspector
    // (driven by nodeParamsAtom mirroring this object) does not
    // freeze on the previous character.
    const after1 = mergeDraftParam({}, 'url', 'h');
    const after2 = mergeDraftParam(after1, 'url', 'ht');
    expect(after2['url']).toBe('ht');
    expect(after1).not.toBe(after2); // new identity each call
  });

  it('writes a dotted path as a nested object instead of a flat key', () => {
    // Regression for finding #2: previously the draft branch stored
    // dot-paths verbatim ({ "properties.show": ... }) which would have
    // been sent to the engine as-is.
    const out = mergeDraftParam({}, 'properties.show', true);
    expect(out).toEqual({ properties: { show: true } });
    expect(out['properties.show']).toBeUndefined();
  });

  it('deep-merges sibling nested keys instead of clobbering them', () => {
    const start = { properties: { show: true, color: 'red' } };
    const out = mergeDraftParam(start, 'properties.color', 'blue');
    expect(out).toEqual({ properties: { show: true, color: 'blue' } });
  });

  it('preserves unrelated top-level keys when editing a nested path', () => {
    const start = { width: 1280 };
    const out = mergeDraftParam(start, 'properties.show', true);
    expect(out).toEqual({ width: 1280, properties: { show: true } });
  });
});
