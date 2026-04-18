// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect } from 'vitest';

import {
  extractSliderConfigs,
  extractToggleConfigs,
  extractTextConfigs,
  deepMergeSchemas,
  schemaToControlConfigs,
} from './jsonSchema';

describe('extractToggleConfigs', () => {
  it('returns boolean + tunable properties', () => {
    const result = extractToggleConfigs({
      properties: {
        show: { type: 'boolean', tunable: true, description: 'Show overlay' },
      },
    });
    expect(result).toEqual([
      {
        key: 'show',
        path: 'show',
        schema: { type: 'boolean', tunable: true, description: 'Show overlay' },
      },
    ]);
  });

  it('excludes boolean properties without tunable', () => {
    const result = extractToggleConfigs({
      properties: {
        enabled: { type: 'boolean' },
      },
    });
    expect(result).toEqual([]);
  });

  it('excludes non-boolean tunable properties', () => {
    const result = extractToggleConfigs({
      properties: {
        gain: { type: 'number', tunable: true },
        name: { type: 'string', tunable: true },
      },
    });
    expect(result).toEqual([]);
  });

  it('uses schema path when provided', () => {
    const result = extractToggleConfigs({
      properties: {
        show: {
          type: 'boolean',
          tunable: true,
          path: 'properties.show',
        },
      },
    });
    expect(result).toHaveLength(1);
    expect(result[0].key).toBe('show');
    expect(result[0].path).toBe('properties.show');
  });

  it('defaults path to key when not specified', () => {
    const result = extractToggleConfigs({
      properties: {
        mute: { type: 'boolean', tunable: true },
      },
    });
    expect(result[0].path).toBe('mute');
  });

  it('returns empty array for undefined schema', () => {
    expect(extractToggleConfigs(undefined)).toEqual([]);
  });

  it('returns empty array for schema without properties', () => {
    expect(extractToggleConfigs({})).toEqual([]);
  });
});

describe('extractTextConfigs', () => {
  it('returns string + tunable properties', () => {
    const result = extractTextConfigs({
      properties: {
        name: { type: 'string', tunable: true, description: 'Player name' },
      },
    });
    expect(result).toEqual([
      {
        key: 'name',
        path: 'name',
        schema: { type: 'string', tunable: true, description: 'Player name' },
      },
    ]);
  });

  it('excludes string properties without tunable', () => {
    const result = extractTextConfigs({
      properties: {
        label: { type: 'string' },
      },
    });
    expect(result).toEqual([]);
  });

  it('excludes non-string tunable properties', () => {
    const result = extractTextConfigs({
      properties: {
        gain: { type: 'number', tunable: true },
        show: { type: 'boolean', tunable: true },
      },
    });
    expect(result).toEqual([]);
  });

  it('uses schema path when provided', () => {
    const result = extractTextConfigs({
      properties: {
        name: {
          type: 'string',
          tunable: true,
          path: 'properties.name',
        },
      },
    });
    expect(result).toHaveLength(1);
    expect(result[0].key).toBe('name');
    expect(result[0].path).toBe('properties.name');
  });

  it('defaults path to key when not specified', () => {
    const result = extractTextConfigs({
      properties: {
        title: { type: 'string', tunable: true },
      },
    });
    expect(result[0].path).toBe('title');
  });

  it('returns empty array for undefined schema', () => {
    expect(extractTextConfigs(undefined)).toEqual([]);
  });

  it('excludes enum-constrained string properties', () => {
    const result = extractTextConfigs({
      properties: {
        mode: {
          type: 'string',
          tunable: true,
          enum: ['fast', 'balanced', 'quality'],
        },
      },
    });
    expect(result).toEqual([]);
  });

  it('includes string properties with empty enum array', () => {
    const result = extractTextConfigs({
      properties: {
        label: { type: 'string', tunable: true, enum: [] },
      },
    });
    expect(result).toHaveLength(1);
    expect(result[0].key).toBe('label');
  });
});

