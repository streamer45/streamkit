// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { CompletionContext, autocompletion } from '@codemirror/autocomplete';
import type { CompletionResult } from '@codemirror/autocomplete';
import { load } from 'js-yaml';

import type { NodeDefinition } from '@/types/generated/api-types';

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

interface JsonSchema {
  type?: string;
  properties?: Record<string, JsonSchemaProperty>;
  required?: string[];
  [key: string]: unknown;
}

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

function isInsideNeedsArray(linesBefore: string[]): boolean {
  for (let i = linesBefore.length - 1; i >= 0; i--) {
    const prevLine = linesBefore[i];
    if (/^\s*needs:\s*$/.test(prevLine)) {
      return true;
    }
    if (/^\s*\w+:/.test(prevLine) && !/^\s*needs:/.test(prevLine)) {
      return false;
    }
  }
  return false;
}

function getNeedsArrayCompletions(
  textBeforeCursor: string,
  lineStart: number,
  context: CompletionContext,
  line: { from: number },
  fullText: string
): CompletionResult | null {
  const needsArrayMatch = /^\s*-\s+(.*)$/.exec(textBeforeCursor);
  if (!needsArrayMatch) return null;

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

export function createYamlAutocompletion(nodeDefinitions: NodeDefinition[]) {
  return autocompletion({
    activateOnTyping: true,
    override: [
      (context: CompletionContext): CompletionResult | null => {
        const line = context.state.doc.lineAt(context.pos);
        const lineText = line.text;
        const lineStart = line.from;
        const cursorPosInLine = context.pos - lineStart;

        const textBeforeCursor = lineText.slice(0, cursorPosInLine);

        const fullText = context.state.doc.toString();

        const linesBefore = context.state.doc.sliceString(0, line.from).split('\n');

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
    const matches = yamlText.matchAll(/^(\w+):\s*$/gm);
    const names = new Set<string>();
    for (const match of matches) {
      const name = match[1];
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

function findCurrentNodeKind(linesBefore: string[]): string | null {
  for (let i = linesBefore.length - 1; i >= 0; i--) {
    const line = linesBefore[i];

    const kindMatch = line.match(/^\s*kind:\s+(.+)$/);
    if (kindMatch) {
      return kindMatch[1].trim();
    }

    if (line.match(/^[a-zA-Z0-9_:.-]+:\s*$/)) {
      return null;
    }
  }
  return null;
}

function isInsideParamsBlock(linesBefore: string[]): boolean {
  for (let i = linesBefore.length - 1; i >= 0; i--) {
    const prevLine = linesBefore[i];

    if (/^\s*params:\s*$/.test(prevLine)) {
      return true;
    }

    if (/^[a-zA-Z0-9_:.-]+:/.test(prevLine)) {
      return false;
    }
  }
  return false;
}

function getParamNameCompletions(
  textBeforeCursor: string,
  lineStart: number,
  linesBefore: string[],
  nodeDefinitions: NodeDefinition[]
): CompletionResult | null {
  if (!isInsideParamsBlock(linesBefore)) {
    return null;
  }

  const paramNameMatch = /^\s+([a-zA-Z0-9_]*)$/.exec(textBeforeCursor);
  if (!paramNameMatch) {
    return null;
  }

  const typed = paramNameMatch[1];
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
  if (!schema.properties) {
    return null;
  }

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

function getNumberCompletions(
  paramSchema: JsonSchemaProperty,
  typed: string
): Array<{ label: string; type: string; detail?: string }> {
  const options: Array<{ label: string; type: string; detail?: string }> = [];

  if (typed === '' && paramSchema.default !== undefined) {
    let detail = 'Default value';

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
