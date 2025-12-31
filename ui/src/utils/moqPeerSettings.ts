// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { load } from 'js-yaml';

export interface MoqPeerSettings {
  gatewayPath?: string;
  inputBroadcast?: string;
  outputBroadcast?: string;
}

type ParsedNode = {
  kind?: string;
  params?: {
    gateway_path?: string;
    input_broadcast?: string;
    output_broadcast?: string;
  };
};

type ParsedYaml = {
  nodes?: Record<string, ParsedNode>;
};

/**
 * Extracts moq_peer settings from a pipeline YAML string.
 * Looks for any node with kind 'transport::moq::peer' and returns its
 * gateway_path, input_broadcast, and output_broadcast parameters.
 *
 * @param yamlContent - The YAML string to parse
 * @returns MoqPeerSettings if a moq_peer node is found, null otherwise
 */
export function extractMoqPeerSettings(yamlContent: string): MoqPeerSettings | null {
  try {
    const parsed = load(yamlContent) as ParsedYaml;

    if (!parsed || typeof parsed !== 'object' || !parsed.nodes) {
      return null;
    }

    // Find the first node with kind 'transport::moq::peer'
    for (const nodeConfig of Object.values(parsed.nodes)) {
      if (nodeConfig.kind === 'transport::moq::peer' && nodeConfig.params) {
        return {
          gatewayPath: nodeConfig.params.gateway_path,
          inputBroadcast: nodeConfig.params.input_broadcast,
          outputBroadcast: nodeConfig.params.output_broadcast,
        };
      }
    }

    return null;
  } catch {
    return null;
  }
}

/**
 * Updates a URL's path with a new path while preserving the protocol, host, and port.
 *
 * @param baseUrl - The original URL string
 * @param newPath - The new path to set
 * @returns The updated URL string, or the original if parsing fails
 */
export function updateUrlPath(baseUrl: string, newPath: string): string {
  try {
    const url = new URL(baseUrl);
    url.pathname = newPath;
    return url.toString();
  } catch {
    // If URL parsing fails, try a simple path replacement
    // Handle URLs like "https://example.com:4545/moq" -> "https://example.com:4545/moq/transcoder"
    const match = baseUrl.match(/^(https?:\/\/[^/]+)(\/.*)?$/);
    if (match) {
      return match[1] + newPath;
    }
    return baseUrl;
  }
}
