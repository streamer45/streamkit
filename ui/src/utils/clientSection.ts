// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { load } from 'js-yaml';

import type { ClientSection, Pipeline } from '@/types/types';

/**
 * Extracts the `client` section from a compiled `Pipeline`, returning `null`
 * when the field is absent.
 */
export function extractClientSection(pipeline: Pipeline | null | undefined): ClientSection | null {
  return pipeline?.client ?? null;
}

/**
 * Extracts the `client` section from an already-parsed YAML object.
 * Use this when you have already called `load()` and want to avoid
 * parsing the same YAML string again.
 */
export function extractClientFromParsed(
  parsed: Record<string, unknown> | null | undefined
): ClientSection | null {
  if (!parsed || typeof parsed !== 'object') return null;
  return (parsed.client as ClientSection) ?? null;
}

/**
 * Parses a raw pipeline YAML string and extracts the `client` section.
 * Returns `null` if the YAML is invalid or has no client section.
 */
export function parseClientFromYaml(yamlContent: string): ClientSection | null {
  try {
    const parsed = load(yamlContent) as Record<string, unknown> | null;
    return extractClientFromParsed(parsed);
  } catch {
    return null;
  }
}

/**
 * Converts a CSS-style `accept` attribute (e.g. `"audio/*"`, `".ogg,.opus"`)
 * into an array of lowercase format names suitable for asset-format matching.
 * Returns `null` when all formats are acceptable.
 */
export function parseAcceptToFormats(accept: string | null | undefined): string[] | null {
  if (!accept) return null;
  if (accept.includes('*')) return null;

  const formats: string[] = [];
  for (const part of accept.split(',')) {
    const trimmed = part.trim();
    if (!trimmed) continue;
    if (trimmed.startsWith('.')) {
      formats.push(trimmed.slice(1).toLowerCase());
    } else if (trimmed.includes('/')) {
      const sub = trimmed.split('/')[1]?.toLowerCase();
      if (sub) formats.push(sub);
    } else {
      formats.push(trimmed.toLowerCase());
    }
  }
  return formats.length > 0 ? formats : null;
}

/**
 * Derives `MoqPeerSettings`-shaped data from a declarative `ClientSection`.
 *
 * The return type mirrors the `MoqPeerSettings` interface defined in
 * `moqPeerSettings.ts`.  We avoid importing that interface here to prevent
 * a circular module dependency.
 */
export function deriveSettingsFromClient(client: ClientSection): {
  gatewayPath?: string;
  relayUrl?: string;
  inputBroadcast?: string;
  outputBroadcast?: string;
  hasInputBroadcast: boolean;
  needsAudioInput: boolean;
  needsVideoInput: boolean;
  outputsAudio: boolean;
  outputsVideo: boolean;
} {
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
  };
}
