// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { load } from 'js-yaml';

import type { ClientSection } from '@/types/types';

import { deriveSettingsFromClient } from './clientSection';

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
  try {
    const parsed = load(yamlContent) as Record<string, unknown> | null;
    if (!parsed || typeof parsed !== 'object') return null;

    const client = parsed.client as ClientSection | undefined;
    if (!client) return null;

    // Only return settings for dynamic pipelines that declare MoQ transport.
    if (!client.gateway_path && !client.relay_url && !client.publish && !client.watch) {
      return null;
    }

    return deriveSettingsFromClient(client);
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
