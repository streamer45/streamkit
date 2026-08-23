// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import * as Moq from '@moq/net';
import * as Publish from '@moq/publish';
import { Effect, Signal, type Getter } from '@moq/signals';
import * as Watch from '@moq/watch';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { AV1_CODEC_STRING } from '@/constants/codecs';
import type { PublishTrackConfig } from '@/types/types';

import {
  decideConnect,
  cleanupConnectAttempt,
  waitForSignalValue,
  formatConnectError,
  analyzeSecondaryBroadcastTracks,
  filterSecondaryTracks,
  buildVideoEncoderConfig,
  validateTrackCodecs,
  NULL_MOQ_REFS,
  performConnect,
  VideoRendererHandle,
  AudioEmitterHandle,
  MicrophoneHandle,
  CameraHandle,
  ScreenHandle,
  PublishHandle,
  type ConnectAttempt,
  type ConnectableState,
  type ConnectDecision,
} from './streamStoreHelpers';

/** Factory helper to reduce boilerplate in track-related tests. */
function makeTrack(overrides: Partial<PublishTrackConfig> = {}): PublishTrackConfig {
  return {
    kind: 'video',
    source: 'camera',
    broadcast: null,
    width: null,
    height: null,
    codec: null,
    max_bitrate: null,
    ...overrides,
  };
}

// Mock the MoQ libraries to avoid ESM resolution errors in the test environment.
vi.mock('@moq/net', () => ({
  Connection: { Reload: vi.fn() },
  Path: { from: vi.fn((s: string) => s) },
}));
vi.mock('@moq/publish', () => ({
  Broadcast: vi.fn(),
  Audio: { Encoder: vi.fn() },
  Video: { Capture: vi.fn(), Encoder: vi.fn() },
  Source: { Microphone: vi.fn(), Camera: vi.fn(), Screen: vi.fn() },
}));
vi.mock('@moq/watch', () => {
  const AudioDecoder = vi.fn() as ReturnType<typeof vi.fn> & { supported?: unknown };
  const VideoDecoder = vi.fn() as ReturnType<typeof vi.fn> & { supported?: unknown };
  AudioDecoder.supported = vi.fn();
  VideoDecoder.supported = vi.fn();
  return {
    Broadcast: vi.fn(),
    Sync: vi.fn(),
    Audio: { Source: vi.fn(), Decoder: AudioDecoder, Emitter: vi.fn() },
    Video: { Source: vi.fn(), Decoder: VideoDecoder, Renderer: vi.fn() },
  };
});
vi.mock('@moq/signals', () => {
  class MockSignal<T> {
    #value: T;
    #listeners: ((v: T) => void)[] = [];
    constructor(value: T) {
      this.#value = value;
    }
    peek() {
      return this.#value;
    }
    set(value: T) {
      this.#value = value;
      for (const l of [...this.#listeners]) l(value);
    }
    subscribe(fn: (v: T) => void) {
      this.#listeners.push(fn);
      return () => {
        const idx = this.#listeners.indexOf(fn);
        if (idx >= 0) this.#listeners.splice(idx, 1);
      };
    }
  }
  return {
    Effect: vi.fn(),
    Signal: MockSignal,
  };
});

// decideConnect

describe('decideConnect', () => {
  it('should reject empty server URL', () => {
    const result = decideConnect({
      connectionMode: 'session',
      enablePublish: true,
      enableWatch: true,
      serverUrl: '',
    });

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.errorMessage).toContain('Missing MoQ Gateway URL');
    }
  });

  it('should reject whitespace-only server URL', () => {
    const result = decideConnect({
      connectionMode: 'session',
      enablePublish: true,
      enableWatch: true,
      serverUrl: '   ',
    });

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.errorMessage).toContain('Missing MoQ Gateway URL');
    }
  });

  it('should reject direct mode with neither publish nor watch enabled', () => {
    const result = decideConnect({
      connectionMode: 'direct',
      enablePublish: false,
      enableWatch: false,
      serverUrl: 'http://localhost:4545/moq',
    });

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.errorMessage).toContain('Publish or Watch');
    }
  });

  it('should succeed in session mode with valid URL', () => {
    const result = decideConnect({
      connectionMode: 'session',
      enablePublish: false,
      enableWatch: false,
      serverUrl: 'http://localhost:4545/moq',
    });

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.trimmedServerUrl).toBe('http://localhost:4545/moq');
      // Session mode always watches
      expect(result.shouldWatch).toBe(true);
      expect(result.shouldPublish).toBe(false);
    }
  });

  it('should trim the server URL', () => {
    const result = decideConnect({
      connectionMode: 'session',
      enablePublish: true,
      enableWatch: true,
      serverUrl: '  http://example.com/moq  ',
    });

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.trimmedServerUrl).toBe('http://example.com/moq');
    }
  });

  it('should enable publish when enablePublish is true', () => {
    const result = decideConnect({
      connectionMode: 'direct',
      enablePublish: true,
      enableWatch: false,
      serverUrl: 'http://localhost:4545/moq',
    });

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.shouldPublish).toBe(true);
      expect(result.shouldWatch).toBe(false);
    }
  });

  it('should enable watch in direct mode when enableWatch is true', () => {
    const result = decideConnect({
      connectionMode: 'direct',
      enablePublish: false,
      enableWatch: true,
      serverUrl: 'http://localhost:4545/moq',
    });

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.shouldWatch).toBe(true);
      expect(result.shouldPublish).toBe(false);
    }
  });

  it('should enable both publish and watch', () => {
    const result = decideConnect({
      connectionMode: 'direct',
      enablePublish: true,
      enableWatch: true,
      serverUrl: 'http://localhost:4545/moq',
    });

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.shouldPublish).toBe(true);
      expect(result.shouldWatch).toBe(true);
    }
  });
});

// formatConnectError

describe('formatConnectError', () => {
  it('should format Error instances', () => {
    const msg = formatConnectError(new Error('timeout'));
    expect(msg).toBe('Connection failed: timeout');
  });

  it('should return generic message for non-Error values', () => {
    const msg = formatConnectError('string error');
    expect(msg).toContain('Failed to connect');
  });

  it('should return generic message for null', () => {
    const msg = formatConnectError(null);
    expect(msg).toContain('Failed to connect');
  });
});

