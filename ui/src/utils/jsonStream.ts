// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Extracts complete JSON values from a stream buffer.
 *
 * StreamKit oneshot JSON output is commonly newline-delimited, but can also appear as
 * concatenated top-level JSON objects (e.g., `}{`) depending on how the bytes are viewed/copied.
 * This helper frames complete JSON objects/arrays without relying on newlines.
 */
// eslint-disable-next-line max-statements, complexity, sonarjs/cognitive-complexity -- Stream framing is inherently stateful; keeping it in one place is clearer than splitting into many tiny helpers.
export function extractJsonValues(buffer: string): { values: string[]; remainder: string } {
  const values: string[] = [];

  let startIndex: number | null = null;
  let depth = 0;
  let inString = false;
  let escape = false;

  for (let i = 0; i < buffer.length; i++) {
    const ch = buffer[i];

    if (startIndex === null) {
      if (ch === ' ' || ch === '\n' || ch === '\r' || ch === '\t') continue;
      // We only frame objects/arrays (which is what StreamKit packets serialize to).
      if (ch !== '{' && ch !== '[') {
        continue;
      }
      startIndex = i;
      depth = 1;
      inString = false;
      escape = false;
      continue;
    }

    if (inString) {
      if (escape) {
        escape = false;
        continue;
      }
      if (ch === '\\') {
        escape = true;
        continue;
      }
      if (ch === '"') {
        inString = false;
      }
      continue;
    }

    if (ch === '"') {
      inString = true;
      continue;
    }

    if (ch === '{' || ch === '[') {
      depth += 1;
      continue;
    }

    if (ch === '}' || ch === ']') {
      depth -= 1;
      if (depth === 0) {
        const jsonText = buffer.slice(startIndex, i + 1);
        values.push(jsonText);
        startIndex = null;
      }
    }
  }

  if (startIndex === null) {
    return { values, remainder: '' };
  }

  return { values, remainder: buffer.slice(startIndex) };
}
