// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/** Build a nested object from a dot-notation path (e.g. `"properties.score"` → `{properties:{score: value}}`). */
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

/** Read a value from a nested object using a dot-notation path. */
export function readByPath(obj: Record<string, unknown>, path: string): unknown {
  const parts = path.split('.').filter(Boolean);
  let current: unknown = obj;
  for (const part of parts) {
    if (current == null || typeof current !== 'object') return undefined;
    current = (current as Record<string, unknown>)[part];
  }
  return current;
}

/** Route flat keys to `onFlat`, dot-paths to `onNested` (via `buildParamUpdate`). */
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

/** Recursively deep-merge `source` into `target`; arrays/primitives are replaced wholesale. */
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
