// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect } from 'vitest';

import { buildParamUpdate, deepMerge } from './controlProps';

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
