// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Builds a nested `UpdateParams` object from a dot-notation property path.
 *
 * For example:
 * - `buildParamUpdate("properties.home_score", 4)` → `{ properties: { home_score: 4 } }`
 * - `buildParamUpdate("gain_db", 1.5)` → `{ gain_db: 1.5 }`
 *
 * Empty or whitespace-only segments are discarded so that malformed paths
 * like `""`, `"."`, or `"a..b"` degrade gracefully instead of producing
 * keys with empty strings.
 *
 * @throws {Error} if the path resolves to zero valid segments.
 */
export function buildParamUpdate(path: string, value: unknown): Record<string, unknown> {
  const parts = path.split('.').filter(Boolean);
  if (parts.length === 0) {
    throw new Error(
      `buildParamUpdate: path must contain at least one non-empty segment, got "${path}"`
    );
  }
  let result: unknown = value;
  for (let i = parts.length - 1; i > 0; i--) {
    result = { [parts[i]]: result };
  }
  return { [parts[0]]: result };
}
