// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { VideoSourceType } from '@/stores/streamStore';
import type { ClientSection, PublishTrackConfig } from '@/types/types';

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
  /** Parsed tracks from the publish config. */
  tracks: PublishTrackConfig[];
  /** All unique broadcast names derived from tracks (for multi-broadcast). */
  publishBroadcasts: string[];
  /**
   * MSE endpoint path suffix from `client.watch.mse_path` (e.g. `/video`).
   * When set, the browser fetches chunked WebM from
   * `/mse/{session_id}{msePath}` and plays it via `MSEPlayer`.
   */
  msePath?: string;
}

/**
 * Derives `MoqPeerSettings` from a declarative `ClientSection`.
 */
export function deriveSettingsFromClient(client: ClientSection): MoqPeerSettings {
  const hasRelayUrl = Boolean(client.relay_url);
  const hasGatewayPath = Boolean(client.gateway_path);

  // External relay pattern: relay_url is explicit, OR the pipeline declares
  // both publish and watch (with a MoQ broadcast) without a gateway_path.
  // Gateway pipelines always set gateway_path; its absence with pub+watch
  // means nodes connect to a standalone relay and the browser must wait for
  // the output broadcast announcement before subscribing.
  // MSE-only watch configs (no broadcast) don't count as external relay.
  const hasMoqWatch = Boolean(client.watch?.broadcast);
  const isExternalRelay =
    hasRelayUrl || (!hasGatewayPath && Boolean(client.publish) && hasMoqWatch);

  // Derive media needs from tracks array
  const tracks: PublishTrackConfig[] = client.publish?.tracks ?? [];
  const needsAudioInput = tracks.some((t) => t.kind === 'audio');
  const needsVideoInput = tracks.some((t) => t.kind === 'video');

  // Determine the video source type from the primary broadcast's video track.
  // A track with source: 'screen' means getDisplayMedia, otherwise getUserMedia.
  const defaultBroadcast = client.publish?.broadcast;
  const primaryVideoTrack = tracks.find(
    (t) => t.kind === 'video' && (t.broadcast ?? defaultBroadcast) === defaultBroadcast
  );
  const videoSourceType: VideoSourceType =
    primaryVideoTrack?.source === 'screen' ? 'screen' : 'camera';

  // Collect all unique broadcast names from tracks
  const publishBroadcasts: string[] = [];
  if (defaultBroadcast) {
    publishBroadcasts.push(defaultBroadcast);
  }
  for (const track of tracks) {
    const bc = track.broadcast ?? defaultBroadcast;
    if (bc && !publishBroadcasts.includes(bc)) {
      publishBroadcasts.push(bc);
    }
  }

  return {
    gatewayPath: client.gateway_path ?? undefined,
    relayUrl: client.relay_url ?? undefined,
    inputBroadcast: client.publish?.broadcast,
    outputBroadcast: client.watch?.broadcast ?? undefined,
    hasInputBroadcast: Boolean(client.publish),
    needsAudioInput,
    needsVideoInput,
    outputsAudio: client.watch?.audio ?? false,
    outputsVideo: client.watch?.video ?? false,
    isExternalRelay,
    videoSourceType,
    tracks,
    publishBroadcasts,
    msePath: client.watch?.mse_path ?? undefined,
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
