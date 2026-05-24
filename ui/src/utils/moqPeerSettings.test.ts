// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { extractMoqPeerSettings, updateUrlPath } from './moqPeerSettings';

// extractMoqPeerSettings — reads the declarative `client` section

describe('extractMoqPeerSettings', () => {
  it('should return null for empty YAML', () => {
    expect(extractMoqPeerSettings('')).toBeNull();
  });

  it('should return null for YAML without client section', () => {
    expect(extractMoqPeerSettings('name: test')).toBeNull();
  });

  it('should return null for invalid YAML', () => {
    expect(extractMoqPeerSettings('{{{')).toBeNull();
  });

  it('should return null for oneshot client (input/output only)', () => {
    const yaml = `
client:
  input:
    type: file_upload
    accept: "audio/*"
  output:
    type: audio
nodes:
  gain:
    kind: audio::gain
`;
    expect(extractMoqPeerSettings(yaml)).toBeNull();
  });

  it('should extract gateway-based settings with publish and watch', () => {
    const yaml = `
client:
  gateway_path: /moq/echo
  publish:
    broadcast: echo-demo
    tracks:
      - kind: audio
        source: microphone
  watch:
    broadcast: echo-demo
    audio: true
    video: false
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.gatewayPath).toBe('/moq/echo');
    expect(result!.inputBroadcast).toBe('echo-demo');
    expect(result!.outputBroadcast).toBe('echo-demo');
    expect(result!.hasInputBroadcast).toBe(true);
    expect(result!.needsAudioInput).toBe(true);
    expect(result!.needsVideoInput).toBe(false);
    expect(result!.outputsAudio).toBe(true);
    expect(result!.outputsVideo).toBe(false);
    expect(result!.isExternalRelay).toBe(false);
  });

  it('should extract relay-based settings', () => {
    const yaml = `
client:
  relay_url: "https://relay.example.com"
  publish:
    broadcast: input-stream
    tracks:
      - kind: audio
        source: microphone
      - kind: video
        source: camera
  watch:
    broadcast: output-stream
    audio: true
    video: false
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.relayUrl).toBe('https://relay.example.com');
    expect(result!.gatewayPath).toBeUndefined();
    expect(result!.inputBroadcast).toBe('input-stream');
    expect(result!.outputBroadcast).toBe('output-stream');
    expect(result!.hasInputBroadcast).toBe(true);
    expect(result!.needsAudioInput).toBe(true);
    expect(result!.needsVideoInput).toBe(true);
    expect(result!.outputsAudio).toBe(true);
    expect(result!.outputsVideo).toBe(false);
    expect(result!.isExternalRelay).toBe(true);
  });

  it('should report hasInputBroadcast false when publish is absent', () => {
    const yaml = `
client:
  gateway_path: /moq/colorbars
  watch:
    broadcast: colorbars
    audio: false
    video: true
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.hasInputBroadcast).toBe(false);
    expect(result!.needsAudioInput).toBe(false);
    expect(result!.needsVideoInput).toBe(false);
    expect(result!.outputsAudio).toBe(false);
    expect(result!.outputsVideo).toBe(true);
    expect(result!.isExternalRelay).toBe(false);
  });

  it('should detect audio+video publish and watch', () => {
    const yaml = `
client:
  gateway_path: /moq/av
  publish:
    broadcast: av-input
    tracks:
      - kind: audio
        source: microphone
      - kind: video
        source: camera
  watch:
    broadcast: av-output
    audio: true
    video: true
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.needsAudioInput).toBe(true);
    expect(result!.needsVideoInput).toBe(true);
    expect(result!.outputsAudio).toBe(true);
    expect(result!.outputsVideo).toBe(true);
  });

  it('should handle watch-only pipeline (no publish)', () => {
    const yaml = `
client:
  gateway_path: /moq/output
  watch:
    broadcast: output-stream
    audio: true
    video: true
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.hasInputBroadcast).toBe(false);
    expect(result!.needsAudioInput).toBe(false);
    expect(result!.needsVideoInput).toBe(false);
    expect(result!.outputsAudio).toBe(true);
    expect(result!.outputsVideo).toBe(true);
  });

  it('should return settings when only gateway_path is present', () => {
    const yaml = `
client:
  gateway_path: /moq/peer
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.gatewayPath).toBe('/moq/peer');
    expect(result!.hasInputBroadcast).toBe(false);
    expect(result!.outputsAudio).toBe(false);
    expect(result!.outputsVideo).toBe(false);
  });

  it('should return settings when only relay_url is present', () => {
    const yaml = `
client:
  relay_url: "https://relay.example.com"
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.relayUrl).toBe('https://relay.example.com');
    expect(result!.hasInputBroadcast).toBe(false);
    expect(result!.isExternalRelay).toBe(true);
  });

  it('should detect isExternalRelay from pub+watch without gateway_path', () => {
    // When relay_url is absent but the pipeline declares both publish and watch
    // without a gateway_path, it must be an external relay pattern — the browser
    // needs to wait for the output broadcast announcement before subscribing.
    const yaml = `
client:
  publish:
    broadcast: input
    tracks:
      - kind: audio
        source: microphone
      - kind: video
        source: camera
  watch:
    broadcast: output
    audio: true
    video: true
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.isExternalRelay).toBe(true);
    expect(result!.relayUrl).toBeUndefined();
    expect(result!.gatewayPath).toBeUndefined();
  });

  it('should not flag isExternalRelay for gateway pub+watch', () => {
    const yaml = `
client:
  gateway_path: /moq/av
  publish:
    broadcast: input
    tracks:
      - kind: audio
        source: microphone
      - kind: video
        source: camera
  watch:
    broadcast: output
    audio: true
    video: true
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.isExternalRelay).toBe(false);
  });

  it('should handle audio-only publish', () => {
    const yaml = `
client:
  gateway_path: /moq/audio
  publish:
    broadcast: mic
    tracks:
      - kind: audio
        source: microphone
  watch:
    broadcast: processed
    audio: true
    video: false
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.needsAudioInput).toBe(true);
    expect(result!.needsVideoInput).toBe(false);
    expect(result!.outputsAudio).toBe(true);
    expect(result!.outputsVideo).toBe(false);
  });

  // videoSourceType extraction

  it('should default videoSourceType to camera when not specified', () => {
    const yaml = `
client:
  gateway_path: /moq/echo
  publish:
    broadcast: input
    tracks:
      - kind: audio
        source: microphone
      - kind: video
        source: camera
  watch:
    broadcast: output
    audio: true
    video: true
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.videoSourceType).toBe('camera');
  });

  it('should extract videoSourceType as screen when specified', () => {
    const yaml = `
client:
  gateway_path: /moq/screen
  publish:
    broadcast: input
    tracks:
      - kind: audio
        source: microphone
      - kind: video
        source: screen
  watch:
    broadcast: output
    audio: true
    video: true
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.videoSourceType).toBe('screen');
  });

  it('should extract videoSourceType as camera when explicitly set', () => {
    const yaml = `
client:
  gateway_path: /moq/cam
  publish:
    broadcast: input
    tracks:
      - kind: audio
        source: microphone
      - kind: video
        source: camera
  watch:
    broadcast: output
    audio: true
    video: true
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.videoSourceType).toBe('camera');
  });

  it('should default videoSourceType to camera when publish is absent', () => {
    const yaml = `
client:
  gateway_path: /moq/output
  watch:
    broadcast: output
    audio: true
    video: true
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.videoSourceType).toBe('camera');
  });

  // Multi-broadcast track grouping

  it('should extract multi-broadcast tracks and publishBroadcasts', () => {
    const yaml = `
client:
  gateway_path: /moq/screenshare
  publish:
    broadcast: screen-input
    tracks:
      - kind: audio
        source: microphone
      - kind: video
        source: screen
      - kind: video
        source: camera
        broadcast: cam-input
  watch:
    broadcast: output
    audio: true
    video: true
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.tracks).toHaveLength(3);
    expect(result!.publishBroadcasts).toEqual(['screen-input', 'cam-input']);
    expect(result!.videoSourceType).toBe('screen');
    expect(result!.needsAudioInput).toBe(true);
    expect(result!.needsVideoInput).toBe(true);
  });

  it('should return empty tracks and publishBroadcasts when no publish', () => {
    const yaml = `
client:
  gateway_path: /moq/output
  watch:
    broadcast: output
    audio: true
    video: true
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.tracks).toEqual([]);
    expect(result!.publishBroadcasts).toEqual([]);
  });
});

// updateUrlPath — preserves host when applying a gateway path

describe('updateUrlPath', () => {
  it('should replace path on a standard URL', () => {
    expect(updateUrlPath('http://127.0.0.1:4545/moq', '/moq/echo')).toBe(
      'http://127.0.0.1:4545/moq/echo'
    );
  });

  it('should replace path on a relay URL (regression: gateway→relay→gateway)', () => {
    // If the caller mistakenly passes a relay URL as baseUrl when switching
    // back to a gateway pipeline, the result keeps the relay host — which is
    // the bug.  The fix is for the *caller* to always pass the original
    // config URL, but this test documents updateUrlPath's expected behaviour.
    expect(updateUrlPath('http://localhost:4443', '/moq')).toBe('http://localhost:4443/moq');
  });

  it('should handle URLs with trailing slashes', () => {
    expect(updateUrlPath('http://example.com:4545/', '/moq/transcoder')).toBe(
      'http://example.com:4545/moq/transcoder'
    );
  });

  it('should preserve protocol and port', () => {
    expect(updateUrlPath('https://host.example.com:9443/old-path', '/moq/new')).toBe(
      'https://host.example.com:9443/moq/new'
    );
  });
});
