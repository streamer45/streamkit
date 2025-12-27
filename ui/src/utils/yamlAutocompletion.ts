// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { CompletionContext, autocompletion } from '@codemirror/autocomplete';
import type { CompletionResult } from '@codemirror/autocomplete';
import { load } from 'js-yaml';

import type { NodeDefinition } from '@/types/generated/api-types';

/**
 * JSON Schema property definition
 */
interface JsonSchemaProperty {
  type?: string | string[];
  enum?: unknown[];
  default?: unknown;
  minimum?: number;
  maximum?: number;
  description?: string;
  properties?: Record<string, JsonSchemaProperty>;
  items?: JsonSchemaProperty;
  [key: string]: unknown;
}

/**
 * JSON Schema definition
 */
interface JsonSchema {
  type?: string;
  properties?: Record<string, JsonSchemaProperty>;
  required?: string[];
  [key: string]: unknown;
}

/**
 * Provides mode completions (dynamic/oneshot)
 */
function getModeCompletions(textBeforeCursor: string, lineStart: number): CompletionResult | null {
  const modeMatch = /^\s*mode:\s*(.*)$/.exec(textBeforeCursor);
  if (!modeMatch) return null;

  const typed = modeMatch[1];
  const from = lineStart + textBeforeCursor.lastIndexOf(typed);

  const modeOptions = ['dynamic', 'oneshot']
    .filter((mode) => mode.toLowerCase().includes(typed.toLowerCase()))
    .map((mode) => ({
      label: mode,
      type: 'constant',
      detail: mode === 'dynamic' ? 'Long-running, real-time pipeline' : 'One-shot file transcoding',
    }));

  if (modeOptions.length === 0) return null;

  return {
    from,
    options: modeOptions,
    validFor: /^[\w_-]*$/,
  };
}

/**
 * Provides kind completions (node types)
 */
function getKindCompletions(
  textBeforeCursor: string,
  lineStart: number,
  nodeDefinitions: NodeDefinition[]
): CompletionResult | null {
  const kindMatch = /^\s*kind:\s*(.*)$/.exec(textBeforeCursor);
  if (!kindMatch) return null;

  const typed = kindMatch[1];
  const from = lineStart + textBeforeCursor.lastIndexOf(typed);

  const options = nodeDefinitions
    .filter((def) => def.kind.toLowerCase().includes(typed.toLowerCase()))
    .map((def) => ({
      label: def.kind,
      type: 'constant',
      detail: def.categories.join(', ') || 'Node type',
    }));

  if (options.length === 0) return null;

  return {
    from,
    options,
    validFor: /^[\w:_-]*$/,
  };
}

/**
 * Provides needs field completions (node names)
 */
function getNeedsFieldCompletions(
  textBeforeCursor: string,
  lineStart: number,
  fullText: string
): CompletionResult | null {
  const needsMatch = /^\s*needs:\s*(.*)$/.exec(textBeforeCursor);
  if (!needsMatch) return null;

  const typed = needsMatch[1];
  const from = lineStart + textBeforeCursor.lastIndexOf(typed);

  const nodeNames = extractNodeNames(fullText);

  const options = nodeNames
    .filter((name) => name.toLowerCase().includes(typed.toLowerCase()))
    .map((name) => ({
      label: name,
      type: 'variable',
      detail: 'Node name',
    }));

  if (options.length === 0) return null;

  return {
    from,
    options,
    validFor: /^[\w_-]*$/,
  };
}

/**
 * Checks if we're inside a needs array by looking at previous lines
 */
function isInsideNeedsArray(linesBefore: string[]): boolean {
  for (let i = linesBefore.length - 1; i >= 0; i--) {
    const prevLine = linesBefore[i];
    if (/^\s*needs:\s*$/.test(prevLine)) {
      return true;
    }
    // If we hit a non-indented line or another key, we're not in needs array
    if (/^\s*\w+:/.test(prevLine) && !/^\s*needs:/.test(prevLine)) {
      return false;
    }
  }
  return false;
}

/**
 * Provides needs array item completions (node names in array)
 */
function getNeedsArrayCompletions(
  textBeforeCursor: string,
  lineStart: number,
  context: CompletionContext,
  line: { from: number },
  fullText: string
): CompletionResult | null {
  const needsArrayMatch = /^\s*-\s+(.*)$/.exec(textBeforeCursor);
  if (!needsArrayMatch) return null;

  // Check if we're inside a needs array by looking at previous lines
  const linesBefore = context.state.doc.sliceString(0, line.from).split('\n');
  if (!isInsideNeedsArray(linesBefore)) return null;

  const typed = needsArrayMatch[1];
  const from = lineStart + textBeforeCursor.lastIndexOf(typed);

  const nodeNames = extractNodeNames(fullText);

  const options = nodeNames
    .filter((name) => name.toLowerCase().includes(typed.toLowerCase()))
    .map((name) => ({
      label: name,
      type: 'variable',
      detail: 'Node name',
    }));

  if (options.length === 0) return null;

  return {
    from,
    options,
    validFor: /^[\w_-]*$/,
  };
}

