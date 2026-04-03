// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect } from 'vitest';

import { buildParamUpdate } from './controlProps';

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
