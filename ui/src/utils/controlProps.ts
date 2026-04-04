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

/**
 * Reads a value from a nested object using a dot-notation path.
 *
 * Companion to `buildParamUpdate` — while `buildParamUpdate` *writes* a
 * value into a nested structure, `readByPath` *reads* one back out.
 *
 * For example:
 * - `readByPath({ gain_db: 1.5 }, "gain_db")` → `1.5`
 * - `readByPath({ properties: { show: true } }, "properties.show")` → `true`
 * - `readByPath({}, "missing.key")` → `undefined`
 */
export function readByPath(obj: Record<string, unknown>, path: string): unknown {
  const parts = path.split('.').filter(Boolean);
  let current: unknown = obj;
  for (const part of parts) {
    if (current == null || typeof current !== 'object') return undefined;
    current = (current as Record<string, unknown>)[part];
  }
  return current;
}

/**
 * Dispatches a param update through the correct handler based on whether
 * the param name is a flat key or a dot-notation path.
 *
 * This centralises the `if (name.includes('.'))` branching that otherwise
 * appears in every call-site (MonitorView, usePipeline, etc.).
 *
 * - **Flat keys** (e.g. `"gain_db"`) → `onFlat(nodeId, key, value)`
 * - **Dot-paths** (e.g. `"properties.show"`) → `onNested(nodeId, partialConfig)`
 *   where `partialConfig` is produced by `buildParamUpdate`.
 */
export function dispatchParamUpdate(
  nodeId: string,
  paramName: string,
  value: unknown,
  onFlat: (nodeId: string, key: string, value: unknown) => void,
  onNested: (nodeId: string, config: Record<string, unknown>) => void
): void {
  if (paramName.includes('.')) {
    onNested(nodeId, buildParamUpdate(paramName, value));
  } else {
    onFlat(nodeId, paramName, value);
  }
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

/**
 * Recursively deep-merges `source` into `target`, returning a new object.
 * Only plain objects are merged recursively; arrays and other values are
 * replaced wholesale (matching the semantics of `UpdateParams`).
 */
export function deepMerge(
  target: Record<string, unknown>,
  source: Record<string, unknown>
): Record<string, unknown> {
  const result: Record<string, unknown> = { ...target };
  for (const key of Object.keys(source)) {
    const srcVal = source[key];
    const tgtVal = result[key];
    if (isPlainObject(srcVal) && isPlainObject(tgtVal)) {
      result[key] = deepMerge(tgtVal, srcVal);
    } else {
      result[key] = srcVal;
    }
  }
  return result;
}
