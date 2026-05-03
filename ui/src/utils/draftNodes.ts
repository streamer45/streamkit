// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { NodeDefinition } from '@/types/types';
import { buildParamUpdate, deepMerge } from '@/utils/controlProps';

const isMissingValue = (v: unknown): boolean => {
  if (v === undefined || v === null) return true;
  if (typeof v === 'string' && v.trim() === '') return true;
  return false;
};

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
