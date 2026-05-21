// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { CompletionContext, type CompletionResult } from '@codemirror/autocomplete';
import { EditorState } from '@codemirror/state';
import { load as yamlLoad } from 'js-yaml';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { NodeDefinition } from '@/types/generated/api-types';

import { createYamlAutocompletion } from './yamlAutocompletion';

// `vi.mock` is hoisted, so use `vi.hoisted` for state shared with the factory.
//
// Invariant: `hoisted.source` is owned exclusively by `runSource()` below.
// `beforeEach` resets it to null; no other code path may call
// `createYamlAutocompletion` outside `runSource`, or captured sources will
// leak across tests.
const hoisted = vi.hoisted(() => ({
  source: null as ((ctx: CompletionContext) => CompletionResult | null) | null,
}));

vi.mock('@codemirror/autocomplete', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@codemirror/autocomplete')>();
  return {
    ...actual,
    autocompletion: (config: {
      override?: Array<(ctx: CompletionContext) => CompletionResult | null>;
    }) => {
      hoisted.source = config.override?.[0] ?? null;
      // The return value is treated as a CodeMirror extension; tests never feed
      // it back to a real editor, so an opaque sentinel is fine.
      return { __test__: 'autocompletion-extension' };
    },
  };
});

function makeDef(
  kind: string,
  options: { categories?: string[]; paramSchema?: unknown } = {}
): NodeDefinition {
  return {
    kind,
    description: null,
    param_schema: options.paramSchema ?? {},
    inputs: [],
    outputs: [],
    categories: options.categories ?? [],
    bidirectional: false,
  };
}

function runSource(doc: string, defs: NodeDefinition[] = []): CompletionResult | null {
  createYamlAutocompletion(defs);
  if (!hoisted.source) throw new Error('autocompletion source was not captured');
  const state = EditorState.create({ doc });
  const ctx = new CompletionContext(state, doc.length, true);
  return hoisted.source(ctx);
}

beforeEach(() => {
  hoisted.source = null;
});

afterEach(() => {
  vi.clearAllMocks();
});

describe('createYamlAutocompletion', () => {
  it('registers an override completion source on construction', () => {
    createYamlAutocompletion([]);
    expect(hoisted.source).toBeInstanceOf(Function);
  });
});

describe('mode: completions', () => {
  it('offers both "dynamic" and "oneshot" when the value is empty', () => {
    const result = runSource('mode: ');
    expect(result).not.toBeNull();
    expect(result!.options.map((o) => o.label)).toEqual(['dynamic', 'oneshot']);
    expect(result!.options.every((o) => o.type === 'constant')).toBe(true);
    expect(result!.options[0].detail).toMatch(/real-time/i);
    expect(result!.options[1].detail).toMatch(/one-shot/i);
  });

  it('filters mode options case-insensitively by what has been typed', () => {
    const result = runSource('mode: ONE');
    expect(result!.options.map((o) => o.label)).toEqual(['oneshot']);
  });

  it('returns null when no mode option matches the typed prefix', () => {
    expect(runSource('mode: zzz')).toBeNull();
  });
});

describe('kind: completions', () => {
  const defs: NodeDefinition[] = [
    makeDef('audio::gain', { categories: ['audio', 'filters'] }),
    makeDef('audio::source', { categories: ['audio'] }),
    makeDef('video::sink'),
  ];

  it('lists every node definition kind when the value is empty', () => {
    const result = runSource('    kind: ', defs);
    expect(result).not.toBeNull();
    expect(result!.options.map((o) => o.label).sort()).toEqual(
      ['audio::gain', 'audio::source', 'video::sink'].sort()
    );
  });

  it('joins categories for the detail string, with a "Node type" fallback when none', () => {
    const result = runSource('    kind: ', defs);
    const byLabel = new Map(result!.options.map((o) => [o.label, o.detail]));
    expect(byLabel.get('audio::gain')).toBe('audio, filters');
    expect(byLabel.get('audio::source')).toBe('audio');
    expect(byLabel.get('video::sink')).toBe('Node type');
  });

  it('filters by case-insensitive substring on the kind', () => {
    const result = runSource('    kind: AUDIO::', defs);
    expect(result!.options.map((o) => o.label).sort()).toEqual(['audio::gain', 'audio::source']);
  });

  it('returns null when no kind matches the typed prefix', () => {
    expect(runSource('    kind: missing', defs)).toBeNull();
  });
});