// cleanupConnectAttempt

describe('cleanupConnectAttempt', () => {
  function makeAttempt(overrides?: Partial<ConnectAttempt>): ConnectAttempt {
    return { ...NULL_MOQ_REFS, ...overrides };
  }

  it('should call close() on all closeable resources', () => {
    const closes = {
      healthEffect: { close: vi.fn() },
      publish: { close: vi.fn() },
      videoRenderer: { close: vi.fn() },
      videoDecoder: { close: vi.fn() },
      videoSource: { close: vi.fn() },
      audioEmitter: { close: vi.fn() },
      audioDecoder: { close: vi.fn() },
      audioSource: { close: vi.fn() },
      watchSync: { close: vi.fn() },
      watch: { close: vi.fn() },
      connection: { close: vi.fn() },
    };

    cleanupConnectAttempt(makeAttempt(closes as never));

    for (const resource of Object.values(closes)) {
      expect(resource.close).toHaveBeenCalledOnce();
    }
  });

  it('should call close() on microphone when available', () => {
    const mic = { close: vi.fn() };
    cleanupConnectAttempt(makeAttempt({ microphone: mic as never }));
    expect(mic.close).toHaveBeenCalledOnce();
  });

  it('should call close() on camera when available', () => {
    const cam = { close: vi.fn() };
    cleanupConnectAttempt(makeAttempt({ camera: cam as never }));
    expect(cam.close).toHaveBeenCalledOnce();
  });

  it('should call close() on secondaryPublish when available', () => {
    const pub = { close: vi.fn() };
    cleanupConnectAttempt(makeAttempt({ secondaryPublish: pub as never }));
    expect(pub.close).toHaveBeenCalledOnce();
  });

  it('should call close() on secondaryCamera when available', () => {
    const cam = { close: vi.fn() };
    cleanupConnectAttempt(makeAttempt({ secondaryCamera: cam as never }));
    expect(cam.close).toHaveBeenCalledOnce();
  });

  it('should call close() on secondaryScreen when available', () => {
    const scr = { close: vi.fn() };
    cleanupConnectAttempt(makeAttempt({ secondaryScreen: scr as never }));
    expect(scr.close).toHaveBeenCalledOnce();
  });

  it('should disable secondaryCamera via enabled.set(false) when close() is unavailable', () => {
    const cam = { enabled: { set: vi.fn() } };
    cleanupConnectAttempt(makeAttempt({ secondaryCamera: cam as never }));
    expect(cam.enabled.set).toHaveBeenCalledWith(false);
  });

  it('should disable secondaryScreen via enabled.set(false) when close() is unavailable', () => {
    const scr = { enabled: { set: vi.fn() } };
    cleanupConnectAttempt(makeAttempt({ secondaryScreen: scr as never }));
    expect(scr.enabled.set).toHaveBeenCalledWith(false);
  });

  it('should disable microphone via enabled.set(false) when close() is unavailable', () => {
    const mic = { enabled: { set: vi.fn() } };
    cleanupConnectAttempt(makeAttempt({ microphone: mic as never }));
    expect(mic.enabled.set).toHaveBeenCalledWith(false);
  });

  it('should disable camera via enabled.set(false) when close() is unavailable', () => {
    const cam = { enabled: { set: vi.fn() } };
    cleanupConnectAttempt(makeAttempt({ camera: cam as never }));
    expect(cam.enabled.set).toHaveBeenCalledWith(false);
  });

  it('should handle all-null attempt gracefully', () => {
    expect(() => cleanupConnectAttempt(makeAttempt())).not.toThrow();
  });
});

// waitForSignalValue

describe('waitForSignalValue', () => {
  /** Minimal mock that behaves like a @moq/signals Signal<T>. */
  function createMockSignal<T>(initial: T) {
    type Listener = (value: T) => void;
    let current = initial;
    const listeners: Listener[] = [];

    const mock = {
      peek: () => current,
      subscribe: (listener: Listener) => {
        listeners.push(listener);
        return () => {
          const idx = listeners.indexOf(listener);
          if (idx >= 0) listeners.splice(idx, 1);
        };
      },
      set: (value: T) => {
        current = value;
        for (const l of listeners) l(value);
      },
    };

    // Cast to Getter<T> — only peek/subscribe are needed by waitForSignalValue.
    return mock as typeof mock & Getter<T>;
  }

  it('should resolve immediately when predicate matches initial value', async () => {
    const signal = createMockSignal(42);
    const value = await waitForSignalValue(signal, (v) => v === 42, 1_000, 'timeout');
    expect(value).toBe(42);
  });

  it('should resolve when signal value changes to match predicate', async () => {
    const signal = createMockSignal<string | undefined>(undefined);

    const promise = waitForSignalValue(signal, (v) => v !== undefined, 5_000, 'should not timeout');

    // Simulate delayed signal emission
    setTimeout(() => signal.set('connected'), 50);

    const value = await promise;
    expect(value).toBe('connected');
  });

  it('should reject with timeout message when predicate never matches', async () => {
    vi.useFakeTimers();

    const signal = createMockSignal(0);
    const promise = waitForSignalValue(signal, (v) => v > 100, 3_000, 'timed out waiting');

    vi.advanceTimersByTime(3_000);

    await expect(promise).rejects.toThrow('timed out waiting');

    vi.useRealTimers();
  });

  it('should reject immediately when abortSignal is already aborted', async () => {
    const signal = createMockSignal(0);
    const abort = new AbortController();
    abort.abort();

    await expect(
      waitForSignalValue(signal, (v) => v > 0, 5_000, 'timeout', abort.signal)
    ).rejects.toThrow('Aborted');
  });

  it('should reject with AbortError when abortSignal fires during wait', async () => {
    const signal = createMockSignal(0);
    const abort = new AbortController();

    const promise = waitForSignalValue(signal, (v) => v > 0, 5_000, 'timeout', abort.signal);

    // Abort before the signal value changes
    abort.abort();

    await expect(promise).rejects.toThrow('Aborted');
  });

  it('should not reject on abort if predicate already matched', async () => {
    const signal = createMockSignal(42);
    const abort = new AbortController();

    // Predicate matches initial value — resolves synchronously before abort
    const value = await waitForSignalValue(signal, (v) => v === 42, 5_000, 'timeout', abort.signal);
    expect(value).toBe(42);

    // Aborting after resolution should be harmless
    abort.abort();
  });

  it('should clean up subscription when abortSignal fires', async () => {
    const signal = createMockSignal<number>(0);
    const abort = new AbortController();

    const promise = waitForSignalValue(signal, (v) => v > 100, 5_000, 'timeout', abort.signal);
    abort.abort();

    await expect(promise).rejects.toThrow('Aborted');

    // After abort, emitting new values should be harmless (no dangling listeners)
    expect(() => signal.set(200)).not.toThrow();
  });
});

