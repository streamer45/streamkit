// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { deepEqual } from './deepEqual';

describe('deepEqual primitives', () => {
  it.each([
    [1, 1, true],
    [1, 2, false],
    ['a', 'a', true],
    ['a', 'b', false],
    [true, true, true],
    [false, true, false],
    [null, null, true],
    [undefined, undefined, true],
    [0, 0, true],
    // Object.is distinguishes +0 and -0.
    [-0, 0, false],
  ] as const)('deepEqual(%p, %p) === %p', (a, b, expected) => {
    expect(deepEqual(a, b)).toBe(expected);
  });

  it('treats NaN as equal to itself (Object.is semantics)', () => {
    expect(deepEqual(NaN, NaN)).toBe(true);
  });

  it('returns false when only one side is null', () => {
    expect(deepEqual(null, {})).toBe(false);
    expect(deepEqual({}, null)).toBe(false);
    expect(deepEqual(null, 0)).toBe(false);
  });

  it('returns false when only one side is undefined', () => {
    expect(deepEqual(undefined, null)).toBe(false);
    expect(deepEqual({}, undefined)).toBe(false);
  });

  it('returns false when types differ', () => {
    expect(deepEqual(1, '1')).toBe(false);
    expect(deepEqual(0, false)).toBe(false);
    expect(deepEqual('', null)).toBe(false);
  });
});

describe('deepEqual arrays', () => {
  it('returns true for empty arrays', () => {
    expect(deepEqual([], [])).toBe(true);
  });

  it('returns true for arrays with identical primitive contents', () => {
    expect(deepEqual([1, 2, 3], [1, 2, 3])).toBe(true);
  });

  it('returns false when array lengths differ', () => {
    expect(deepEqual([1, 2], [1, 2, 3])).toBe(false);
  });

  it('returns false when element order differs', () => {
    expect(deepEqual([1, 2, 3], [3, 2, 1])).toBe(false);
  });

  it('returns false when comparing an array to a non-array', () => {
    expect(deepEqual([1, 2, 3], { 0: 1, 1: 2, 2: 3, length: 3 })).toBe(false);
    expect(deepEqual({ 0: 1 }, [1])).toBe(false);
  });

  it('recurses into nested arrays', () => {
    expect(deepEqual([[1, 2], [3]], [[1, 2], [3]])).toBe(true);
    expect(deepEqual([[1, 2], [3]], [[1, 2], [4]])).toBe(false);
  });

  it('treats holes (sparse arrays) as undefined elements with same length', () => {
    const a: (number | undefined)[] = new Array(3);
    a[0] = 1;
    a[2] = 3;
    const b: (number | undefined)[] = [1, undefined, 3];
    expect(deepEqual(a, b)).toBe(true);
  });
});

describe('deepEqual plain objects', () => {
  it('returns true for empty plain objects', () => {
    expect(deepEqual({}, {})).toBe(true);
  });

  it('returns true regardless of key insertion order', () => {
    expect(deepEqual({ a: 1, b: 2 }, { b: 2, a: 1 })).toBe(true);
  });

  it('returns false when key counts differ', () => {
    expect(deepEqual({ a: 1 }, { a: 1, b: 2 })).toBe(false);
  });

  it('returns false when key names differ even with same count', () => {
    expect(deepEqual({ a: 1, b: 2 }, { a: 1, c: 2 })).toBe(false);
  });

  it('recurses into nested plain objects', () => {
    expect(deepEqual({ a: { b: { c: 1 } } }, { a: { b: { c: 1 } } })).toBe(true);
    expect(deepEqual({ a: { b: { c: 1 } } }, { a: { b: { c: 2 } } })).toBe(false);
  });

  it('supports null-prototype objects (Object.create(null))', () => {
    const a = Object.create(null) as Record<string, number>;
    a.x = 1;
    const b = Object.create(null) as Record<string, number>;
    b.x = 1;
    expect(deepEqual(a, b)).toBe(true);
  });

  it('treats {a: undefined} as different from {}', () => {
    expect(deepEqual({ a: undefined }, {})).toBe(false);
  });

  it('compares deeply mixed structures of arrays and objects', () => {
    const a = { list: [{ k: 1 }, { k: 2 }], meta: { tags: ['x', 'y'] } };
    const b = { list: [{ k: 1 }, { k: 2 }], meta: { tags: ['x', 'y'] } };
    expect(deepEqual(a, b)).toBe(true);

    const c = { list: [{ k: 1 }, { k: 3 }], meta: { tags: ['x', 'y'] } };
    expect(deepEqual(a, c)).toBe(false);
  });
});

describe('deepEqual non-plain objects', () => {
  it('returns true for the same function reference (Object.is)', () => {
    const fn = () => 1;
    expect(deepEqual(fn, fn)).toBe(true);
  });

  it('returns false for different function references even with identical bodies', () => {
    expect(
      deepEqual(
        () => 1,
        () => 1
      )
    ).toBe(false);
  });

  it('returns false for two equal Map instances (not plain objects)', () => {
    expect(deepEqual(new Map([['a', 1]]), new Map([['a', 1]]))).toBe(false);
  });

  it('returns false for two equal Date instances (not plain objects)', () => {
    expect(deepEqual(new Date(0), new Date(0))).toBe(false);
  });

  it('returns false when comparing a plain object to a class instance', () => {
    class Foo {
      x = 1;
    }
    expect(deepEqual({ x: 1 }, new Foo())).toBe(false);
  });
});
