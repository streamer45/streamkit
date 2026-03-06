// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { load } from 'js-yaml';

export interface MoqPeerSettings {
  gatewayPath?: string;
  inputBroadcast?: string;
  outputBroadcast?: string;
  /** Whether the pipeline declares an input_broadcast (i.e. expects a publisher). */
  hasInputBroadcast: boolean;
  /** Whether the pipeline consumes audio from the client's input broadcast. */
  needsAudioInput: boolean;
  /** Whether the pipeline consumes video from the client's input broadcast. */
  needsVideoInput: boolean;
}

type NeedsValue = string | string[] | Record<string, string>;

type ParsedNode = {
  kind?: string;
  params?: {
    gateway_path?: string;
    input_broadcast?: string;
    output_broadcast?: string;
  };
  needs?: NeedsValue;
};

type ParsedYaml = {
  nodes?: Record<string, ParsedNode>;
};

/**
 * Collects all dependency references from a node's `needs` field as flat strings.
 */
function collectNeedsRefs(needs: NeedsValue | undefined): string[] {
  if (!needs) return [];
  if (typeof needs === 'string') return [needs];
  if (Array.isArray(needs)) return needs.filter((v): v is string => typeof v === 'string');
  // Record<string, string> (map variant) — values are the dependency refs
  return Object.values(needs).filter((v): v is string => typeof v === 'string');
}

/**
 * Scans all nodes in the pipeline to detect which media types downstream nodes
 * consume from the moq_peer's output pins.
 *
 * - A reference to `<peerName>` (bare) or `<peerName>.out` → audio
 * - A reference to `<peerName>.out_1` → video
 */
function detectPeerInputMediaTypes(
  peerName: string,
  nodes: Record<string, ParsedNode>
): { needsAudio: boolean; needsVideo: boolean } {
  let needsAudio = false;
  let needsVideo = false;

  for (const [nodeName, nodeConfig] of Object.entries(nodes)) {
    if (nodeName === peerName) continue;
    for (const ref of collectNeedsRefs(nodeConfig.needs)) {
      if (ref === peerName || ref === `${peerName}.out`) {
        needsAudio = true;
      } else if (ref === `${peerName}.out_1`) {
        needsVideo = true;
      }
    }
  }

  return { needsAudio, needsVideo };
}

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
    let peerNodeName: string | null = null;
    let peerNodeConfig: ParsedNode | null = null;
    for (const [name, nodeConfig] of Object.entries(parsed.nodes)) {
      if (nodeConfig.kind === 'transport::moq::peer') {
        peerNodeName = name;
        peerNodeConfig = nodeConfig;
        break;
      }
    }

    if (!peerNodeName || !peerNodeConfig?.params) {
      return null;
    }

    // Determine which media types downstream nodes consume from the moq_peer.
    // References to "<peer>" or "<peer>.out" indicate audio;
    // references to "<peer>.out_1" indicate video.
    const { needsAudio, needsVideo } = detectPeerInputMediaTypes(peerNodeName, parsed.nodes);

    return {
      gatewayPath: peerNodeConfig.params.gateway_path,
      inputBroadcast: peerNodeConfig.params.input_broadcast,
      outputBroadcast: peerNodeConfig.params.output_broadcast,
      hasInputBroadcast: Boolean(peerNodeConfig.params.input_broadcast),
      needsAudioInput: needsAudio,
      needsVideoInput: needsVideo,
    };
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