describe('extractSliderConfigs — path field', () => {
  it('defaults path to key when not specified', () => {
    const result = extractSliderConfigs({
      properties: {
        gain_db: {
          type: 'number',
          tunable: true,
          minimum: -60,
          maximum: 12,
        },
      },
    });
    expect(result).toHaveLength(1);
    expect(result[0].key).toBe('gain_db');
    expect(result[0].path).toBe('gain_db');
  });

  it('uses schema path when provided', () => {
    const result = extractSliderConfigs({
      properties: {
        score: {
          type: 'integer',
          tunable: true,
          minimum: 0,
          maximum: 99,
          path: 'properties.score',
        },
      },
    });
    expect(result).toHaveLength(1);
    expect(result[0].key).toBe('score');
    expect(result[0].path).toBe('properties.score');
  });
});

describe('mixed schema extraction', () => {
  const schema = {
    properties: {
      show: { type: 'boolean', tunable: true, default: true },
      name: { type: 'string', tunable: true, default: 'Player' },
      score: {
        type: 'integer',
        tunable: true,
        minimum: 0,
        maximum: 99,
        default: 0,
      },
      codec: { type: 'string', tunable: false },
      internal_flag: { type: 'boolean' },
    },
  };

  it('extracts only boolean tunables as toggles', () => {
    const toggles = extractToggleConfigs(schema);
    expect(toggles).toHaveLength(1);
    expect(toggles[0].key).toBe('show');
  });

  it('extracts only string tunables as text inputs', () => {
    const texts = extractTextConfigs(schema);
    expect(texts).toHaveLength(1);
    expect(texts[0].key).toBe('name');
  });

  it('extracts only numeric tunables with bounds as sliders', () => {
    const sliders = extractSliderConfigs(schema);
    expect(sliders).toHaveLength(1);
    expect(sliders[0].key).toBe('score');
  });
});

// ---------------------------------------------------------------------------
// deepMergeSchemas
// ---------------------------------------------------------------------------

describe('deepMergeSchemas', () => {
  it('returns empty object when both are undefined', () => {
    expect(deepMergeSchemas(undefined, undefined)).toEqual({});
  });

  it('returns base when runtime is undefined', () => {
    const base = { properties: { gain: { type: 'number', tunable: true } } };
    expect(deepMergeSchemas(base, undefined)).toEqual(base);
  });

  it('returns runtime when base is undefined', () => {
    const runtime = { properties: { show: { type: 'boolean', tunable: true } } };
    expect(deepMergeSchemas(undefined, runtime)).toEqual(runtime);
  });

  it('preserves base properties not in runtime', () => {
    const base = {
      properties: {
        fps: { type: 'integer', default: 30 },
        width: { type: 'integer', default: 640 },
      },
    };
    const runtime = {
      properties: {
        show: { type: 'boolean', tunable: true, path: 'properties.show' },
      },
    };
    const merged = deepMergeSchemas(base, runtime);
    expect(merged.properties).toHaveProperty('fps');
    expect(merged.properties).toHaveProperty('width');
    expect(merged.properties).toHaveProperty('show');
  });

  it('runtime properties override base properties with same key', () => {
    const base = {
      properties: {
        show: { type: 'boolean', default: false },
      },
    };
    const runtime = {
      properties: {
        show: { type: 'boolean', tunable: true, path: 'properties.show' },
      },
    };
    const merged = deepMergeSchemas(base, runtime);
    // Runtime fields win, but base-only fields (default) are preserved.
    expect(merged.properties?.show).toEqual({
      type: 'boolean',
      default: false,
      tunable: true,
      path: 'properties.show',
    });
  });

  it('preserves base minimum/maximum when runtime only adds tunable + path', () => {
    const base = {
      properties: {
        score: { type: 'integer', minimum: 0, maximum: 99, default: 0 },
      },
    };
    const runtime = {
      properties: {
        score: { type: 'integer', tunable: true, path: 'properties.score' },
      },
    };
    const merged = deepMergeSchemas(base, runtime);
    expect(merged.properties?.score).toEqual({
      type: 'integer',
      minimum: 0,
      maximum: 99,
      default: 0,
      tunable: true,
      path: 'properties.score',
    });
  });

  it('merged schema works with extractors', () => {
    const base = {
      properties: {
        fps: { type: 'integer', default: 30 },
      },
    };
    const runtime = {
      properties: {
        show: { type: 'boolean', tunable: true, path: 'properties.show' },
        score: {
          type: 'number',
          tunable: true,
          minimum: 0,
          maximum: 99,
          path: 'properties.score',
        },
        name: { type: 'string', tunable: true, path: 'properties.name' },
      },
    };
    const merged = deepMergeSchemas(base, runtime);

    expect(extractToggleConfigs(merged)).toHaveLength(1);
    expect(extractToggleConfigs(merged)[0].path).toBe('properties.show');

    expect(extractSliderConfigs(merged)).toHaveLength(1);
    expect(extractSliderConfigs(merged)[0].path).toBe('properties.score');

    expect(extractTextConfigs(merged)).toHaveLength(1);
    expect(extractTextConfigs(merged)[0].path).toBe('properties.name');
  });
});

