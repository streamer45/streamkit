// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Helpers for "draft" nodes in the Monitor view.
 *
 * A draft is a node the user has dragged onto the canvas but cannot yet
 * be sent to the engine because one or more required schema params (in
 * `param_schema.required`) have no value (no default + nothing entered
 * by the user yet).  Drafts live entirely in the UI; they are promoted
 * to a real `addnode` WebSocket call once all required params are
 * filled.  See `MonitorView.tsx` for the full lifecycle.
 */

import type { NodeDefinition } from '@/types/types';
import { buildParamUpdate, deepMerge } from '@/utils/controlProps';

/** A param value is considered "missing" if it is undefined, null, or
 *  an empty / whitespace-only string.  Numbers and booleans always count
 *  as set (including 0 / false), matching how plugin validators read
 *  required params. */
const isMissingValue = (v: unknown): boolean => {
  if (v === undefined || v === null) return true;
  if (typeof v === 'string' && v.trim() === '') return true;
  return false;
};

/** Return the subset of `param_schema.required` keys whose value in
 *  `params` is missing.  Returns `[]` for kinds with no required params,
 *  no schema, or no node definition. */
export const computeMissingRequired = (
  kind: string,
  params: Record<string, unknown>,
  nodeDefinitions: NodeDefinition[]
): string[] => {
  const def = nodeDefinitions.find((d) => d.kind === kind);
  const schema = def?.param_schema as Record<string, unknown> | undefined;
  const required = schema?.['required'];
  if (!Array.isArray(required)) return [];
  return required.filter((k): k is string => typeof k === 'string' && isMissingValue(params[k]));
};

/** Build the default param object the UI sends on drop: every property
 *  that has an explicit `default` in the schema is filled with that
 *  default; required-but-defaultless properties are left absent so the
 *  caller can detect them via `computeMissingRequired`. */
export const defaultParamsForKind = (
  kind: string,
  nodeDefinitions: NodeDefinition[]
): Record<string, unknown> => {
  const def = nodeDefinitions.find((d) => d.kind === kind);
  const params: Record<string, unknown> = {};
  const schema = def?.param_schema as Record<string, unknown> | undefined;
  const props = schema?.['properties'] as Record<string, Record<string, unknown>> | undefined;
  if (!props) return params;
  for (const [key, propSchema] of Object.entries(props)) {
    if (propSchema && typeof propSchema === 'object' && 'default' in propSchema) {
      const defVal = propSchema['default'];
      if (defVal !== undefined) {
        params[key] = defVal;
      }
    }
  }
  return params;
};

/** Apply a single inspector edit to a draft's `params`.
 *
 *  Flat keys (no dot) replace the corresponding top-level entry.  Dotted
 *  paths (e.g. `"properties.show"`) are converted into a nested partial
 *  via `buildParamUpdate` and deep-merged into the existing params so
 *  sibling fields are preserved — this matches the live-node code path
 *  in `controlProps.dispatchParamUpdate` and ensures drafts produce the
 *  same shape on promotion that a normal `tunenode` would have. */
export const mergeDraftParam = (
  params: Record<string, unknown>,
  key: string,
  value: unknown
): Record<string, unknown> => {
  if (key.includes('.')) {
    return deepMerge(params, buildParamUpdate(key, value));
  }
  return { ...params, [key]: value };
};
