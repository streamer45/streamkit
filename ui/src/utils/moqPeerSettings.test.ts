// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { extractMoqPeerSettings } from './moqPeerSettings';

// ---------------------------------------------------------------------------
// extractMoqPeerSettings — reads the declarative `client` section
// ---------------------------------------------------------------------------

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
    audio: true
    video: false
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
  });

  it('should extract relay-based settings', () => {
    const yaml = `
client:
  relay_url: "https://relay.example.com"
  publish:
    broadcast: input-stream
    audio: true
    video: true
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
  });

  it('should detect audio+video publish and watch', () => {
    const yaml = `
client:
  gateway_path: /moq/av
  publish:
    broadcast: av-input
    audio: true
    video: true
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
  });

  it('should handle audio-only publish', () => {
    const yaml = `
client:
  gateway_path: /moq/audio
  publish:
    broadcast: mic
    audio: true
    video: false
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
});
