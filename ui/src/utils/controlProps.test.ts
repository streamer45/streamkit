// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect, vi } from 'vitest';

import { buildParamUpdate, deepMerge, dispatchParamUpdate, readByPath } from './controlProps';

describe('buildParamUpdate', () => {
  it('wraps a single-segment path as a flat key', () => {
    expect(buildParamUpdate('gain_db', 1.5)).toEqual({ gain_db: 1.5 });
  });

  it('nests a two-segment dot path', () => {
    expect(buildParamUpdate('properties.home_score', 4)).toEqual({
      properties: { home_score: 4 },
    });
  });

  it('nests a three-segment dot path', () => {
    expect(buildParamUpdate('a.b.c', true)).toEqual({ a: { b: { c: true } } });
  });

  it('handles string values', () => {
    expect(buildParamUpdate('properties.name', 'Alex')).toEqual({
      properties: { name: 'Alex' },
    });
  });

  it('handles null and undefined values', () => {
    expect(buildParamUpdate('key', null)).toEqual({ key: null });
    expect(buildParamUpdate('key', undefined)).toEqual({ key: undefined });
  });

  it('throws on an empty string path', () => {
    expect(() => buildParamUpdate('', 1)).toThrow(/at least one non-empty segment/);
  });

  it('throws on a dot-only path', () => {
    expect(() => buildParamUpdate('.', 1)).toThrow(/at least one non-empty segment/);
    expect(() => buildParamUpdate('..', 1)).toThrow(/at least one non-empty segment/);
  });

  it('filters empty segments from malformed paths like "a..b"', () => {
    expect(buildParamUpdate('a..b', 42)).toEqual({ a: { b: 42 } });
  });

  it('filters leading/trailing dots', () => {
    expect(buildParamUpdate('.foo.bar.', 'x')).toEqual({ foo: { bar: 'x' } });
  });
});

describe('deepMerge', () => {
  it('merges flat keys without clobbering siblings', () => {
    const target = { a: 1, b: 2 };
    const source = { b: 3, c: 4 };
    expect(deepMerge(target, source)).toEqual({ a: 1, b: 3, c: 4 });
  });

  it('recursively merges nested objects', () => {
    const target = { properties: { home_score: 3, away_score: 1 } };
    const source = { properties: { home_score: 4 } };
    expect(deepMerge(target, source)).toEqual({
      properties: { home_score: 4, away_score: 1 },
    });
  });

  it('preserves sibling nested properties across successive merges', () => {
    const state1 = deepMerge({}, { properties: { home_score: 3 } });
    const state2 = deepMerge(state1, { properties: { away_score: 1 } });
    expect(state2).toEqual({ properties: { home_score: 3, away_score: 1 } });
  });

  it('replaces non-object values wholesale', () => {
    const target = { x: 'old' };
    const source = { x: 'new' };
    expect(deepMerge(target, source)).toEqual({ x: 'new' });
  });

  it('replaces arrays instead of merging them', () => {
    const target = { items: [1, 2, 3] };
    const source = { items: [4, 5] };
    expect(deepMerge(target, source)).toEqual({ items: [4, 5] });
  });

  it('replaces an object with a primitive', () => {
    const target = { nested: { a: 1 } };
    const source = { nested: 42 };
    expect(deepMerge(target, source)).toEqual({ nested: 42 });
  });

  it('replaces a primitive with an object', () => {
    const target = { nested: 42 };
    const source = { nested: { a: 1 } };
    expect(deepMerge(target, source)).toEqual({ nested: { a: 1 } });
  });

  it('does not mutate the target', () => {
    const target = { properties: { score: 1 } };
    const source = { properties: { score: 2 } };
    deepMerge(target, source);
    expect(target).toEqual({ properties: { score: 1 } });
  });
});

describe('readByPath', () => {
  it('reads a flat key', () => {
    expect(readByPath({ gain_db: 1.5 }, 'gain_db')).toBe(1.5);
  });

  it('reads a two-segment nested path', () => {
    expect(readByPath({ properties: { show: true } }, 'properties.show')).toBe(true);
  });

  it('reads a three-segment nested path', () => {
    expect(readByPath({ a: { b: { c: 42 } } }, 'a.b.c')).toBe(42);
  });

  it('returns undefined for missing keys', () => {
    expect(readByPath({}, 'missing')).toBeUndefined();
    expect(readByPath({}, 'a.b.c')).toBeUndefined();
  });

  it('returns undefined when traversing through a non-object', () => {
    expect(readByPath({ a: 'string' }, 'a.b')).toBeUndefined();
    expect(readByPath({ a: null }, 'a.b')).toBeUndefined();
  });

  it('handles various value types', () => {
    expect(readByPath({ key: false }, 'key')).toBe(false);
    expect(readByPath({ key: 0 }, 'key')).toBe(0);
    expect(readByPath({ key: '' }, 'key')).toBe('');
  });

  it('is the inverse of buildParamUpdate for reading back', () => {
    const update = buildParamUpdate('properties.home_score', 4);
    expect(readByPath(update, 'properties.home_score')).toBe(4);
  });
});

describe('dispatchParamUpdate', () => {
  it('routes flat keys through onFlat', () => {
    const onFlat = vi.fn();
    const onNested = vi.fn();
    dispatchParamUpdate('node1', 'gain_db', 1.5, onFlat, onNested);
    expect(onFlat).toHaveBeenCalledWith('node1', 'gain_db', 1.5);
    expect(onNested).not.toHaveBeenCalled();
  });

  it('routes dot-notation paths through onNested with buildParamUpdate result', () => {
    const onFlat = vi.fn();
    const onNested = vi.fn();
    dispatchParamUpdate('node1', 'properties.show', true, onFlat, onNested);
    expect(onNested).toHaveBeenCalledWith('node1', { properties: { show: true } });
    expect(onFlat).not.toHaveBeenCalled();
  });

  it('handles multi-segment dot paths', () => {
    const onNested = vi.fn();
    dispatchParamUpdate('node1', 'a.b.c', 42, vi.fn(), onNested);
    expect(onNested).toHaveBeenCalledWith('node1', { a: { b: { c: 42 } } });
  });
});
