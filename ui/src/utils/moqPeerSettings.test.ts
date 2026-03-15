// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { extractMoqPeerSettings } from './moqPeerSettings';

// ---------------------------------------------------------------------------
// extractMoqPeerSettings — basic parsing
// ---------------------------------------------------------------------------

describe('extractMoqPeerSettings', () => {
  it('should return null for empty YAML', () => {
    expect(extractMoqPeerSettings('')).toBeNull();
  });

  it('should return null for YAML without nodes', () => {
    expect(extractMoqPeerSettings('name: test')).toBeNull();
  });

  it('should return null when no moq_peer node exists', () => {
    const yaml = `
nodes:
  gain:
    kind: audio::gain
    params:
      gain: 1.0
`;
    expect(extractMoqPeerSettings(yaml)).toBeNull();
  });

  it('should return null for invalid YAML', () => {
    expect(extractMoqPeerSettings('{{{')).toBeNull();
  });

  it('should return null when moq_peer has no params', () => {
    const yaml = `
nodes:
  moq_peer:
    kind: transport::moq::peer
`;
    expect(extractMoqPeerSettings(yaml)).toBeNull();
  });

  it('should extract basic moq_peer settings', () => {
    const yaml = `
nodes:
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      input_broadcast: input
      output_broadcast: output
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.gatewayPath).toBe('/moq');
    expect(result!.inputBroadcast).toBe('input');
    expect(result!.outputBroadcast).toBe('output');
    expect(result!.hasInputBroadcast).toBe(true);
  });

  it('should report hasInputBroadcast false when input_broadcast is absent', () => {
    const yaml = `
nodes:
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      output_broadcast: output
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result).not.toBeNull();
    expect(result!.hasInputBroadcast).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// detectPeerInputMediaTypes (exercised through extractMoqPeerSettings)
// ---------------------------------------------------------------------------

describe('detectPeerInputMediaTypes (via extractMoqPeerSettings)', () => {
  it('should detect audio input (bare peer reference)', () => {
    const yaml = `
nodes:
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      output_broadcast: output
  opus_decoder:
    kind: audio::opus::decoder
    needs: moq_peer
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.needsAudioInput).toBe(true);
    expect(result!.needsVideoInput).toBe(false);
  });

  it('should detect audio input (explicit .out pin)', () => {
    const yaml = `
nodes:
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      output_broadcast: output
  opus_decoder:
    kind: audio::opus::decoder
    needs: moq_peer.out
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.needsAudioInput).toBe(true);
    expect(result!.needsVideoInput).toBe(false);
  });

  it('should detect video input (.out_1 pin)', () => {
    const yaml = `
nodes:
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      output_broadcast: output
  vp9_decoder:
    kind: video::vp9::decoder
    needs: moq_peer.out_1
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.needsAudioInput).toBe(false);
    expect(result!.needsVideoInput).toBe(true);
  });

  it('should detect both audio and video inputs', () => {
    const yaml = `
nodes:
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      output_broadcast: output
  opus_decoder:
    kind: audio::opus::decoder
    needs: moq_peer.out
  vp9_decoder:
    kind: video::vp9::decoder
    needs: moq_peer.out_1
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.needsAudioInput).toBe(true);
    expect(result!.needsVideoInput).toBe(true);
  });

  it('should handle needs as array', () => {
    const yaml = `
nodes:
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      output_broadcast: output
  mixer:
    kind: audio::mixer
    needs:
      - moq_peer
      - other_source
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.needsAudioInput).toBe(true);
  });

  it('should handle needs as map (record)', () => {
    const yaml = `
nodes:
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      output_broadcast: output
  compositor:
    kind: video::compositor
    needs:
      in: colorbars
      in_1: moq_peer.out_1
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.needsVideoInput).toBe(true);
  });

  it('should report no inputs when no downstream nodes reference the peer', () => {
    const yaml = `
nodes:
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      output_broadcast: output
  gain:
    kind: audio::gain
    needs: some_other_node
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.needsAudioInput).toBe(false);
    expect(result!.needsVideoInput).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// detectPeerOutputMediaTypes (exercised through extractMoqPeerSettings)
// ---------------------------------------------------------------------------

describe('detectPeerOutputMediaTypes (via extractMoqPeerSettings)', () => {
  it('should detect audio output when moq_peer needs an audio:: node', () => {
    const yaml = `
nodes:
  opus_encoder:
    kind: audio::opus::encoder
    needs: gain
  gain:
    kind: audio::gain
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      input_broadcast: input
      output_broadcast: output
    needs: opus_encoder
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.outputsAudio).toBe(true);
    expect(result!.outputsVideo).toBe(false);
  });

  it('should detect video output when moq_peer needs a video:: node', () => {
    const yaml = `
nodes:
  vp9_encoder:
    kind: video::vp9::encoder
    needs: pixel_convert
  pixel_convert:
    kind: video::pixel_convert
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      input_broadcast: input
      output_broadcast: output
    needs: vp9_encoder
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.outputsAudio).toBe(false);
    expect(result!.outputsVideo).toBe(true);
  });

  it('should detect both audio and video outputs', () => {
    const yaml = `
nodes:
  opus_encoder:
    kind: audio::opus::encoder
  vp9_encoder:
    kind: video::vp9::encoder
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      input_broadcast: input
      output_broadcast: output
    needs:
      in: opus_encoder
      in_1: vp9_encoder
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.outputsAudio).toBe(true);
    expect(result!.outputsVideo).toBe(true);
  });

  it('should report no outputs when moq_peer has no needs', () => {
    const yaml = `
nodes:
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      output_broadcast: output
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.outputsAudio).toBe(false);
    expect(result!.outputsVideo).toBe(false);
  });

  it('should handle dotted pin references in moq_peer needs', () => {
    const yaml = `
nodes:
  opus_encoder:
    kind: audio::opus::encoder
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      input_broadcast: input
      output_broadcast: output
    needs: opus_encoder.out
`;
    const result = extractMoqPeerSettings(yaml);
    // "opus_encoder.out" → nodeName is "opus_encoder" → kind is audio:: → outputsAudio
    expect(result!.outputsAudio).toBe(true);
  });

  it('should ignore upstream nodes without a kind', () => {
    const yaml = `
nodes:
  unknown_node: {}
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      output_broadcast: output
    needs: unknown_node
`;
    const result = extractMoqPeerSettings(yaml);
    expect(result!.outputsAudio).toBe(false);
    expect(result!.outputsVideo).toBe(false);
  });
});
