// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { ClientSection } from '@/types/types';

import {
  extractClientFromParsed,
  extractClientSection,
  parseAcceptToFormats,
  parseClientFromYaml,
} from './clientSection';
import { deriveSettingsFromClient } from './moqPeerSettings';

describe('extractClientFromParsed', () => {
  it('returns null for null input', () => {
    expect(extractClientFromParsed(null)).toBeNull();
  });

  it('returns null for undefined input', () => {
    expect(extractClientFromParsed(undefined)).toBeNull();
  });

  it('returns null when parsed object has no client key', () => {
    expect(extractClientFromParsed({ nodes: {}, steps: [] })).toBeNull();
  });

  it('extracts client section from a pre-parsed object', () => {
    const client: ClientSection = {
      relay_url: null,
      gateway_path: '/moq/test',
      publish: null,
      watch: null,
      input: {
        type: 'file_upload',
        accept: 'audio/opus',
        asset_tags: null,
        placeholder: null,
        field_hints: null,
      },
      output: { type: 'audio' },
    };
    const parsed = { nodes: {}, client };
    expect(extractClientFromParsed(parsed)).toBe(client);
  });
});

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

describe('parseAcceptToFormats', () => {
  it('returns null for null/undefined', () => {
    expect(parseAcceptToFormats(null)).toBeNull();
    expect(parseAcceptToFormats(undefined)).toBeNull();
  });

  it('returns null for empty string', () => {
    expect(parseAcceptToFormats('')).toBeNull();
  });

  it('returns null for wildcard accept', () => {
    expect(parseAcceptToFormats('audio/*')).toBeNull();
    expect(parseAcceptToFormats('*/*')).toBeNull();
  });

  it('parses dot-prefixed extensions', () => {
    expect(parseAcceptToFormats('.ogg,.opus')).toEqual(['ogg', 'opus']);
  });

  it('parses MIME types', () => {
    expect(parseAcceptToFormats('audio/wav,audio/ogg')).toEqual(['wav', 'ogg']);
  });

  it('parses mixed extensions and MIME types', () => {
    expect(parseAcceptToFormats('audio/wav,.wav')).toEqual(['wav', 'wav']);
  });

  it('handles bare format names', () => {
    expect(parseAcceptToFormats('ogg,opus')).toEqual(['ogg', 'opus']);
  });

  it('lowercases all formats', () => {
    expect(parseAcceptToFormats('.OGG,.OPUS')).toEqual(['ogg', 'opus']);
  });

  it('trims whitespace', () => {
    expect(parseAcceptToFormats(' .ogg , .opus ')).toEqual(['ogg', 'opus']);
  });
});

describe('parseClientFromYaml', () => {
  it('returns null for empty YAML', () => {
    expect(parseClientFromYaml('')).toBeNull();
  });

  it('returns null for YAML without client section', () => {
    const yaml = `
steps:
  - kind: ogg::demuxer
`;
    expect(parseClientFromYaml(yaml)).toBeNull();
  });

  it('parses a full client section from YAML', () => {
    const yaml = `
client:
  input:
    type: file_upload
    accept: ".ogg,.opus"
    asset_tags:
      - speech
    field_hints:
      voice:
        type: file
        accept: "audio/wav,.wav"
  output:
    type: transcription
steps:
  - kind: ogg::demuxer
`;
    const client = parseClientFromYaml(yaml);
    expect(client).not.toBeNull();
    expect(client?.input?.type).toBe('file_upload');
    expect(client?.input?.accept).toBe('.ogg,.opus');
    expect(client?.input?.asset_tags).toEqual(['speech']);
    expect(client?.input?.field_hints?.voice?.type).toBe('file');
    expect(client?.input?.field_hints?.voice?.accept).toBe('audio/wav,.wav');
    expect(client?.output?.type).toBe('transcription');
  });

  it('parses a dynamic pipeline client section', () => {
    const yaml = `
client:
  gateway_path: /moq/compositor
  publish:
    broadcast: camera-feed
    audio: true
    video: true
  watch:
    broadcast: composited-output
    audio: true
    video: true
nodes:
  pub:
    kind: moq::subscriber
`;
    const client = parseClientFromYaml(yaml);
    expect(client).not.toBeNull();
    expect(client?.gateway_path).toBe('/moq/compositor');
    expect(client?.publish?.broadcast).toBe('camera-feed');
    expect(client?.watch?.broadcast).toBe('composited-output');
  });

  it('returns null for invalid YAML', () => {
    expect(parseClientFromYaml('{{invalid')).toBeNull();
  });
});

// -----------------------------------------------------------------------
// Monitor preview config derivation (Gap 4a)
// -----------------------------------------------------------------------