// NULL_MOQ_REFS

describe('NULL_MOQ_REFS', () => {
  it('should have all expected keys set to null', () => {
    const expectedKeys = [
      'publish',
      'watch',
      'watchSync',
      'audioSource',
      'audioDecoder',
      'audioEmitter',
      'videoSource',
      'videoDecoder',
      'videoRenderer',
      'connection',
      'microphone',
      'camera',
      'screen',
      'healthEffect',
      'secondaryPublish',
      'secondaryCamera',
      'secondaryScreen',
    ];

    for (const key of expectedKeys) {
      expect(NULL_MOQ_REFS).toHaveProperty(key, null);
    }
  });
});

// analyzeSecondaryBroadcastTracks

describe('analyzeSecondaryBroadcastTracks', () => {
  it('returns needsVideo=true and camera source for a single camera video track', () => {
    const result = analyzeSecondaryBroadcastTracks('cam-input', [
      {
        kind: 'video',
        source: 'camera',
        broadcast: 'cam-input',
        width: null,
        height: null,
        codec: null,
        max_bitrate: null,
      },
    ]);
    expect(result.needsVideo).toBe(true);
    expect(result.videoSourceType).toBe('camera');
    expect(result.warnings).toHaveLength(0);
  });

  it('returns needsVideo=true and screen source for a screen video track', () => {
    const result = analyzeSecondaryBroadcastTracks('screen2', [
      {
        kind: 'video',
        source: 'screen',
        broadcast: 'screen2',
        width: null,
        height: null,
        codec: null,
        max_bitrate: null,
      },
    ]);
    expect(result.needsVideo).toBe(true);
    expect(result.videoSourceType).toBe('screen');
    expect(result.warnings).toHaveLength(0);
  });

  it('returns needsVideo=false when only audio tracks are present', () => {
    const result = analyzeSecondaryBroadcastTracks('audio-only', [
      {
        kind: 'audio',
        source: 'microphone',
        broadcast: 'audio-only',
        width: null,
        height: null,
        codec: null,
        max_bitrate: null,
      },
    ]);
    expect(result.needsVideo).toBe(false);
    expect(result.videoSourceType).toBe('camera'); // default fallback
    expect(result.warnings).toHaveLength(1);
    expect(result.warnings[0]).toContain('audio tracks which are not yet supported');
  });

  it('warns when audio tracks are present alongside video', () => {
    const result = analyzeSecondaryBroadcastTracks('mixed', [
      {
        kind: 'video',
        source: 'camera',
        broadcast: 'mixed',
        width: null,
        height: null,
        codec: null,
        max_bitrate: null,
      },
      {
        kind: 'audio',
        source: 'microphone',
        broadcast: 'mixed',
        width: null,
        height: null,
        codec: null,
        max_bitrate: null,
      },
    ]);
    expect(result.needsVideo).toBe(true);
    expect(result.warnings).toHaveLength(1);
    expect(result.warnings[0]).toContain('audio tracks which are not yet supported');
  });

  it('warns when multiple video tracks are present', () => {
    const result = analyzeSecondaryBroadcastTracks('multi-video', [
      {
        kind: 'video',
        source: 'camera',
        broadcast: 'multi-video',
        width: null,
        height: null,
        codec: null,
        max_bitrate: null,
      },
      {
        kind: 'video',
        source: 'screen',
        broadcast: 'multi-video',
        width: null,
        height: null,
        codec: null,
        max_bitrate: null,
      },
    ]);
    expect(result.needsVideo).toBe(true);
    expect(result.videoSourceType).toBe('camera'); // first track wins
    expect(result.warnings).toHaveLength(1);
    expect(result.warnings[0]).toContain('2 video tracks');
    expect(result.warnings[0]).toContain('only the first is used');
  });

  it('collects both audio and multi-video warnings', () => {
    const result = analyzeSecondaryBroadcastTracks('all-warnings', [
      {
        kind: 'video',
        source: 'camera',
        broadcast: 'all-warnings',
        width: null,
        height: null,
        codec: null,
        max_bitrate: null,
      },
      {
        kind: 'video',
        source: 'screen',
        broadcast: 'all-warnings',
        width: null,
        height: null,
        codec: null,
        max_bitrate: null,
      },
      {
        kind: 'audio',
        source: 'microphone',
        broadcast: 'all-warnings',
        width: null,
        height: null,
        codec: null,
        max_bitrate: null,
      },
    ]);
    expect(result.warnings).toHaveLength(2);
  });

  it('returns needsVideo=false for empty tracks', () => {
    const result = analyzeSecondaryBroadcastTracks('empty', []);
    expect(result.needsVideo).toBe(false);
    expect(result.warnings).toHaveLength(0);
  });
});

// filterSecondaryTracks

