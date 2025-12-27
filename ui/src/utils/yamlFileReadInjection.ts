// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { utilsLogger } from '@/utils/logger';

/**
 * Replaces http_input node with file_reader in steps: format (linear pipelines)
 */
function replaceHttpInputInSteps(
  lines: string[],
  assetPath: string
): { modified: boolean; result: string[] } {
  const result: string[] = [];
  let foundHttpInput = false;

  for (const line of lines) {
    const trimmed = line.trim();

    if (trimmed.startsWith('- kind: streamkit::http_input')) {
      foundHttpInput = true;
      const match = line.match(/^(\s+)/);
      const indent = match ? match[1] : '  ';

      utilsLogger.debug(
        '[yamlPipeline] Found streamkit::http_input in steps, replacing with core::file_reader'
      );

      result.push(`${indent}- kind: core::file_reader`);
      result.push(`${indent}  params:`);
      result.push(`${indent}    path: "${assetPath}"`);
      result.push(`${indent}    chunk_size: 8192`);
      continue;
    }

    result.push(line);
  }

  return { modified: foundHttpInput, result };
}

/**
 * Checks if a line marks the start of a new node in nodes: format
 */
function isNewNodeStart(trimmedLine: string): boolean {
  const nodeNameMatch = trimmedLine.match(/^[a-zA-Z0-9_]+:/);
  return Boolean(
    nodeNameMatch &&
    !trimmedLine.includes('kind:') &&
    !trimmedLine.includes('params:') &&
    !trimmedLine.includes('needs:')
  );
}

/**
 * Checks if a line should be skipped as part of old http_input params
 */
function shouldSkipOldParams(line: string, trimmedLine: string): boolean {
  // Skip lines that are part of the old http_input params
  if (trimmedLine.startsWith('params:') || line.match(/^\s{4,}/)) {
    return true;
  }
  return false;
}

/**
 * Replaces http_input node with file_reader in nodes: format (DAG pipelines)
 */
function replaceHttpInputInNodes(
  lines: string[],
  assetPath: string
): { modified: boolean; result: string[] } {
  const result: string[] = [];
  let foundHttpInput = false;
  let skipUntilNextNode = false;

  for (const line of lines) {
    const trimmed = line.trim();

    // Look for node definition with streamkit::http_input
    if (trimmed.startsWith('kind: streamkit::http_input')) {
      foundHttpInput = true;
      skipUntilNextNode = true;
      const match = line.match(/^(\s+)/);
      const indent = match ? match[1] : '    ';

      utilsLogger.debug(
        '[yamlPipeline] Found streamkit::http_input in nodes, replacing with core::file_reader'
      );

      result.push(`${indent}kind: core::file_reader`);
      result.push(`${indent}params:`);
      result.push(`${indent}  path: "${assetPath}"`);
      result.push(`${indent}  chunk_size: 8192`);
      continue;
    }

    // Skip params section of the old http_input node
    if (skipUntilNextNode) {
      // Check if we're at the start of a new node
      if (isNewNodeStart(trimmed)) {
        skipUntilNextNode = false;
        result.push(line);
        continue;
      }
      // Skip lines that are part of the old http_input params
      if (shouldSkipOldParams(line, trimmed)) {
        continue;
      }
      // If we hit the next property (like 'needs:'), stop skipping
      skipUntilNextNode = false;
    }

    result.push(line);
  }

  return { modified: foundHttpInput, result };
}

/**
 * Replaces streamkit::http_input with core::file_reader in pipeline YAML
 *
 * This function supports both pipeline formats:
 * - Linear format (steps:) - sequential processing chains
 * - DAG format (nodes:) - explicit node dependencies
 *
 * @param originalYaml - The original pipeline YAML string
 * @param assetPath - The file path to inject into file_reader params
 * @returns Modified YAML with http_input replaced by file_reader
 */
// eslint-disable-next-line max-statements -- Line-oriented YAML manipulation
export function injectFileReadNode(originalYaml: string, assetPath: string): string {
  utilsLogger.debug('injectFileReadNode called with path:', assetPath);
  utilsLogger.debug('Original YAML:', originalYaml);

  try {
    const lines = originalYaml.split('\n');
    const result: string[] = [];
    let inStepsSection = false;
    let inNodesSection = false;
    let foundHttpInput = false;

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      const trimmed = line.trim();

      // Detect section transitions
      if (trimmed.startsWith('steps:')) {
        inStepsSection = true;
        inNodesSection = false;
        result.push(line);
        continue;
      }

      if (trimmed.startsWith('nodes:')) {
        inNodesSection = true;
        inStepsSection = false;
        result.push(line);
        continue;
      }

      // Reset section flags when we hit a top-level key
      if (trimmed && !line.startsWith(' ') && !line.startsWith('\t')) {
        if (!trimmed.startsWith('steps:') && !trimmed.startsWith('nodes:')) {
          inStepsSection = false;
          inNodesSection = false;
        }
      }

      // Process steps section
      if (inStepsSection) {
        const remaining = lines.slice(i);
        const replacement = replaceHttpInputInSteps(remaining, assetPath);
        if (replacement.modified) {
          foundHttpInput = true;
          result.push(...replacement.result);
          break; // We've processed the rest of the file
        }
      }

      // Process nodes section
      if (inNodesSection) {
        const remaining = lines.slice(i);
        const replacement = replaceHttpInputInNodes(remaining, assetPath);
        if (replacement.modified) {
          foundHttpInput = true;
          result.push(...replacement.result);
          break; // We've processed the rest of the file
        }
      }

      result.push(line);
    }

    if (!foundHttpInput) {
      utilsLogger.warn('No streamkit::http_input found to replace');
    }

    const finalYaml = result.join('\n');
    utilsLogger.debug('Modified YAML:', finalYaml);
    return finalYaml;
  } catch (error) {
    utilsLogger.error('Failed to inject file_read node:', error);
    return originalYaml;
  }
}
