// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect } from 'vitest';

import { arraysEqual } from './arraysEqual';

describe('arraysEqual', () => {
  it('returns true for element-wise equal arrays', () => {
    expect(arraysEqual(['a', 'b'], ['a', 'b'])).toBe(true);
    expect(arraysEqual([], [])).toBe(true);
  });

  it('returns false when lengths differ', () => {
    expect(arraysEqual(['a'], ['a', 'b'])).toBe(false);
  });

  it('returns false when an element differs', () => {
    expect(arraysEqual(['a', 'b'], ['a', 'c'])).toBe(false);
  });

  it('is order-sensitive', () => {
    expect(arraysEqual(['a', 'b'], ['b', 'a'])).toBe(false);
  });
});