describe('filterSecondaryTracks', () => {
  const allTracks = [
    {
      kind: 'audio' as const,
      source: 'microphone' as const,
      broadcast: null,
      width: null,
      height: null,
      codec: null,
      max_bitrate: null,
    },
    {
      kind: 'video' as const,
      source: 'screen' as const,
      broadcast: null,
      width: null,
      height: null,
      codec: null,
      max_bitrate: null,
    },
    {
      kind: 'video' as const,
      source: 'camera' as const,
      broadcast: 'cam-input',
      width: null,
      height: null,
      codec: null,
      max_bitrate: null,
    },
  ];

  it('returns tracks whose broadcast matches the secondary name', () => {
    const result = filterSecondaryTracks(allTracks, 'screen-input', 'cam-input');
    expect(result).toHaveLength(1);
    expect(result[0].source).toBe('camera');
  });

  it('defaults null-broadcast tracks to primaryBroadcast', () => {
    const result = filterSecondaryTracks(allTracks, 'screen-input', 'screen-input');
    expect(result).toHaveLength(2);
    expect(result.map((t) => t.source)).toEqual(['microphone', 'screen']);
  });

  it('returns empty array when no tracks match', () => {
    const result = filterSecondaryTracks(allTracks, 'screen-input', 'nonexistent');
    expect(result).toHaveLength(0);
  });

  it('handles all-explicit broadcasts correctly', () => {
    const explicitTracks = [
      {
        kind: 'video' as const,
        source: 'screen' as const,
        broadcast: 'a',
        width: null,
        height: null,
        codec: null,
        max_bitrate: null,
      },
      {
        kind: 'video' as const,
        source: 'camera' as const,
        broadcast: 'b',
        width: null,
        height: null,
        codec: null,
        max_bitrate: null,
      },
    ];
    expect(filterSecondaryTracks(explicitTracks, 'a', 'b')).toHaveLength(1);
    expect(filterSecondaryTracks(explicitTracks, 'a', 'a')).toHaveLength(1);
  });
});

// buildVideoEncoderConfig

describe('buildVideoEncoderConfig', () => {
  it('returns default vp09 codec when no track is provided', () => {
    const result = buildVideoEncoderConfig();
    expect(result.encoderConfig.codec).toBe('vp09');
    expect(result.encoderConfig.maxPixels).toBeUndefined();
    expect(result.encoderConfig.maxBitrate).toBeUndefined();
    expect(result.constraints).toBeUndefined();
  });

  it('returns default vp09 codec when track has no codec set', () => {
    const result = buildVideoEncoderConfig(makeTrack());
    expect(result.encoderConfig.codec).toBe('vp09');
  });

  it('maps vp9 to vp09 WebCodecs codec string', () => {
    const result = buildVideoEncoderConfig(makeTrack({ codec: 'vp9' }));
    expect(result.encoderConfig.codec).toBe('vp09');
  });

  it('maps av1 to the shared AV1_CODEC_STRING WebCodecs codec string', () => {
    const result = buildVideoEncoderConfig(makeTrack({ codec: 'av1' }));
    expect(result.encoderConfig.codec).toBe(AV1_CODEC_STRING);
  });

  it('throws for unrecognized codec values', () => {
    expect(() => buildVideoEncoderConfig(makeTrack({ codec: 'h264' }))).toThrow(
      /Unsupported video codec 'h264'/
    );
  });

  it('computes maxPixels from width × height', () => {
    const result = buildVideoEncoderConfig(
      makeTrack({ source: 'screen', width: 1280, height: 720 })
    );
    expect(result.encoderConfig.maxPixels).toBe(1280 * 720);
  });

  it('does not set maxPixels when only width is provided', () => {
    const result = buildVideoEncoderConfig(makeTrack({ width: 1280 }));
    expect(result.encoderConfig.maxPixels).toBeUndefined();
  });

  it('does not set maxPixels when only height is provided', () => {
    const result = buildVideoEncoderConfig(makeTrack({ height: 720 }));
    expect(result.encoderConfig.maxPixels).toBeUndefined();
  });

  it('converts max_bitrate from kbps to bps', () => {
    const result = buildVideoEncoderConfig(makeTrack({ max_bitrate: 2500 }));
    expect(result.encoderConfig.maxBitrate).toBe(2_500_000);
  });

  it('returns capture constraints with width and height', () => {
    const result = buildVideoEncoderConfig(
      makeTrack({ source: 'screen', width: 640, height: 480, codec: 'vp9' })
    );
    expect(result.constraints).toEqual({ width: 640, height: 480 });
  });

  it('returns constraints with only width when height is null', () => {
    const result = buildVideoEncoderConfig(makeTrack({ width: 1920 }));
    expect(result.constraints).toEqual({ width: 1920 });
  });

  it('handles all fields set together', () => {
    const result = buildVideoEncoderConfig(
      makeTrack({
        source: 'screen',
        broadcast: 'my-broadcast',
        width: 1920,
        height: 1080,
        codec: 'vp9',
        max_bitrate: 5000,
      })
    );
    expect(result.encoderConfig).toEqual({
      codec: 'vp09',
      maxPixels: 1920 * 1080,
      maxBitrate: 5_000_000,
    });
    expect(result.constraints).toEqual({ width: 1920, height: 1080 });
  });

  it('handles null track', () => {
    const result = buildVideoEncoderConfig(null);
    expect(result.encoderConfig.codec).toBe('vp09');
    expect(result.constraints).toBeUndefined();
  });

  it('skips maxPixels when width is zero', () => {
    const result = buildVideoEncoderConfig(makeTrack({ width: 0, height: 720 }));
    expect(result.encoderConfig.maxPixels).toBeUndefined();
    // Zero-valued dimensions should also be excluded from capture constraints
    // to avoid OverconstrainedError in getUserMedia/getDisplayMedia.
    expect(result.constraints).toEqual({ height: 720 });
  });

  it('skips maxPixels when height is zero', () => {
    const result = buildVideoEncoderConfig(makeTrack({ width: 1280, height: 0 }));
    expect(result.encoderConfig.maxPixels).toBeUndefined();
    expect(result.constraints).toEqual({ width: 1280 });
  });

  it('skips maxBitrate when max_bitrate is zero', () => {
    const result = buildVideoEncoderConfig(makeTrack({ max_bitrate: 0 }));
    expect(result.encoderConfig.maxBitrate).toBeUndefined();
  });

  it('logs warning for partial dimensions (width only)', () => {
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    buildVideoEncoderConfig(makeTrack({ width: 1280 }));
    const partialCalls = logSpy.mock.calls.filter((args) =>
      args.some((a) => typeof a === 'string' && a.includes('partial dimensions'))
    );
    expect(partialCalls).toHaveLength(1);
    logSpy.mockRestore();
  });
});

