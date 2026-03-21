// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { load } from 'js-yaml';

export interface MoqPeerSettings {
  gatewayPath?: string;
  /** Direct relay URL from publisher/subscriber `url` param (external relay pattern). */
  relayUrl?: string;
  inputBroadcast?: string;
  outputBroadcast?: string;
  /** Whether the pipeline declares an input_broadcast (i.e. expects a publisher). */
  hasInputBroadcast: boolean;
  /** Whether the pipeline consumes audio from the client's input broadcast. */
  needsAudioInput: boolean;
  /** Whether the pipeline consumes video from the client's input broadcast. */
  needsVideoInput: boolean;
  /** Whether the pipeline outputs audio to subscribers via the moq_peer. */
  outputsAudio: boolean;
  /** Whether the pipeline outputs video to subscribers via the moq_peer. */
  outputsVideo: boolean;
}

type NeedsValue = string | string[] | Record<string, string>;

type ParsedNode = {
  kind?: string;
  params?: {
    gateway_path?: string;
    input_broadcast?: string;
    output_broadcast?: string;
    url?: string;
    broadcast?: string;
    audio?: boolean;
    video?: boolean;
    [key: string]: unknown;
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
 * The `transport::moq::peer` node exposes two fixed output pins:
 *   - `out`   → audio (Opus-encoded)
 *   - `out_1` → video (VP9-encoded)
 *
 * This is a stable naming convention enforced by the backend node
 * definition (see `MoqPeerNode::output_pins` in `crates/nodes/src/transport/moq/peer.rs`).
 * A bare reference to the peer name (e.g. `moq_peer`) is equivalent to `moq_peer.out`.
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
 * Detects what media types the moq_peer outputs to subscribers by looking at
 * which upstream nodes are connected to its inputs.  The node `kind` prefix
 * (`audio::` or `video::`) determines the media type.
 */
function detectPeerOutputMediaTypes(
  peerName: string,
  nodes: Record<string, ParsedNode>
): { outputsAudio: boolean; outputsVideo: boolean } {
  const peerNode = nodes[peerName];
  if (!peerNode) return { outputsAudio: false, outputsVideo: false };

  let outputsAudio = false;
  let outputsVideo = false;

  for (const ref of collectNeedsRefs(peerNode.needs)) {
    const nodeName = ref.split('.')[0];
    const sourceNode = nodes[nodeName];
    if (!sourceNode?.kind) continue;

    if (sourceNode.kind.startsWith('audio::')) {
      outputsAudio = true;
    } else if (sourceNode.kind.startsWith('video::')) {
      outputsVideo = true;
    }
  }

  return { outputsAudio, outputsVideo };
}

/** Finds the first subscriber and publisher nodes in the pipeline. */
function findPubSubNodes(nodes: Record<string, ParsedNode>): {
  subscriberName: string | null;
  subscriberConfig: ParsedNode | null;
  publisherConfig: ParsedNode | null;
} {
  let subscriberName: string | null = null;
  let subscriberConfig: ParsedNode | null = null;
  let publisherConfig: ParsedNode | null = null;

  for (const [name, nodeConfig] of Object.entries(nodes)) {
    if (nodeConfig.kind === 'transport::moq::subscriber') {
      subscriberName = name;
      subscriberConfig = nodeConfig;
    } else if (nodeConfig.kind === 'transport::moq::publisher') {
      publisherConfig = nodeConfig;
    }
  }

  return { subscriberName, subscriberConfig, publisherConfig };
}

/**
 * Detects which media types downstream nodes consume from a subscriber node
 * by scanning `needs` references across all nodes.
 */
function detectSubscriberInputMediaTypes(
  subscriberName: string,
  nodes: Record<string, ParsedNode>
): { needsAudio: boolean; needsVideo: boolean } {
  let needsAudio = false;
  let needsVideo = false;

  for (const nodeConfig of Object.values(nodes)) {
    for (const ref of collectNeedsRefs(nodeConfig.needs)) {
      if (ref === subscriberName || ref === `${subscriberName}.out`) {
        needsAudio = true;
      } else if (ref.startsWith(`${subscriberName}.`) && ref.includes('video')) {
        needsVideo = true;
      }
    }
  }

  return { needsAudio, needsVideo };
}

/**
 * Detects media types for pipelines using separate `transport::moq::publisher`
 * and `transport::moq::subscriber` nodes (external relay pattern).
 */
function extractPubSubSettings(nodes: Record<string, ParsedNode>): MoqPeerSettings | null {
  const { subscriberName, subscriberConfig, publisherConfig } = findPubSubNodes(nodes);
  if (!subscriberConfig && !publisherConfig) return null;

  const subParams = subscriberConfig?.params;
  const pubParams = publisherConfig?.params;

  const inputMedia =
    subscriberName != null
      ? detectSubscriberInputMediaTypes(subscriberName, nodes)
      : { needsAudio: false, needsVideo: false };

  return {
    relayUrl: subParams?.url ?? pubParams?.url,
    inputBroadcast: subParams?.broadcast,
    outputBroadcast: pubParams?.broadcast,
    hasInputBroadcast: Boolean(subParams?.broadcast),
    needsAudioInput: inputMedia.needsAudio,
    needsVideoInput: inputMedia.needsVideo,
    outputsAudio: pubParams?.audio === true,
    outputsVideo: pubParams?.video === true,
  };
}

/**
 * Extracts moq_peer settings from a pipeline YAML string.
 * Looks for `transport::moq::peer` nodes first (gateway pattern), then falls
 * back to separate `transport::moq::publisher`/`subscriber` nodes (external
 * relay pattern).
 *
 * @param yamlContent - The YAML string to parse
 * @returns MoqPeerSettings if MoQ transport nodes are found, null otherwise
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

    // Gateway pattern: transport::moq::peer
    if (peerNodeName && peerNodeConfig?.params) {
      const { needsAudio, needsVideo } = detectPeerInputMediaTypes(peerNodeName, parsed.nodes);
      const { outputsAudio, outputsVideo } = detectPeerOutputMediaTypes(peerNodeName, parsed.nodes);

      return {
        gatewayPath: peerNodeConfig.params.gateway_path,
        inputBroadcast: peerNodeConfig.params.input_broadcast,
        outputBroadcast: peerNodeConfig.params.output_broadcast,
        hasInputBroadcast: Boolean(peerNodeConfig.params.input_broadcast),
        needsAudioInput: needsAudio,
        needsVideoInput: needsVideo,
        outputsAudio,
        outputsVideo,
      };
    }

    // External relay pattern: separate publisher/subscriber nodes
    return extractPubSubSettings(parsed.nodes);
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
