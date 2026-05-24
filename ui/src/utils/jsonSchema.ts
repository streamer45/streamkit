// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { ControlConfig } from '@/types/types';

export interface JsonSchemaProperty {
  type?: string;
  description?: string;
  default?: unknown;
  minimum?: number;
  maximum?: number;
  exclusiveMinimum?: number;
  exclusiveMaximum?: number;
  multipleOf?: number;
  /** Whether this param supports live updates via UpdateParams. */
  tunable?: boolean;
  /** Override the UpdateParams key path (dot-notation). Defaults to the property key. */
  path?: string;
  enum?: unknown[];
}

export interface JsonSchema {
  properties?: Record<string, JsonSchemaProperty>;
}

export interface SliderConfig {
  key: string;
  /** Dot-notation path for UpdateParams. Defaults to `key`. */
  path: string;
  schema: JsonSchemaProperty;
  min: number;
  max: number;
  step: number;
  tunable: boolean;
}

export const isFiniteNumber = (value: unknown): value is number =>
  typeof value === 'number' && Number.isFinite(value);

export const resolveMinimum = (schema: JsonSchemaProperty): number | undefined => {
  if (isFiniteNumber(schema.minimum)) return schema.minimum;
  if (isFiniteNumber(schema.exclusiveMinimum)) return schema.exclusiveMinimum;
  return undefined;
};

export const resolveMaximum = (schema: JsonSchemaProperty): number | undefined => {
  if (isFiniteNumber(schema.maximum)) return schema.maximum;
  if (isFiniteNumber(schema.exclusiveMaximum)) return schema.exclusiveMaximum;
  return undefined;
};

export const decimalPlacesFromStep = (step: number): number => {
  if (!Number.isFinite(step) || step <= 0) {
    return 0;
  }
  const stepStr = step.toString();
  if (stepStr.includes('e-')) {
    const parts = stepStr.split('e-');
    const exponent = parseInt(parts[1] ?? '0', 10);
    return Math.max(0, exponent);
  }
  if (stepStr.includes('.')) {
    return stepStr.length - stepStr.indexOf('.') - 1;
  }
  return 0;
};

export const inferStep = (schema: JsonSchemaProperty, min: number, max: number): number => {
  if (isFiniteNumber(schema.multipleOf) && schema.multipleOf > 0) {
    return schema.multipleOf;
  }
  if (schema.type === 'integer') {
    return 1;
  }
  const range = max - min;
  if (!Number.isFinite(range) || range <= 0) {
    return 0.1;
  }
  const rough = range / 100;
  const step = Number.isFinite(rough) && rough > 0 ? rough : 0.1;
  const decimals = decimalPlacesFromStep(step);
  const rounded = Number(step.toFixed(Math.min(decimals + 1, 4)));
  return rounded > 0 ? rounded : 0.1;
};

export const formatNumber = (value: number, decimals: number, includeSign: boolean): string => {
  const fixed = value.toFixed(decimals);
  if (includeSign && value > 0) {
    return `+${fixed}`;
  }
  return fixed;
};

export const extractSliderConfigs = (schema: JsonSchema | undefined): SliderConfig[] => {
  if (!schema) return [];

  const properties = schema.properties ?? {};

  return Object.entries(properties).reduce((acc, [key, schemaProp]) => {
    if (!schemaProp || (schemaProp.type !== 'number' && schemaProp.type !== 'integer')) {
      return acc;
    }
    if (!schemaProp.tunable) {
      return acc;
    }
    const min = resolveMinimum(schemaProp);
    const max = resolveMaximum(schemaProp);
    if (!isFiniteNumber(min) || !isFiniteNumber(max) || max <= min) {
      return acc;
    }
    const step = inferStep(schemaProp, min, max);
    acc.push({
      key,
      path: schemaProp.path ?? key,
      schema: schemaProp,
      min,
      max,
      step,
      tunable: schemaProp.tunable ?? false,
    });
    return acc;
  }, [] as SliderConfig[]);
};

const validateNumericValue = (value: number, schema: JsonSchemaProperty): string | null => {
  if (!isFiniteNumber(value)) {
    return 'Value must be a finite number';
  }

  if (schema.type === 'integer' && !Number.isInteger(value)) {
    return 'Value must be an integer';
  }

  const min = resolveMinimum(schema);
  const max = resolveMaximum(schema);

  if (isFiniteNumber(min) && value < min) {
    return `Value must be at least ${min}, got ${value}`;
  }

  if (isFiniteNumber(max) && value > max) {
    return `Value must be at most ${max}, got ${value}`;
  }

  if (isFiniteNumber(schema.multipleOf) && value % schema.multipleOf !== 0) {
    return `Value must be a multiple of ${schema.multipleOf}`;
  }

  return null;
};

