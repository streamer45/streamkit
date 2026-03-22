// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { ClientSection } from '@/types/types';

import { deriveSettingsFromClient, extractClientSection } from './clientSection';

describe('extractClientSection', () => {
  it('returns null for null pipeline', () => {
    expect(extractClientSection(null)).toBeNull();
  });

  it('returns null for undefined pipeline', () => {
    expect(extractClientSection(undefined)).toBeNull();
  });

  it('returns null when pipeline has no client', () => {
    const pipeline = { nodes: {}, connections: [], mode: 'live' } as never;
    expect(extractClientSection(pipeline)).toBeNull();
  });

  it('returns client section when present', () => {
    const client: ClientSection = {
      relay_url: null,
      gateway_path: '/moq/test',
      publish: null,
      watch: null,
      input: null,
      output: null,
    };
    const pipeline = { nodes: {}, connections: [], mode: 'live', client } as never;
    expect(extractClientSection(pipeline)).toBe(client);
  });
});

describe('deriveSettingsFromClient', () => {
  it('derives settings for a gateway publish+watch pipeline', () => {
    const client: ClientSection = {
      relay_url: null,
      gateway_path: '/moq/compositor',
      publish: { broadcast: 'camera-feed', audio: true, video: true },
      watch: { broadcast: 'composited-output', audio: true, video: true },
      input: null,
      output: null,
    };

    const settings = deriveSettingsFromClient(client);

    expect(settings).toEqual({
      gatewayPath: '/moq/compositor',
      relayUrl: undefined,
      inputBroadcast: 'camera-feed',
      outputBroadcast: 'composited-output',
      hasInputBroadcast: true,
      needsAudioInput: true,
      needsVideoInput: true,
      outputsAudio: true,
      outputsVideo: true,
    });
  });

  it('derives settings for a relay-based pipeline', () => {
    const client: ClientSection = {
      relay_url: 'https://relay.example.com',
      gateway_path: null,
      publish: { broadcast: 'input', audio: true, video: false },
      watch: { broadcast: 'output', audio: false, video: true },
      input: null,
      output: null,
    };

    const settings = deriveSettingsFromClient(client);

    expect(settings).toEqual({
      gatewayPath: undefined,
      relayUrl: 'https://relay.example.com',
      inputBroadcast: 'input',
      outputBroadcast: 'output',
      hasInputBroadcast: true,
      needsAudioInput: true,
      needsVideoInput: false,
      outputsAudio: false,
      outputsVideo: true,
    });
  });

  it('derives settings for a watch-only pipeline', () => {
    const client: ClientSection = {
      relay_url: null,
      gateway_path: '/moq/monitor',
      publish: null,
      watch: { broadcast: 'preview', audio: false, video: true },
      input: null,
      output: null,
    };

    const settings = deriveSettingsFromClient(client);

    expect(settings).toEqual({
      gatewayPath: '/moq/monitor',
      relayUrl: undefined,
      inputBroadcast: undefined,
      outputBroadcast: 'preview',
      hasInputBroadcast: false,
      needsAudioInput: false,
      needsVideoInput: false,
      outputsAudio: false,
      outputsVideo: true,
    });
  });

  it('derives settings for a oneshot pipeline (no publish/watch)', () => {
    const client: ClientSection = {
      relay_url: null,
      gateway_path: null,
      publish: null,
      watch: null,
      input: {
        type: 'file_upload',
        accept: 'audio/*',
        asset_tags: null,
        placeholder: null,
        field_hints: null,
      },
      output: { type: 'transcription' },
    };

    const settings = deriveSettingsFromClient(client);

    expect(settings).toEqual({
      gatewayPath: undefined,
      relayUrl: undefined,
      inputBroadcast: undefined,
      outputBroadcast: undefined,
      hasInputBroadcast: false,
      needsAudioInput: false,
      needsVideoInput: false,
      outputsAudio: false,
      outputsVideo: false,
    });
  });
});