describe('needs: scalar completions', () => {
  const yaml = [
    'nodes:',
    '  source:',
    '    kind: audio::source',
    '  gain:',
    '    kind: audio::gain',
    '    needs: ',
  ].join('\n');

  it('lists the other nodes in the document', () => {
    const result = runSource(yaml);
    expect(result).not.toBeNull();
    expect(result!.options.map((o) => o.label).sort()).toEqual(['gain', 'source']);
    expect(result!.options.every((o) => o.type === 'variable')).toBe(true);
  });

  it('filters needs by what has been typed', () => {
    const result = runSource(yaml + 'sou');
    expect(result!.options.map((o) => o.label)).toEqual(['source']);
  });

  it('uses a regex fallback over top-level `name:` lines when YAML parsing fails', () => {
    // The leading `{[}` is an unambiguous flow-collection syntax error in YAML
    // 1.1 and 1.2 (mismatched `{` and `[`), so js-yaml is guaranteed to throw
    // and the implementation will fall back to the `/^(\w+):\s*$/gm` regex.
    // The cross-check below fails loudly if a future js-yaml version ever
    // becomes lenient enough to parse this — at which point the test would
    // silently exercise the wrong codepath.
    const broken = ['{[}', 'alpha:', 'beta:', 'needs: '].join('\n');
    expect(() => yamlLoad(broken)).toThrow();

    const result = runSource(broken);
    expect(result).not.toBeNull();
    expect(new Set(result!.options.map((o) => o.label))).toEqual(new Set(['alpha', 'beta']));
  });

  it('returns null when no node names match the prefix', () => {
    expect(runSource(yaml + 'zzz')).toBeNull();
  });
});

describe('needs: array element completions', () => {
  const yaml = [
    'nodes:',
    '  source_a:',
    '    kind: audio::source',
    '  source_b:',
    '    kind: audio::source',
    '  sink:',
    '    kind: audio::sink',
    '    needs:',
    '      - ',
  ].join('\n');

  it('completes node names inside a `needs:` YAML array', () => {
    const result = runSource(yaml);
    expect(result).not.toBeNull();
    expect(result!.options.map((o) => o.label).sort()).toEqual(['sink', 'source_a', 'source_b']);
  });

  it('does not offer needs-array completions for a `- ` outside any needs block', () => {
    const outside = ['steps:', '  - id: a', '    kind: noop', '  - '].join('\n');
    expect(runSource(outside)).toBeNull();
  });

  it('returns null when the array prefix filter excludes everything', () => {
    expect(runSource(yaml + 'zzz')).toBeNull();
  });
});

describe('params: parameter-name completions', () => {
  const defs: NodeDefinition[] = [
    makeDef('audio::gain', {
      paramSchema: {
        properties: {
          db: { type: 'number', default: 0, description: 'Gain in decibels' },
          mute: { type: 'boolean', description: 'Mute the output' },
        },
      },
    }),
  ];

  it('suggests param names with description + default in the detail string', () => {
    const yaml = ['nodes:', '  g:', '    kind: audio::gain', '    params:', '      '].join('\n');
    const result = runSource(yaml, defs);
    expect(result).not.toBeNull();
    const byLabel = new Map(result!.options.map((o) => [o.label, o.detail]));
    expect(byLabel.get('db')).toBe('Gain in decibels (default: 0)');
    expect(byLabel.get('mute')).toBe('Mute the output');
  });

  it('returns null when there is no enclosing params block', () => {
    const yaml = ['nodes:', '  g:', '    kind: audio::gain', '    '].join('\n');
    expect(runSource(yaml, defs)).toBeNull();
  });

  it('returns null when the matching node kind has no param_schema properties', () => {
    const yaml = ['nodes:', '  g:', '    kind: audio::other', '    params:', '      '].join('\n');
    expect(runSource(yaml, [makeDef('audio::other', { paramSchema: {} })])).toBeNull();
  });

  it('returns null when there is no kind: line above the params block', () => {
    const yaml = ['params:', '  '].join('\n');
    expect(runSource(yaml, defs)).toBeNull();
  });

  it('filters param names case-insensitively by the typed prefix', () => {
    const yaml = ['nodes:', '  g:', '    kind: audio::gain', '    params:', '      MU'].join('\n');
    const result = runSource(yaml, defs);
    expect(result).not.toBeNull();
    expect(result!.options.map((o) => o.label)).toEqual(['mute']);
  });
});