/**
 * Creates a YAML autocompletion extension for StreamKit pipelines.
 * Provides completions for:
 * - `mode:` fields with pipeline execution modes (dynamic/oneshot)
 * - `kind:` fields with available node types
 * - `needs:` fields with node names from the current document
 * - `params:` parameter names based on node schema
 * - Parameter values based on JSON Schema types (enums, booleans, etc.)
 */
export function createYamlAutocompletion(nodeDefinitions: NodeDefinition[]) {
  return autocompletion({
    activateOnTyping: true,
    override: [
      (context: CompletionContext): CompletionResult | null => {
        const line = context.state.doc.lineAt(context.pos);
        const lineText = line.text;
        const lineStart = line.from;
        const cursorPosInLine = context.pos - lineStart;

        // Get text before cursor on current line
        const textBeforeCursor = lineText.slice(0, cursorPosInLine);

        // Get the full document text to parse node names
        const fullText = context.state.doc.toString();

        // Get lines before cursor for context analysis
        const linesBefore = context.state.doc.sliceString(0, line.from).split('\n');

        // Try each completion type in order
        // Note: Order matters! More specific completions should come first
        return (
          getModeCompletions(textBeforeCursor, lineStart) ||
          getKindCompletions(textBeforeCursor, lineStart, nodeDefinitions) ||
          getNeedsFieldCompletions(textBeforeCursor, lineStart, fullText) ||
          getNeedsArrayCompletions(textBeforeCursor, lineStart, context, line, fullText) ||
          getParamValueCompletions(textBeforeCursor, lineStart, linesBefore, nodeDefinitions) ||
          getParamNameCompletions(textBeforeCursor, lineStart, linesBefore, nodeDefinitions)
        );
      },
    ],
  });
}

/**
 * Extracts node names from YAML document
 */
function extractNodeNames(yamlText: string): string[] {
  try {
    const parsed = load(yamlText) as {
      nodes?: Record<string, unknown>;
      steps?: Array<unknown>;
    };

    if (!parsed) return [];

    if (parsed.nodes && typeof parsed.nodes === 'object') {
      return Object.keys(parsed.nodes);
    }

    if (parsed.steps && Array.isArray(parsed.steps)) {
      // For steps format, generate step_0, step_1, etc.
      return parsed.steps.map((_, i) => `step_${i}`);
    }

    return [];
  } catch {
    // If YAML is invalid, try to extract node names with regex
    const matches = yamlText.matchAll(/^(\w+):\s*$/gm);
    const names = new Set<string>();
    for (const match of matches) {
      const name = match[1];
      // Filter out common YAML keys
      if (
        ![
          'mode',
          'nodes',
          'steps',
          'kind',
          'params',
          'needs',
          'ui',
          'position',
          'description',
          'name',
        ].includes(name)
      ) {
        names.add(name);
      }
    }
    return Array.from(names);
  }
}

/**
 * Finds the node kind for the current cursor position by looking backwards for 'kind:' field
 */
function findCurrentNodeKind(linesBefore: string[]): string | null {
  for (let i = linesBefore.length - 1; i >= 0; i--) {
    const line = linesBefore[i];

    // Check if we found a kind: line
    const kindMatch = line.match(/^\s*kind:\s+(.+)$/);
    if (kindMatch) {
      return kindMatch[1].trim();
    }

    // If we hit a top-level key (node name), stop searching
    if (line.match(/^[a-zA-Z0-9_:.-]+:\s*$/)) {
      return null;
    }
  }
  return null;
}

/**
 * Checks if we're inside a params block by looking at previous lines
 */
function isInsideParamsBlock(linesBefore: string[]): boolean {
  for (let i = linesBefore.length - 1; i >= 0; i--) {
    const prevLine = linesBefore[i];

    // Found params: - we're inside
    if (/^\s*params:\s*$/.test(prevLine)) {
      return true;
    }

    // If we hit a non-indented line or another top-level key, we're not in params
    if (/^[a-zA-Z0-9_:.-]+:/.test(prevLine)) {
      return false;
    }
  }
  return false;
}

/**
 * Provides parameter name completions for params: blocks
 */
function getParamNameCompletions(
  textBeforeCursor: string,
  lineStart: number,
  linesBefore: string[],
  nodeDefinitions: NodeDefinition[]
): CompletionResult | null {
  // Check if we're inside a params block
  if (!isInsideParamsBlock(linesBefore)) {
    return null;
  }

  // Check if we're typing a param name (line starts with whitespace and word characters)
  const paramNameMatch = /^\s+([a-zA-Z0-9_]*)$/.exec(textBeforeCursor);
  if (!paramNameMatch) {
    return null;
  }

  const typed = paramNameMatch[1];
  const from = lineStart + textBeforeCursor.lastIndexOf(typed);

  // Find the node kind to get its schema
  const nodeKind = findCurrentNodeKind(linesBefore);
  if (!nodeKind) {
    return null;
  }

  // Find the node definition
  const nodeDef = nodeDefinitions.find((def) => def.kind === nodeKind);
  if (!nodeDef || !nodeDef.param_schema) {
    return null;
  }

  // Parse the schema
  const schema = nodeDef.param_schema as JsonSchema;
  if (!schema.properties) {
    return null;
  }

  // Get parameter names from schema
  const options = Object.entries(schema.properties)
    .filter(([name]) => name.toLowerCase().includes(typed.toLowerCase()))
    .map(([name, prop]) => {
      const detail = prop.description || `${prop.type || 'any'}`;
      const defaultValue =
        prop.default !== undefined ? ` (default: ${JSON.stringify(prop.default)})` : '';

      return {
        label: name,
        type: 'property',
        detail: detail + defaultValue,
        info: prop.description,
      };
    });

  if (options.length === 0) {
    return null;
  }

  return {
    from,
    options,
    validFor: /^[a-zA-Z0-9_]*$/,
  };
}

