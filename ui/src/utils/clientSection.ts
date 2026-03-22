// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { ClientSection, Pipeline } from '@/types/types';

import type { MoqPeerSettings } from './moqPeerSettings';

/**
 * Extracts the `client` section from a compiled `Pipeline`, returning `null`
 * when the field is absent.
 */
export function extractClientSection(pipeline: Pipeline | null | undefined): ClientSection | null {
  return pipeline?.client ?? null;
}

/**
 * Derives `MoqPeerSettings` from a declarative `ClientSection`.
 *
 * This is the client-section counterpart of `extractMoqPeerSettings()` (which
 * heuristically inspects raw YAML).  When a pipeline carries a `client`
 * section the UI should prefer this function.
 */
export function deriveSettingsFromClient(client: ClientSection): MoqPeerSettings {
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
