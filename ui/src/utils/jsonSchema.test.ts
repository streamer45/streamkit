// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect } from 'vitest';

import { extractSliderConfigs, extractToggleConfigs, extractTextConfigs } from './jsonSchema';

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