describe('deriveSettingsFromClient — monitor preview scenarios', () => {
  it('derives watch-only settings for monitor preview (no publish)', () => {
    const client: ClientSection = {
      relay_url: null,
      gateway_path: '/moq/transcoder',
      publish: null,
      watch: { broadcast: 'output', audio: true, video: false },
      input: null,
      output: null,
    };

    const settings = deriveSettingsFromClient(client);

    expect(settings.gatewayPath).toBe('/moq/transcoder');
    expect(settings.outputBroadcast).toBe('output');
    expect(settings.hasInputBroadcast).toBe(false);
    expect(settings.outputsAudio).toBe(true);
    expect(settings.outputsVideo).toBe(false);
  });

  it('derives audio+video output for compositor preview', () => {
    const client: ClientSection = {
      relay_url: null,
      gateway_path: '/moq/compositor',
      publish: { broadcast: 'camera', audio: true, video: true },
      watch: { broadcast: 'composited', audio: true, video: true },
      input: null,
      output: null,
    };

    const settings = deriveSettingsFromClient(client);

    expect(settings.outputsAudio).toBe(true);
    expect(settings.outputsVideo).toBe(true);
    expect(settings.outputBroadcast).toBe('composited');
  });

  it('derives relay-url settings for external relay preview', () => {
    const client: ClientSection = {
      relay_url: 'https://relay.example.com',
      gateway_path: null,
      publish: null,
      watch: { broadcast: 'preview', audio: false, video: true },
      input: null,
      output: null,
    };

    const settings = deriveSettingsFromClient(client);

    expect(settings.relayUrl).toBe('https://relay.example.com');
    expect(settings.gatewayPath).toBeUndefined();
    expect(settings.outputsVideo).toBe(true);
    expect(settings.outputsAudio).toBe(false);
  });
});

// -----------------------------------------------------------------------
// ConvertView-driven pipeline classification (Gap 4b)
// -----------------------------------------------------------------------

describe('parseClientFromYaml — trigger vs none and output-type', () => {
  it('parses trigger input type', () => {
    const yaml = `
client:
  input:
    type: trigger
  output:
    type: video
steps:
  - kind: core::passthrough
`;
    const client = parseClientFromYaml(yaml);
    expect(client).not.toBeNull();
    expect(client?.input?.type).toBe('trigger');
    expect(client?.output?.type).toBe('video');
  });

  it('parses none input type', () => {
    const yaml = `
client:
  input:
    type: none
  output:
    type: video
steps:
  - kind: core::passthrough
`;
    const client = parseClientFromYaml(yaml);
    expect(client?.input?.type).toBe('none');
  });

  it('identifies transcription output type for renderer selection', () => {
    const yaml = `
client:
  input:
    type: file_upload
    accept: "audio/*"
  output:
    type: transcription
steps:
  - kind: streamkit::http_input
`;
    const client = parseClientFromYaml(yaml);
    expect(client?.output?.type).toBe('transcription');
    // ConvertView uses: isTranscriptionPipeline = client?.output?.type === 'transcription'
    expect(client?.output?.type === 'transcription').toBe(true);
  });

  it('identifies audio output type for audio player renderer', () => {
    const yaml = `
client:
  input:
    type: file_upload
    accept: "audio/*"
  output:
    type: audio
steps:
  - kind: streamkit::http_input
`;
    const client = parseClientFromYaml(yaml);
    expect(client?.output?.type).toBe('audio');
    // ConvertView renders CustomAudioPlayer for audio output
    expect(client?.output?.type === 'audio').toBe(true);
  });

  it('identifies video output type for video player renderer', () => {
    const yaml = `
client:
  input:
    type: none
  output:
    type: video
steps:
  - kind: core::passthrough
`;
    const client = parseClientFromYaml(yaml);
    expect(client?.output?.type).toBe('video');
    // ConvertView: isVideoPipeline = client?.output?.type === 'video'
    const isNoInput = client?.input?.type === 'none' || client?.input?.type === 'trigger';
    expect(isNoInput).toBe(true);
  });

  it('identifies json output type for JsonStreamDisplay', () => {
    const yaml = `
client:
  input:
    type: file_upload
    accept: "audio/*"
  output:
    type: json
steps:
  - kind: streamkit::http_input
`;
    const client = parseClientFromYaml(yaml);
    expect(client?.output?.type).toBe('json');
  });

  it('classifies text input as TTS pipeline', () => {
    const yaml = `
client:
  input:
    type: text
    placeholder: "Enter text to convert to speech"
  output:
    type: audio
steps:
  - kind: streamkit::http_input
`;
    const client = parseClientFromYaml(yaml);
    // ConvertView: isTTSPipeline = client?.input?.type === 'text'
    expect(client?.input?.type).toBe('text');
    expect(client?.input?.type === 'text').toBe(true);
  });

  it('trigger and none both classify as no-input pipeline', () => {
    for (const inputType of ['trigger', 'none']) {
      const yaml = `
client:
  input:
    type: ${inputType}
  output:
    type: audio
steps:
  - kind: core::passthrough
`;
      const client = parseClientFromYaml(yaml);
      // ConvertView: isNoInputPipeline = client?.input?.type === 'none' || client?.input?.type === 'trigger'
      const isNoInput = client?.input?.type === 'none' || client?.input?.type === 'trigger';
      expect(isNoInput).toBe(true);
    }
  });

  it('file_upload does NOT classify as no-input pipeline', () => {
    const yaml = `
client:
  input:
    type: file_upload
    accept: "audio/*"
  output:
    type: audio
steps:
  - kind: streamkit::http_input
`;
    const client = parseClientFromYaml(yaml);
    const isNoInput = client?.input?.type === 'none' || client?.input?.type === 'trigger';
    expect(isNoInput).toBe(false);
  });
});