describe('schemaToControlConfigs', () => {
  it('converts boolean tunable properties to toggle ControlConfigs', () => {
    const result = schemaToControlConfigs('scoreboard', {
      properties: {
        clock_running: {
          type: 'boolean',
          tunable: true,
          path: 'properties.clock_running',
          default: true,
        },
      },
    });
    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({
      label: 'Clock Running',
      type: 'toggle',
      node: 'scoreboard',
      property: 'properties.clock_running',
      default: true,
    });
  });

  it('converts number tunable properties to number ControlConfigs', () => {
    const result = schemaToControlConfigs('scoreboard', {
      properties: {
        home_score: {
          type: 'number',
          tunable: true,
          path: 'properties.home_score',
          minimum: 0,
          maximum: 99,
          default: 0,
        },
      },
    });
    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({
      label: 'Home Score',
      type: 'number',
      node: 'scoreboard',
      property: 'properties.home_score',
      min: 0,
      max: 99,
      default: 0,
    });
  });

  it('converts string tunable properties to text ControlConfigs', () => {
    const result = schemaToControlConfigs('scoreboard', {
      properties: {
        home_team: {
          type: 'string',
          tunable: true,
          path: 'properties.home_team',
          default: 'HOME',
        },
      },
    });
    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({
      label: 'Home Team',
      type: 'text',
      node: 'scoreboard',
      property: 'properties.home_team',
      default: 'HOME',
    });
  });

  it('skips non-tunable properties', () => {
    const result = schemaToControlConfigs('node', {
      properties: {
        fps: { type: 'number', default: 30 },
        width: { type: 'integer', default: 420 },
      },
    });
    expect(result).toEqual([]);
  });

  it('converts enum-constrained strings to select ControlConfigs', () => {
    const result = schemaToControlConfigs('node', {
      properties: {
        mode: { type: 'string', tunable: true, enum: ['fast', 'slow'] },
      },
    });
    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({
      label: 'Mode',
      type: 'select',
      node: 'node',
      default: 'fast',
      options: [
        { label: 'fast', value: 'fast' },
        { label: 'slow', value: 'slow' },
      ],
    });
  });

  it('assigns group label when provided', () => {
    const result = schemaToControlConfigs(
      'scoreboard',
      {
        properties: {
          show: { type: 'boolean', tunable: true, path: 'properties.show' },
        },
      },
      'Scoreboard'
    );
    expect(result[0].group).toBe('Scoreboard');
  });

  it('derives label from snake_case and kebab-case keys', () => {
    const result = schemaToControlConfigs('node', {
      properties: {
        clock_running: { type: 'boolean', tunable: true },
        'font-size': { type: 'number', tunable: true, minimum: 8, maximum: 72 },
      },
    });
    expect(result.map((c) => c.label)).toEqual(['Clock Running', 'Font Size']);
  });

  it('returns empty array for undefined schema', () => {
    expect(schemaToControlConfigs('node', undefined)).toEqual([]);
  });
});
