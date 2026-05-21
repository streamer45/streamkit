// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { extractJsonValues } from './jsonStream';

describe('extractJsonValues', () => {
  it('extracts a single complete object and leaves no remainder', () => {
    const { values, remainder } = extractJsonValues('{"a":1}');
    expect(values).toEqual(['{"a":1}']);
    expect(remainder).toBe('');
  });

  it('extracts multiple objects concatenated without a separator (}{)', () => {
    const { values, remainder } = extractJsonValues('{"a":1}{"b":2}{"c":3}');
    expect(values).toEqual(['{"a":1}', '{"b":2}', '{"c":3}']);
    expect(remainder).toBe('');
  });

  it('returns the partial trailing object as remainder', () => {
    const { values, remainder } = extractJsonValues('{"a":1}{"b":2');
    expect(values).toEqual(['{"a":1}']);
    expect(remainder).toBe('{"b":2');
  });

  it('returns the whole buffer as remainder when no complete object is present', () => {
    const { values, remainder } = extractJsonValues('{"a":1');
    expect(values).toEqual([]);
    expect(remainder).toBe('{"a":1');
  });

  it('returns no values and empty remainder for whitespace-only input', () => {
    const { values, remainder } = extractJsonValues('   \n\t\r  ');
    expect(values).toEqual([]);
    expect(remainder).toBe('');
  });

  it('skips whitespace between objects', () => {
    const { values, remainder } = extractJsonValues('  {"a":1}\n\n {"b":2}\t');
    expect(values).toEqual(['{"a":1}', '{"b":2}']);
    expect(remainder).toBe('');
  });

  it('handles nested objects (does not split on inner closing braces)', () => {
    const { values, remainder } = extractJsonValues('{"a":{"b":{"c":1}}}{"d":2}');
    expect(values).toEqual(['{"a":{"b":{"c":1}}}', '{"d":2}']);
    expect(remainder).toBe('');
  });

  it('ignores braces that appear inside JSON strings', () => {
    const { values, remainder } = extractJsonValues('{"msg":"hello { world }"}{"x":1}');
    expect(values).toEqual(['{"msg":"hello { world }"}', '{"x":1}']);
    expect(remainder).toBe('');
  });

  it('honours escaped quotes inside strings', () => {
    const input = '{"q":"she said \\"hi\\" }"}{"next":true}';
    const { values, remainder } = extractJsonValues(input);
    expect(values).toEqual(['{"q":"she said \\"hi\\" }"}', '{"next":true}']);
    expect(remainder).toBe('');
  });

  it('honours escaped backslashes inside strings', () => {
    const input = '{"path":"C:\\\\Users\\\\}"}{"ok":1}';
    const { values, remainder } = extractJsonValues(input);
    expect(values).toEqual(['{"path":"C:\\\\Users\\\\}"}', '{"ok":1}']);
    expect(remainder).toBe('');
  });

  it('extracts top-level arrays', () => {
    const { values, remainder } = extractJsonValues('[1,2,3][{"a":1}]');
    expect(values).toEqual(['[1,2,3]', '[{"a":1}]']);
    expect(remainder).toBe('');
  });

  it('handles arrays with nested objects', () => {
    const { values, remainder } = extractJsonValues('[{"a":[1,2]},{"b":3}]');
    expect(values).toEqual(['[{"a":[1,2]},{"b":3}]']);
    expect(remainder).toBe('');
  });

  it('ignores leading non-structural characters before a value starts', () => {
    // Anything that is not `{`, `[`, or whitespace is dropped while searching
    // for a value's start — this is by design for stream framing.
    const { values, remainder } = extractJsonValues('garbage{"a":1}');
    expect(values).toEqual(['{"a":1}']);
    expect(remainder).toBe('');
  });

  it('returns empty remainder when buffer is empty', () => {
    const { values, remainder } = extractJsonValues('');
    expect(values).toEqual([]);
    expect(remainder).toBe('');
  });

  it('treats a trailing partial after complete values as remainder starting at the next `{`', () => {
    const { values, remainder } = extractJsonValues('{"a":1}prefix{"b":');
    expect(values).toEqual(['{"a":1}']);
    expect(remainder).toBe('{"b":');
  });

  it('treats unbalanced inner brace inside a string as one complete value', () => {
    const { values, remainder } = extractJsonValues('{"s":"only-open-{"}');
    expect(values).toEqual(['{"s":"only-open-{"}']);
    expect(remainder).toBe('');
  });
});