/**
 * Gets enum value completions
 */
function getEnumCompletions(
  paramSchema: JsonSchemaProperty,
  typed: string
): Array<{ label: string; type: string; detail?: string }> {
  if (!paramSchema.enum || !Array.isArray(paramSchema.enum)) {
    return [];
  }

  return paramSchema.enum
    .filter((value) => String(value).toLowerCase().includes(typed.toLowerCase()))
    .map((value) => ({
      label: String(value),
      type: 'constant',
      detail: paramSchema.description || 'Enum value',
    }));
}

/**
 * Gets boolean value completions
 */
function getBooleanCompletions(
  paramSchema: JsonSchemaProperty,
  typed: string
): Array<{ label: string; type: string; detail?: string }> {
  return ['true', 'false']
    .filter((value) => value.includes(typed.toLowerCase()))
    .map((value) => ({
      label: value,
      type: 'constant',
      detail: paramSchema.description || 'Boolean value',
    }));
}

/**
 * Gets number/integer value completions
 */
function getNumberCompletions(
  paramSchema: JsonSchemaProperty,
  typed: string
): Array<{ label: string; type: string; detail?: string }> {
  const options: Array<{ label: string; type: string; detail?: string }> = [];

  if (typed === '' && paramSchema.default !== undefined) {
    let detail = 'Default value';

    // Add range hint if constraints exist
    if (paramSchema.minimum !== undefined || paramSchema.maximum !== undefined) {
      const min = paramSchema.minimum !== undefined ? String(paramSchema.minimum) : '-∞';
      const max = paramSchema.maximum !== undefined ? String(paramSchema.maximum) : '∞';
      detail = `${detail} (Range: ${min} to ${max})`;
    }

    options.push({
      label: String(paramSchema.default),
      type: 'constant',
      detail,
    });
  }

  return options;
}

/**
 * Gets string value completions
 */
function getStringCompletions(
  paramSchema: JsonSchemaProperty,
  typed: string
): Array<{ label: string; type: string; detail?: string; apply?: string }> {
  if (typed === '' && paramSchema.default !== undefined) {
    return [
      {
        label: `"${paramSchema.default}"`,
        type: 'constant',
        detail: 'Default value',
        apply: String(paramSchema.default),
      },
    ];
  }
  return [];
}

/**
 * Provides parameter value completions based on JSON Schema type
 */
function getParamValueCompletions(
  textBeforeCursor: string,
  lineStart: number,
  linesBefore: string[],
  nodeDefinitions: NodeDefinition[]
): CompletionResult | null {
  if (!isInsideParamsBlock(linesBefore)) {
    return null;
  }

  const paramMatch = /^\s*([a-zA-Z0-9_]+):\s*(.*)$/.exec(textBeforeCursor);
  if (!paramMatch) {
    return null;
  }

  const paramName = paramMatch[1];
  const typed = paramMatch[2];
  const from = lineStart + textBeforeCursor.lastIndexOf(typed);

  const nodeKind = findCurrentNodeKind(linesBefore);
  if (!nodeKind) {
    return null;
  }

  const nodeDef = nodeDefinitions.find((def) => def.kind === nodeKind);
  if (!nodeDef || !nodeDef.param_schema) {
    return null;
  }

  const schema = nodeDef.param_schema as JsonSchema;
  if (!schema.properties || !schema.properties[paramName]) {
    return null;
  }

  const paramSchema = schema.properties[paramName];
  let options: Array<{ label: string; type: string; detail?: string; apply?: string }> = [];

  // Determine which completion type to use
  if (paramSchema.enum && Array.isArray(paramSchema.enum)) {
    options = getEnumCompletions(paramSchema, typed);
  } else if (paramSchema.type === 'boolean') {
    options = getBooleanCompletions(paramSchema, typed);
  } else if (paramSchema.type === 'number' || paramSchema.type === 'integer') {
    options = getNumberCompletions(paramSchema, typed);
  } else if (paramSchema.type === 'string') {
    options = getStringCompletions(paramSchema, typed);
  }

  if (options.length === 0) {
    return null;
  }

  return {
    from,
    options,
    validFor: /^[a-zA-Z0-9_.\-"]*$/,
  };
}
