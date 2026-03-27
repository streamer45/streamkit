// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { Getter } from '@moq/signals';
import { describe, expect, it, vi } from 'vitest';

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
  type ConnectAttempt,
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
vi.mock('@moq/hang', () => ({
  Moq: { Connection: { Reload: vi.fn() } },
}));
vi.mock('@moq/publish', () => ({
  Broadcast: vi.fn(),
  Lite: { Path: { from: vi.fn() } },
  Source: { Microphone: vi.fn(), Camera: vi.fn() },
}));
vi.mock('@moq/watch', () => ({
  Broadcast: vi.fn(),
  Sync: vi.fn(),
  Lite: { Path: { from: vi.fn() } },
  Audio: { Source: vi.fn(), Decoder: vi.fn(), Emitter: vi.fn() },
  Video: { Source: vi.fn(), Decoder: vi.fn(), Renderer: vi.fn() },
}));
vi.mock('@moq/signals', () => ({
  Effect: vi.fn(),
}));

// ---------------------------------------------------------------------------
// decideConnect
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// formatConnectError
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// cleanupConnectAttempt
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// waitForSignalValue
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// NULL_MOQ_REFS
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// analyzeSecondaryBroadcastTracks
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// filterSecondaryTracks
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// buildVideoEncoderConfig
// ---------------------------------------------------------------------------

describe('buildVideoEncoderConfig', () => {
  it('returns default vp09 codec when no track is provided', () => {
    const result = buildVideoEncoderConfig();
    expect(result.encoderConfig.codec).toBe('vp09');
    expect(result.encoderConfig.maxPixels).toBeUndefined();
    expect(result.encoderConfig.maxBitrate).toBeUndefined();
    expect(result.constraints).toBeUndefined();
  });

  it('returns default vp09 codec when track has no codec set', () => {
    const result = buildVideoEncoderConfig({
      kind: 'video',
      source: 'camera',
      broadcast: null,
      width: null,
      height: null,
      codec: null,
      max_bitrate: null,
    });
    expect(result.encoderConfig.codec).toBe('vp09');
  });

  it('maps vp9 to vp09 WebCodecs codec string', () => {
    const result = buildVideoEncoderConfig({
      kind: 'video',
      source: 'screen',
      broadcast: null,
      width: null,
      height: null,
      codec: 'vp9',
      max_bitrate: null,
    });
    expect(result.encoderConfig.codec).toBe('vp09');
  });

  it('passes through unrecognized codec values as-is', () => {
    const result = buildVideoEncoderConfig({
      kind: 'video',
      source: 'camera',
      broadcast: null,
      width: null,
      height: null,
      codec: 'h264',
      max_bitrate: null,
    });
    expect(result.encoderConfig.codec).toBe('h264');
  });

  it('computes maxPixels from width × height', () => {
    const result = buildVideoEncoderConfig({
      kind: 'video',
      source: 'screen',
      broadcast: null,
      width: 1280,
      height: 720,
      codec: null,
      max_bitrate: null,
    });
    expect(result.encoderConfig.maxPixels).toBe(1280 * 720);
  });

  it('does not set maxPixels when only width is provided', () => {
    const result = buildVideoEncoderConfig({
      kind: 'video',
      source: 'camera',
      broadcast: null,
      width: 1280,
      height: null,
      codec: null,
      max_bitrate: null,
    });
    expect(result.encoderConfig.maxPixels).toBeUndefined();
  });

  it('does not set maxPixels when only height is provided', () => {
    const result = buildVideoEncoderConfig({
      kind: 'video',
      source: 'camera',
      broadcast: null,
      width: null,
      height: 720,
      codec: null,
      max_bitrate: null,
    });
    expect(result.encoderConfig.maxPixels).toBeUndefined();
  });

  it('converts max_bitrate from kbps to bps', () => {
    const result = buildVideoEncoderConfig({
      kind: 'video',
      source: 'camera',
      broadcast: null,
      width: null,
      height: null,
      codec: null,
      max_bitrate: 2500,
    });
    expect(result.encoderConfig.maxBitrate).toBe(2_500_000);
  });

  it('returns capture constraints with width and height', () => {
    const result = buildVideoEncoderConfig({
      kind: 'video',
      source: 'screen',
      broadcast: null,
      width: 640,
      height: 480,
      codec: 'vp9',
      max_bitrate: null,
    });
    expect(result.constraints).toEqual({ width: 640, height: 480 });
  });

  it('returns constraints with only width when height is null', () => {
    const result = buildVideoEncoderConfig({
      kind: 'video',
      source: 'camera',
      broadcast: null,
      width: 1920,
      height: null,
      codec: null,
      max_bitrate: null,
    });
    expect(result.constraints).toEqual({ width: 1920 });
  });

  it('handles all fields set together', () => {
    const result = buildVideoEncoderConfig({
      kind: 'video',
      source: 'screen',
      broadcast: 'my-broadcast',
      width: 1920,
      height: 1080,
      codec: 'vp9',
      max_bitrate: 5000,
    });
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
  });

  it('skips maxPixels when height is zero', () => {
    const result = buildVideoEncoderConfig(makeTrack({ width: 1280, height: 0 }));
    expect(result.encoderConfig.maxPixels).toBeUndefined();
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

// ---------------------------------------------------------------------------
// validateTrackCodecs
// ---------------------------------------------------------------------------

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