// validateTrackCodecs

// validateTrackCodecs uses `logger.warn` from tslog. In this test environment,
// tslog routes all output through `console.log` (not `console.warn`), so we
// spy on `console.log` and filter for the 'unrecognized' substring.
describe('validateTrackCodecs', () => {
  it('does not warn for recognized video codec vp9', () => {
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    validateTrackCodecs([makeTrack({ codec: 'vp9' })]);
    const unrecognizedCalls = logSpy.mock.calls.filter((args) =>
      args.some((a) => typeof a === 'string' && a.includes('unrecognized'))
    );
    expect(unrecognizedCalls).toHaveLength(0);
    logSpy.mockRestore();
  });

  it('does not warn for recognized video codec av1', () => {
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    validateTrackCodecs([makeTrack({ codec: 'av1' })]);
    const unrecognizedCalls = logSpy.mock.calls.filter((args) =>
      args.some((a) => typeof a === 'string' && a.includes('unrecognized'))
    );
    expect(unrecognizedCalls).toHaveLength(0);
    logSpy.mockRestore();
  });

  it('does not warn for recognized audio codec opus', () => {
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    validateTrackCodecs([makeTrack({ kind: 'audio', source: 'microphone', codec: 'opus' })]);
    const unrecognizedCalls = logSpy.mock.calls.filter((args) =>
      args.some((a) => typeof a === 'string' && a.includes('unrecognized'))
    );
    expect(unrecognizedCalls).toHaveLength(0);
    logSpy.mockRestore();
  });

  it('does not warn when codec is null', () => {
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    validateTrackCodecs([makeTrack()]);
    const unrecognizedCalls = logSpy.mock.calls.filter((args) =>
      args.some((a) => typeof a === 'string' && a.includes('unrecognized'))
    );
    expect(unrecognizedCalls).toHaveLength(0);
    logSpy.mockRestore();
  });

  it('warns for unrecognized video codec', () => {
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    validateTrackCodecs([makeTrack({ source: 'screen', codec: 'h264' })]);
    const unrecognizedCalls = logSpy.mock.calls.filter((args) =>
      args.some((a) => typeof a === 'string' && a.includes('unrecognized'))
    );
    expect(unrecognizedCalls).toHaveLength(1);
    expect(
      unrecognizedCalls[0]?.some((a: unknown) => typeof a === 'string' && a.includes('h264'))
    ).toBe(true);
    logSpy.mockRestore();
  });

  it('warns for unrecognized audio codec', () => {
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    validateTrackCodecs([makeTrack({ kind: 'audio', source: 'microphone', codec: 'aac' })]);
    const unrecognizedCalls = logSpy.mock.calls.filter((args) =>
      args.some((a) => typeof a === 'string' && a.includes('unrecognized'))
    );
    expect(unrecognizedCalls).toHaveLength(1);
    expect(
      unrecognizedCalls[0]?.some((a: unknown) => typeof a === 'string' && a.includes('aac'))
    ).toBe(true);
    logSpy.mockRestore();
  });
});

// performConnect