export const validateValue = (value: unknown, schema: JsonSchemaProperty): string | null => {
  if (schema.type === 'number' || schema.type === 'integer') {
    if (typeof value !== 'number') {
      return `Expected a number, got ${typeof value}`;
    }
    return validateNumericValue(value, schema);
  }

  if (schema.type === 'boolean' && typeof value !== 'boolean') {
    return `Expected a boolean, got ${typeof value}`;
  }

  if (schema.type === 'string' && typeof value !== 'string') {
    return `Expected a string, got ${typeof value}`;
  }

  return null;
};

// Schema merging — runtime enrichment

/** Deep-merge a runtime param schema into a base (static) schema, preserving base-only fields. */
export const deepMergeSchemas = (
  base: JsonSchema | undefined,
  runtime: JsonSchema | undefined
): JsonSchema => {
  if (!runtime) return base ?? {};
  if (!base) return runtime;

  const baseProps = base.properties ?? {};
  const runtimeProps = runtime.properties ?? {};

  const mergedProps: Record<string, JsonSchemaProperty> = { ...baseProps };
  for (const [key, runtimeEntry] of Object.entries(runtimeProps)) {
    const baseEntry = baseProps[key];
    mergedProps[key] = baseEntry ? { ...baseEntry, ...runtimeEntry } : runtimeEntry;
  }

  return {
    ...base,
    properties: mergedProps,
  };
};

// Toggle (boolean) config extraction

export interface ToggleConfig {
  key: string;
  /** Dot-notation path for UpdateParams. Defaults to `key`. */
  path: string;
  schema: JsonSchemaProperty;
}

export const extractToggleConfigs = (schema: JsonSchema | undefined): ToggleConfig[] => {
  if (!schema) return [];

  const properties = schema.properties ?? {};

  return Object.entries(properties).reduce((acc, [key, schemaProp]) => {
    if (!schemaProp || schemaProp.type !== 'boolean' || !schemaProp.tunable) {
      return acc;
    }
    acc.push({
      key,
      path: schemaProp.path ?? key,
      schema: schemaProp,
    });
    return acc;
  }, [] as ToggleConfig[]);
};

// Text (string) config extraction

export interface TextConfig {
  key: string;
  /** Dot-notation path for UpdateParams. Defaults to `key`. */
  path: string;
  schema: JsonSchemaProperty;
}

export const extractTextConfigs = (schema: JsonSchema | undefined): TextConfig[] => {
  if (!schema) return [];

  const properties = schema.properties ?? {};

  return Object.entries(properties).reduce((acc, [key, schemaProp]) => {
    if (
      !schemaProp ||
      schemaProp.type !== 'string' ||
      !schemaProp.tunable ||
      (schemaProp.enum && schemaProp.enum.length > 0)
    ) {
      return acc;
    }
    acc.push({
      key,
      path: schemaProp.path ?? key,
      schema: schemaProp,
    });
    return acc;
  }, [] as TextConfig[]);
};

// Schema → ControlConfig conversion

/** Derive a human-readable label: "clock_running" → "Clock Running". */
function labelFromKey(key: string): string {
  return key.replace(/[_-]/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase());
}

/** Map a single tunable schema property to a ControlConfig, or null. */
function propToControlConfig(
  nodeId: string,
  key: string,
  prop: JsonSchemaProperty,
  group: string | null
): ControlConfig | null {
  const path = prop.path ?? key;
  const label = labelFromKey(key);
  const base = { label, node: nodeId, property: path, group, value: null, options: null };

  switch (prop.type) {
    case 'boolean':
      return {
        ...base,
        type: 'toggle',
        default: prop.default ?? false,
        min: null,
        max: null,
        step: null,
      };
    case 'number':
    case 'integer': {
      const min = resolveMinimum(prop) ?? 0;
      const max = resolveMaximum(prop) ?? 100;
      return {
        ...base,
        type: 'number',
        default: prop.default ?? min,
        min,
        max,
        step: inferStep(prop, min, max),
      };
    }
    case 'string':
      if (prop.enum && prop.enum.length > 0) {
        return {
          ...base,
          type: 'select',
          default: prop.default ?? prop.enum[0],
          min: null,
          max: null,
          step: null,
          options: prop.enum.map((v) => ({ label: String(v), value: v })),
        };
      }
      return {
        ...base,
        type: 'text',
        default: prop.default ?? '',
        min: null,
        max: null,
        step: null,
      };
    default:
      return null;
  }
}

/** Convert tunable schema properties into ControlConfig entries for OverlayControls. */
export function schemaToControlConfigs(
  nodeId: string,
  schema: JsonSchema | undefined,
  group?: string
): ControlConfig[] {
  if (!schema?.properties) return [];

  const groupLabel = group ?? null;
  const controls: ControlConfig[] = [];

  for (const [key, prop] of Object.entries(schema.properties)) {
    if (!prop?.tunable) continue;
    const ctrl = propToControlConfig(nodeId, key, prop, groupLabel);
    if (ctrl) controls.push(ctrl);
  }

  return controls;
}
