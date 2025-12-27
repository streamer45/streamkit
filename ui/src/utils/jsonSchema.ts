// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * JSON Schema utilities for working with parameter schemas in nodes.
 * These functions help extract slider configurations from JSON schemas.
 */

export interface JsonSchemaProperty {
  type?: string;
  description?: string;
  default?: unknown;
  minimum?: number;
  maximum?: number;
  exclusiveMinimum?: number;
  exclusiveMaximum?: number;
  multipleOf?: number;
  /**
   * Indicates whether this parameter can be updated while the node is running.
   * If false or undefined, the parameter can only be set at initialization time.
   * If true, the parameter supports live updates via UpdateParams messages.
   */
  tunable?: boolean;
}

export interface JsonSchema {
  properties?: Record<string, JsonSchemaProperty>;
}

export interface SliderConfig {
  key: string;
  schema: JsonSchemaProperty;
  min: number;
  max: number;
  step: number;
  tunable: boolean;
}

/**
 * Type guard to check if a value is a finite number
 */
export const isFiniteNumber = (value: unknown): value is number =>
  typeof value === 'number' && Number.isFinite(value);

/**
 * Resolves the minimum value from a JSON schema property.
 * Checks both `minimum` and `exclusiveMinimum` fields.
 */
export const resolveMinimum = (schema: JsonSchemaProperty): number | undefined => {
  if (isFiniteNumber(schema.minimum)) return schema.minimum;
  if (isFiniteNumber(schema.exclusiveMinimum)) return schema.exclusiveMinimum;
  return undefined;
};

/**
 * Resolves the maximum value from a JSON schema property.
 * Checks both `maximum` and `exclusiveMaximum` fields.
 */
export const resolveMaximum = (schema: JsonSchemaProperty): number | undefined => {
  if (isFiniteNumber(schema.maximum)) return schema.maximum;
  if (isFiniteNumber(schema.exclusiveMaximum)) return schema.exclusiveMaximum;
  return undefined;
};

/**
 * Calculates the number of decimal places from a step value.
 * Handles both regular decimals and scientific notation (e.g., 1e-3).
 */
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

/**
 * Infers an appropriate step value for a numeric slider based on the schema.
 * Priority: multipleOf > integer type > calculated from range.
 */
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

/**
 * Formats a number with a specific number of decimal places and optional sign.
 * Useful for displaying slider values (e.g., +12.5 dB).
 */
export const formatNumber = (value: number, decimals: number, includeSign: boolean): string => {
  const fixed = value.toFixed(decimals);
  if (includeSign && value > 0) {
    return `+${fixed}`;
  }
  return fixed;
};

/**
 * Extracts slider configurations from a JSON schema.
 * Only returns configs for numeric/integer properties with valid min/max bounds.
 */
export const extractSliderConfigs = (schema: JsonSchema | undefined): SliderConfig[] => {
  if (!schema) return [];

  const properties = schema.properties ?? {};

  return Object.entries(properties).reduce((acc, [key, schemaProp]) => {
    if (!schemaProp || (schemaProp.type !== 'number' && schemaProp.type !== 'integer')) {
      return acc;
    }
    // Only include tunable params for slider display on node cards
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
      schema: schemaProp,
      min,
      max,
      step,
      tunable: schemaProp.tunable ?? false, // Default to false if not specified
    });
    return acc;
  }, [] as SliderConfig[]);
};

/**
 * Validates a numeric value against schema constraints.
 */
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

/**
 * Validates a value against a JSON schema property.
 * Returns null if valid, or an error message if invalid.
 */
export const validateValue = (value: unknown, schema: JsonSchemaProperty): string | null => {
  // Check for numeric types
  if (schema.type === 'number' || schema.type === 'integer') {
    if (typeof value !== 'number') {
      return `Expected a number, got ${typeof value}`;
    }
    return validateNumericValue(value, schema);
  }

  // Check for boolean type
  if (schema.type === 'boolean' && typeof value !== 'boolean') {
    return `Expected a boolean, got ${typeof value}`;
  }

  // Check for string type
  if (schema.type === 'string' && typeof value !== 'string') {
    return `Expected a string, got ${typeof value}`;
  }

  return null; // Valid
};
