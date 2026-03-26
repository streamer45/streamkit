// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { VideoSourceType } from '@/stores/streamStore';
import type { ClientSection } from '@/types/types';

import { parseClientFromYaml } from './clientSection';

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
  /**
   * Whether the pipeline uses an external MoQ relay (separate publisher/subscriber
   * nodes) rather than the built-in gateway (`transport::moq::peer`).
   *
   * True when `relay_url` is set explicitly, OR when the pipeline declares both
   * `publish` and `watch` without a `gateway_path` — indicating that skit nodes
   * connect directly to a relay and the browser must wait for the output broadcast
   * to be announced before subscribing.
   */
  isExternalRelay: boolean;
  /** The video capture source type: 'camera' (getUserMedia) or 'screen' (getDisplayMedia). */
  videoSourceType: VideoSourceType;
  /** Secondary publish config for dual-source pipelines (e.g. screen bg + camera PiP). */
  secondaryPublish?: {
    broadcast: string;
    videoSourceType: VideoSourceType;
  };
}

/**
 * Derives `MoqPeerSettings` from a declarative `ClientSection`.
 */
export function deriveSettingsFromClient(client: ClientSection): MoqPeerSettings {
  const hasRelayUrl = Boolean(client.relay_url);
  const hasGatewayPath = Boolean(client.gateway_path);

  // External relay pattern: relay_url is explicit, OR the pipeline declares
  // both publish and watch without a gateway_path.  Gateway pipelines always
  // set gateway_path; its absence with pub+watch means nodes connect to a
  // standalone relay and the browser must wait for the output broadcast
  // announcement before subscribing (otherwise the catalog subscribe gets
  // RESET_STREAM because skit hasn't published yet).
  const isExternalRelay =
    hasRelayUrl || (!hasGatewayPath && Boolean(client.publish) && Boolean(client.watch));

  return {
    gatewayPath: client.gateway_path ?? undefined,
    relayUrl: client.relay_url ?? undefined,
    inputBroadcast: client.publish?.broadcast,
    outputBroadcast: client.watch?.broadcast,
    hasInputBroadcast: Boolean(client.publish),
    needsAudioInput: client.publish?.audio ?? false,
    needsVideoInput: client.publish?.video ?? false,
    outputsAudio: client.watch?.audio ?? false,
    outputsVideo: client.watch?.video ?? false,
    isExternalRelay,
    videoSourceType: client.publish?.screen ? 'screen' : 'camera',
    secondaryPublish: client.secondary_publish
      ? {
          broadcast: client.secondary_publish.broadcast,
          videoSourceType: client.secondary_publish.screen ? 'screen' : 'camera',
        }
      : undefined,
  };
}

/**
 * Extracts MoQ peer settings from a pipeline YAML string by reading the
 * declarative `client` section.
 *
 * Returns settings only when the client section declares dynamic transport
 * configuration (gateway_path, relay_url, publish, or watch).  Oneshot
 * pipelines (input/output only) return null.
 *
 * @param yamlContent - The YAML string to parse
 * @returns MoqPeerSettings if the client section declares MoQ transport, null otherwise
 */
export function extractMoqPeerSettings(yamlContent: string): MoqPeerSettings | null {
  const client = parseClientFromYaml(yamlContent);
  if (!client) return null;

  // Only return settings for dynamic pipelines that declare MoQ transport.
  if (!client.gateway_path && !client.relay_url && !client.publish && !client.watch) {
    return null;
  }

  return deriveSettingsFromClient(client);
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