describe('params: parameter-value completions', () => {
  it('lists enum members for an enum-typed parameter', () => {
    const defs = [
      makeDef('codec::aac', {
        paramSchema: {
          properties: {
            profile: { type: 'string', enum: ['lc', 'he', 'he-v2'], description: 'AAC profile' },
          },
        },
      }),
    ];
    const yaml = ['nodes:', '  a:', '    kind: codec::aac', '    params:', '      profile: '].join(
      '\n'
    );
    const result = runSource(yaml, defs);
    expect(result).not.toBeNull();
    expect(result!.options.map((o) => o.label)).toEqual(['lc', 'he', 'he-v2']);
    expect(result!.options.every((o) => o.detail === 'AAC profile')).toBe(true);
  });

  it('offers true/false for a boolean-typed parameter', () => {
    const defs = [
      makeDef('audio::gain', { paramSchema: { properties: { mute: { type: 'boolean' } } } }),
    ];
    const yaml = ['nodes:', '  g:', '    kind: audio::gain', '    params:', '      mute: '].join(
      '\n'
    );
    const result = runSource(yaml, defs);
    expect(result).not.toBeNull();
    expect(result!.options.map((o) => o.label)).toEqual(['true', 'false']);
  });

  it('filters boolean completions by typed prefix', () => {
    const defs = [
      makeDef('audio::gain', { paramSchema: { properties: { mute: { type: 'boolean' } } } }),
    ];
    const yaml = ['nodes:', '  g:', '    kind: audio::gain', '    params:', '      mute: tr'].join(
      '\n'
    );
    const result = runSource(yaml, defs);
    expect(result!.options.map((o) => o.label)).toEqual(['true']);
  });

  it('offers a single default value for a number-typed parameter when the value is empty', () => {
    const defs = [
      makeDef('audio::gain', {
        paramSchema: {
          properties: { db: { type: 'number', default: -6, minimum: -60, maximum: 12 } },
        },
      }),
    ];
    const yaml = ['nodes:', '  g:', '    kind: audio::gain', '    params:', '      db: '].join(
      '\n'
    );
    const result = runSource(yaml, defs);
    expect(result).not.toBeNull();
    expect(result!.options.map((o) => o.label)).toEqual(['-6']);
    expect(result!.options[0].detail).toContain('Range: -60 to 12');
  });

  it('returns null for number-typed params with no default', () => {
    const defs = [
      makeDef('audio::gain', { paramSchema: { properties: { db: { type: 'number' } } } }),
    ];
    const yaml = ['nodes:', '  g:', '    kind: audio::gain', '    params:', '      db: '].join(
      '\n'
    );
    expect(runSource(yaml, defs)).toBeNull();
  });

  it('quotes a string default with `label` and provides the unquoted value via `apply`', () => {
    const defs = [
      makeDef('audio::src', {
        paramSchema: { properties: { url: { type: 'string', default: 'http://example.com' } } },
      }),
    ];
    const yaml = ['nodes:', '  s:', '    kind: audio::src', '    params:', '      url: '].join(
      '\n'
    );
    const result = runSource(yaml, defs);
    expect(result).not.toBeNull();
    expect(result!.options).toHaveLength(1);
    expect(result!.options[0].label).toBe('"http://example.com"');
    // CompletionOption's apply is omitted from the basic type — read via index access.
    expect((result!.options[0] as { apply?: string }).apply).toBe('http://example.com');
  });

  it('returns null when the typed param name is not in the schema', () => {
    const defs = [
      makeDef('audio::gain', { paramSchema: { properties: { db: { type: 'number' } } } }),
    ];
    const yaml = ['nodes:', '  g:', '    kind: audio::gain', '    params:', '      unknown: '].join(
      '\n'
    );
    expect(runSource(yaml, defs)).toBeNull();
  });
});

describe('fallthrough behaviour', () => {
  it('returns null on an unrelated line (no completion applicable)', () => {
    expect(runSource('description: hello world')).toBeNull();
  });

  it('returns null when the document is empty', () => {
    expect(runSource('')).toBeNull();
  });
});
