// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Builds a nested `UpdateParams` object from a dot-notation property path.
 *
 * For example:
 * - `buildParamUpdate("properties.home_score", 4)` → `{ properties: { home_score: 4 } }`
 * - `buildParamUpdate("gain_db", 1.5)` → `{ gain_db: 1.5 }`
 */
export function buildParamUpdate(path: string, value: unknown): Record<string, unknown> {
  const parts = path.split('.');
  let result: unknown = value;
  for (let i = parts.length - 1; i > 0; i--) {
    result = { [parts[i]]: result };
  }
  return { [parts[0]]: result };
}