describe('performConnect', () => {
  /** Minimal mock that behaves like a @moq/signals Signal<T>. */
  function createMockSignal<T>(initial: T) {
    type Listener = (value: T) => void;
    let current = initial;
    const listeners: Listener[] = [];

    const mock = {
      peek: () => current,
      subscribe: (listener: Listener) => {
        listeners.push(listener);
        return () => {
          const idx = listeners.indexOf(listener);
          if (idx >= 0) listeners.splice(idx, 1);
        };
      },
      set: (value: T) => {
        current = value;
        for (const l of [...listeners]) l(value);
      },
    };

    return mock as typeof mock & Getter<T>;
  }

  type EffectSub = [signal: unknown, cb: (v: unknown) => void];

  /** Mock Effect that records subscriptions without auto-invoking callbacks.
   *  Tests trigger callbacks manually via `_subs` to simulate signal changes. */
  function createMockEffect() {
    const subs: EffectSub[] = [];
    return {
      subscribe: vi.fn((signal: unknown, cb: (v: unknown) => void) => {
        subs.push([signal, cb]);
      }),
      close: vi.fn(),
      _subs: subs,
    };
  }

  function asMock(fn: unknown): ReturnType<typeof vi.fn> {
    return fn as ReturnType<typeof vi.fn>;
  }

  function makeState(overrides?: Partial<ConnectableState>): ConnectableState {
    return {
      connectionMode: 'session',
      enablePublish: false,
      enableWatch: true,
      serverUrl: 'http://localhost:4545/moq',
      moqToken: '',
      inputBroadcast: 'input',
      outputBroadcast: 'output',
      pipelineNeedsAudio: false,
      pipelineNeedsVideo: false,
      pipelineOutputsAudio: true,
      pipelineOutputsVideo: true,
      isExternalRelay: false,
      videoSourceType: 'camera',
      tracks: [],
      publishBroadcasts: [],
      status: 'connecting',
      errorMessage: '',
      isMicEnabled: false,
      isCameraEnabled: false,
      micStatus: 'disabled',
      cameraStatus: 'disabled',
      watchStatus: 'disabled',
      isSecondaryCameraEnabled: false,
      secondaryCameraStatus: 'disabled',
      connectingStep: '',
      ...overrides,
    } as ConnectableState;
  }

  type OkDecision = Extract<ConnectDecision, { ok: true }>;
  function makeOkDecision(overrides?: Partial<OkDecision>): OkDecision {
    return {
      ok: true as const,
      trimmedServerUrl: 'http://localhost:4545/moq',
      shouldWatch: true,
      shouldPublish: false,
      ...overrides,
    };
  }

  type SetterFn = (partial: Partial<ConnectableState>) => void;

  let mockEffect: ReturnType<typeof createMockEffect>;
  let connEstablished: ReturnType<typeof createMockSignal<object | undefined>>;
  let connStatus: ReturnType<typeof createMockSignal<string>>;
  let watchStatusSig: ReturnType<typeof createMockSignal<string>>;
  let state: ConnectableState;
  let set: ReturnType<typeof vi.fn<SetterFn>>;
  let get: () => ConnectableState;

  /** Configure Publish.* mocks for a publish path test. Returns the underlying
   *  mock signals so tests can manipulate them (e.g. trigger the mic-ready
   *  Effect subscription before the warning threshold elapses). */
  function setupPublishMocks(opts?: {
    micSourceReady?: boolean;
    camSourceReady?: boolean;
    catalogReady?: boolean;
  }) {
    const { micSourceReady = true, camSourceReady = true, catalogReady = true } = opts ?? {};

    const micSource = createMockSignal<object | undefined>(micSourceReady ? {} : undefined);
    const camSource = createMockSignal<object | undefined>(camSourceReady ? {} : undefined);
    const catalog = createMockSignal<object | undefined>(
      catalogReady ? { ready: true } : undefined
    );
    const audioEncoderInputs: {
      enabled?: { peek(): boolean };
      codec?: { mime: string; usedtx?: boolean };
    }[] = [];

    asMock(Publish.Source.Microphone).mockImplementation(function () {
      return { out: { source: micSource }, close: vi.fn() };
    });
    asMock(Publish.Source.Camera).mockImplementation(function () {
      return { out: { source: camSource }, close: vi.fn() };
    });
    asMock(Publish.Video.Capture).mockImplementation(function () {
      return { out: { display: createMockSignal(undefined) }, close: vi.fn() };
    });
    asMock(Publish.Video.Encoder).mockImplementation(function () {
      return { out: { catalog }, close: vi.fn() };
    });
    asMock(Publish.Audio.Encoder).mockImplementation(function (
      _name: string,
      inputs: {
        enabled?: { peek(): boolean };
        codec?: { mime: string; usedtx?: boolean };
      }
    ) {
      audioEncoderInputs.push(inputs);
      return { close: vi.fn() };
    });
    asMock(Publish.Broadcast).mockImplementation(function () {
      return { close: vi.fn() };
    });

    return { micSource, camSource, catalog, audioEncoderInputs };
  }

  beforeEach(() => {
    mockEffect = createMockEffect();
    connEstablished = createMockSignal<object | undefined>({});
    connStatus = createMockSignal<string>('connected');
    watchStatusSig = createMockSignal<string>('live');

    state = makeState();
    set = vi.fn<SetterFn>((partial) => {
      Object.assign(state, partial);
    });
    get = () => state;

    asMock(Moq.Connection.Reload).mockImplementation(function () {
      return { established: connEstablished, status: connStatus, close: vi.fn() };
    });
    asMock(Effect).mockImplementation(function () {
      return mockEffect;
    });
    asMock(Watch.Broadcast).mockImplementation(function () {
      return { out: { status: watchStatusSig }, close: vi.fn() };
    });
    asMock(Watch.Sync).mockImplementation(function () {
      return { close: vi.fn() };
    });
    asMock(Watch.Audio.Source).mockImplementation(function () {
      return { out: { jitter: createMockSignal(undefined) }, close: vi.fn() };
    });
    asMock(Watch.Audio.Decoder).mockImplementation(function () {
      return { close: vi.fn() };
    });
    asMock(Watch.Audio.Emitter).mockImplementation(function () {
      return { close: vi.fn() };
    });
    asMock(Watch.Video.Source).mockImplementation(function () {
      return { out: { jitter: createMockSignal(undefined) }, close: vi.fn() };
    });
    asMock(Watch.Video.Decoder).mockImplementation(function () {
      return { close: vi.fn() };
    });
    asMock(Watch.Video.Renderer).mockImplementation(function () {
      return { close: vi.fn() };
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it('returns false when abortSignal is already aborted', async () => {
    const abort = new AbortController();
    abort.abort();

    const result = await performConnect(state, makeOkDecision(), get, set, abort.signal);

    expect(result).toBe(false);
    // No error state should be set — caller already knows about the abort.
    expect(state.errorMessage).toBe('');
  });

  it('transitions to connected for a watch-only connection', async () => {
    const decision = makeOkDecision({ shouldWatch: true, shouldPublish: false });
    const abort = new AbortController();

    const result = await performConnect(state, decision, get, set, abort.signal);

    expect(result).toBe(true);
    expect(state.status).toBe('connected');
    expect(state.connectingStep).toBe('');
    expect(state.isMicEnabled).toBe(false);
    expect(state.isCameraEnabled).toBe(false);
  });

  it('syncs watchStatus from the broadcast signal during watch setup', async () => {
    watchStatusSig = createMockSignal<string>('pending');
    asMock(Watch.Broadcast).mockImplementation(function () {
      return { out: { status: watchStatusSig }, close: vi.fn() };
    });

    const decision = makeOkDecision({ shouldWatch: true, shouldPublish: false });
    const abort = new AbortController();

    await performConnect(state, decision, get, set, abort.signal);

    const watchCall = set.mock.calls.find(
      (call: unknown[]) => (call[0] as Record<string, unknown>).watchStatus === 'pending'
    );
    expect(watchCall).toBeDefined();
  });

  it('transitions to connected with mic and camera enabled for publish + watch', async () => {
    state = makeState({ pipelineNeedsAudio: true, pipelineNeedsVideo: true });
    setupPublishMocks();

    const decision = makeOkDecision({ shouldWatch: true, shouldPublish: true });
    const abort = new AbortController();

    const result = await performConnect(state, decision, get, set, abort.signal);

    expect(result).toBe(true);
    expect(state.status).toBe('connected');
    expect(state.isMicEnabled).toBe(true);
    expect(state.isCameraEnabled).toBe(true);
  });

  it('defers audio publishing until the video catalog is ready', async () => {
    state = makeState({ pipelineNeedsAudio: true, pipelineNeedsVideo: true });
    const { audioEncoderInputs } = setupPublishMocks();

    const decision = makeOkDecision({ shouldWatch: false, shouldPublish: true });
    const abort = new AbortController();

    await performConnect(state, decision, get, set, abort.signal);

    // For combined audio+video publish, audio is started disabled and explicitly
    // re-enabled after the video catalog is observed (prevents the ~0.7s A/V
    // desync caused by VP9 encoder startup).
    expect(audioEncoderInputs).toHaveLength(1);
    expect(audioEncoderInputs[0]?.enabled?.peek()).toBe(true);
    expect(audioEncoderInputs[0]?.codec).toEqual({ mime: 'opus', usedtx: false });
  });

  it('sets micStatus to requesting when mic source is initially unavailable', async () => {
    state = makeState({ pipelineNeedsAudio: true, pipelineNeedsVideo: false });
    setupPublishMocks({ micSourceReady: false });

    const decision = makeOkDecision({ shouldWatch: false, shouldPublish: true });
    const abort = new AbortController();

    await performConnect(state, decision, get, set, abort.signal);

    const micCall = set.mock.calls.find(
      (call: unknown[]) => (call[0] as Record<string, unknown>).micStatus === 'requesting'
    );
    expect(micCall).toBeDefined();
  });

  it('sets error state when the relay connection times out', async () => {
    vi.useFakeTimers();

    connEstablished = createMockSignal<object | undefined>(undefined);
    asMock(Moq.Connection.Reload).mockImplementation(function () {
      return { established: connEstablished, status: connStatus, close: vi.fn() };
    });

    const decision = makeOkDecision({ shouldWatch: true, shouldPublish: false });
    const abort = new AbortController();

    const promise = performConnect(state, decision, get, set, abort.signal);
    // performConnect awaits the connection signal with a 12s timeout
    await vi.advanceTimersByTimeAsync(12_000);
    const result = await promise;

    expect(result).toBe(false);
    expect(state.status).toBe('disconnected');
    expect(state.errorMessage).toContain('Connection failed:');
    expect(state.errorMessage).toContain('Timed out');
  });

  it('returns false without overwriting state when aborted mid-connect', async () => {
    connEstablished = createMockSignal<object | undefined>(undefined);
    asMock(Moq.Connection.Reload).mockImplementation(function () {
      return { established: connEstablished, status: connStatus, close: vi.fn() };
    });

    const abort = new AbortController();
    const promise = performConnect(state, makeOkDecision(), get, set, abort.signal);

    abort.abort();
    const result = await promise;

    expect(result).toBe(false);
    // Aborted attempts must not overwrite the store with a disconnected/error
    // state — that would clobber a newer connect attempt or a manual disconnect.
    expect(state.errorMessage).toBe('');
    expect(state.status).not.toBe('disconnected');
  });

  it('cleans up resources and reports error when watch setup throws', async () => {
    const connClose = vi.fn();
    asMock(Moq.Connection.Reload).mockImplementation(function () {
      return { established: connEstablished, status: connStatus, close: connClose };
    });
    asMock(Watch.Broadcast).mockImplementation(function () {
      throw new Error('Watch init failed');
    });

    const decision = makeOkDecision({ shouldWatch: true, shouldPublish: false });
    const abort = new AbortController();

    const result = await performConnect(state, decision, get, set, abort.signal);

    expect(result).toBe(false);
    expect(state.status).toBe('disconnected');
    expect(state.errorMessage).toContain('Watch init failed');
    expect(connClose).toHaveBeenCalled();
  });

  describe('schedulePostConnectWarnings — watch broadcast', () => {
    it('warns when the watch broadcast is not live after the 10s threshold', async () => {
      vi.useFakeTimers();

      watchStatusSig = createMockSignal<string>('pending');
      asMock(Watch.Broadcast).mockImplementation(function () {
        return { out: { status: watchStatusSig }, close: vi.fn() };
      });

      const decision = makeOkDecision({ shouldWatch: true, shouldPublish: false });
      const abort = new AbortController();

      await performConnect(state, decision, get, set, abort.signal);
      expect(state.status).toBe('connected');

      vi.advanceTimersByTime(10_000);

      expect(state.errorMessage).toContain('not live yet');
    });

    it('does not warn when the watch broadcast is already live', async () => {
      vi.useFakeTimers();

      const decision = makeOkDecision({ shouldWatch: true, shouldPublish: false });
      const abort = new AbortController();

      await performConnect(state, decision, get, set, abort.signal);

      const errorBefore = state.errorMessage;
      vi.advanceTimersByTime(10_000);

      expect(state.errorMessage).toBe(errorBefore);
    });

    it('skips the watch-broadcast warning when disconnected before threshold', async () => {
      vi.useFakeTimers();

      watchStatusSig = createMockSignal<string>('pending');
      asMock(Watch.Broadcast).mockImplementation(function () {
        return { out: { status: watchStatusSig }, close: vi.fn() };
      });

      const decision = makeOkDecision({ shouldWatch: true, shouldPublish: false });
      const abort = new AbortController();

      await performConnect(state, decision, get, set, abort.signal);
      expect(state.status).toBe('connected');

      // Simulate the store transitioning out of `connected` before the timer fires
      // — the warning callback should bail out without setting an error message.
      state.status = 'disconnected';
      state.errorMessage = '';

      vi.advanceTimersByTime(10_000);

      expect(state.errorMessage).toBe('');
    });
  });

  describe('schedulePostConnectWarnings — microphone', () => {
    it('warns when the microphone is not ready after the 10s threshold', async () => {
      vi.useFakeTimers();

      state = makeState({ pipelineNeedsAudio: true, pipelineNeedsVideo: false });
      setupPublishMocks({ micSourceReady: false });

      const decision = makeOkDecision({ shouldWatch: false, shouldPublish: true });
      const abort = new AbortController();

      await performConnect(state, decision, get, set, abort.signal);
      expect(state.status).toBe('connected');

      vi.advanceTimersByTime(10_000);

      expect(state.micStatus).toBe('error');
      expect(state.errorMessage).toContain('microphone is not available');
    });

    it('does not warn when the microphone source is immediately available', async () => {
      vi.useFakeTimers();

      state = makeState({ pipelineNeedsAudio: true, pipelineNeedsVideo: false });
      setupPublishMocks({ micSourceReady: true });

      const decision = makeOkDecision({ shouldWatch: false, shouldPublish: true });
      const abort = new AbortController();

      await performConnect(state, decision, get, set, abort.signal);

      vi.advanceTimersByTime(10_000);

      expect(state.micStatus).not.toBe('error');
    });

    it('does not warn when the microphone becomes ready before the threshold', async () => {
      vi.useFakeTimers();

      state = makeState({ pipelineNeedsAudio: true, pipelineNeedsVideo: false });
      const { micSource } = setupPublishMocks({ micSourceReady: false });

      const decision = makeOkDecision({ shouldWatch: false, shouldPublish: true });
      const abort = new AbortController();

      await performConnect(state, decision, get, set, abort.signal);

      // Drive the Effect subscription that was registered for the mic source —
      // the warning closure should observe `wasEverReady` becoming true.
      for (const [sig, cb] of mockEffect._subs) {
        if (sig === micSource) cb({});
      }

      vi.advanceTimersByTime(10_000);

      expect(state.micStatus).not.toBe('error');
    });
  });

  describe('setupConnectionStatusSync — connection health updates', () => {
    it('sets a disconnect error when the connection drops after being connected', async () => {
      const decision = makeOkDecision({ shouldWatch: true, shouldPublish: false });
      const abort = new AbortController();

      await performConnect(state, decision, get, set, abort.signal);
      expect(state.status).toBe('connected');

      // Trigger the connection-status subscription registered by
      // setupConnectionStatusSync. With the store already in `connected`, a
      // drop should propagate to `disconnected` with a helpful error message.
      const connSub = mockEffect._subs.find(([sig]) => sig === connStatus);
      expect(connSub).toBeDefined();
      connSub![1]('disconnected');

      expect(state.status).toBe('disconnected');
      expect(state.errorMessage).toContain('Disconnected from MoQ gateway');
    });

    it('maps connecting relay status to a connecting store status', async () => {
      const decision = makeOkDecision({ shouldWatch: true, shouldPublish: false });
      const abort = new AbortController();

      await performConnect(state, decision, get, set, abort.signal);
      expect(state.status).toBe('connected');

      const connSub = mockEffect._subs.find(([sig]) => sig === connStatus);
      connSub![1]('connecting');

      expect(state.status).toBe('connecting');
      // Connecting-mid-session is not an error condition — no error message.
      expect(state.errorMessage).not.toContain('Disconnected');
    });
  });
});

// Resource handles

describe('resource handles', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('VideoRendererHandle wires the canvas signal into the renderer and closes it', () => {
    const close = vi.fn();
    vi.mocked(Watch.Video.Renderer).mockImplementation(function (this: { close: typeof close }) {
      this.close = close;
    } as never);

    const decoder = {} as Watch.Video.Decoder;
    const handle = new VideoRendererHandle(decoder);

    expect(Watch.Video.Renderer).toHaveBeenCalledWith(decoder, { canvas: handle.canvas });
    handle.close();
    expect(close).toHaveBeenCalled();
  });

  it('AudioEmitterHandle wires muted/volume signals into the emitter and closes it', () => {
    const close = vi.fn();
    vi.mocked(Watch.Audio.Emitter).mockImplementation(function (this: { close: typeof close }) {
      this.close = close;
    } as never);

    const decoder = {} as Watch.Audio.Decoder;
    const handle = new AudioEmitterHandle(decoder);

    expect(Watch.Audio.Emitter).toHaveBeenCalledWith(decoder, {
      muted: handle.muted,
      volume: handle.volume,
    });
    expect(handle.muted.peek()).toBe(false);
    expect(handle.volume.peek()).toBe(0.5);
    handle.close();
    expect(close).toHaveBeenCalled();
  });

  it.each([
    ['MicrophoneHandle', Publish.Source.Microphone, () => new MicrophoneHandle()],
    ['CameraHandle', Publish.Source.Camera, () => new CameraHandle()],
    ['ScreenHandle', Publish.Source.Screen, () => new ScreenHandle()],
  ] as const)(
    '%s exposes the capture source getter and closes the inner source',
    (_name, ctor, make) => {
      const close = vi.fn();
      const source = { kind: 'source' };
      vi.mocked(ctor).mockImplementation(function (this: {
        close: typeof close;
        out: { source: typeof source };
      }) {
        this.close = close;
        this.out = { source };
      } as never);

      const handle = make();

      expect(handle.enabled.peek()).toBe(true);
      expect(handle.source).toBe(source);
      handle.close();
      expect(close).toHaveBeenCalled();
    }
  );

  it('PublishHandle closes every owned resource', () => {
    const broadcast = { close: vi.fn() };
    const capture = { close: vi.fn() };
    const video = { close: vi.fn() };
    const encoder = { close: vi.fn() };
    const enabled = new Signal(true);

    const handle = new PublishHandle({
      broadcast: broadcast as unknown as Publish.Broadcast,
      capture: capture as unknown as Publish.Video.Capture,
      video: video as unknown as Publish.Video.Encoder,
      audio: { enabled, encoder: encoder as unknown as Publish.Audio.Encoder },
    });

    handle.close();
    expect(video.close).toHaveBeenCalled();
    expect(encoder.close).toHaveBeenCalled();
    expect(capture.close).toHaveBeenCalled();
    expect(broadcast.close).toHaveBeenCalled();
  });

  it('PublishHandle tolerates missing optional resources on close', () => {
    const broadcast = { close: vi.fn() };
    const handle = new PublishHandle({ broadcast: broadcast as unknown as Publish.Broadcast });

    handle.close();
    expect(broadcast.close).toHaveBeenCalled();
  });
});
