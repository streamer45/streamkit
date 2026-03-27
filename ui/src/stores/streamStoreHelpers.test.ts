// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { Getter } from '@moq/signals';
import { describe, expect, it, vi } from 'vitest';

import {
  decideConnect,
  cleanupConnectAttempt,
  waitForSignalValue,
  formatConnectError,
  analyzeSecondaryBroadcastTracks,
  filterSecondaryTracks,
  NULL_MOQ_REFS,
  type ConnectAttempt,
} from './streamStoreHelpers';

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
      { kind: 'video', source: 'camera', broadcast: 'cam-input' },
    ]);
    expect(result.needsVideo).toBe(true);
    expect(result.videoSourceType).toBe('camera');
    expect(result.warnings).toHaveLength(0);
  });

  it('returns needsVideo=true and screen source for a screen video track', () => {
    const result = analyzeSecondaryBroadcastTracks('screen2', [
      { kind: 'video', source: 'screen', broadcast: 'screen2' },
    ]);
    expect(result.needsVideo).toBe(true);
    expect(result.videoSourceType).toBe('screen');
    expect(result.warnings).toHaveLength(0);
  });

  it('returns needsVideo=false when only audio tracks are present', () => {
    const result = analyzeSecondaryBroadcastTracks('audio-only', [
      { kind: 'audio', source: 'microphone', broadcast: 'audio-only' },
    ]);
    expect(result.needsVideo).toBe(false);
    expect(result.videoSourceType).toBe('camera'); // default fallback
    expect(result.warnings).toHaveLength(1);
    expect(result.warnings[0]).toContain('audio tracks which are not yet supported');
  });

  it('warns when audio tracks are present alongside video', () => {
    const result = analyzeSecondaryBroadcastTracks('mixed', [
      { kind: 'video', source: 'camera', broadcast: 'mixed' },
      { kind: 'audio', source: 'microphone', broadcast: 'mixed' },
    ]);
    expect(result.needsVideo).toBe(true);
    expect(result.warnings).toHaveLength(1);
    expect(result.warnings[0]).toContain('audio tracks which are not yet supported');
  });

  it('warns when multiple video tracks are present', () => {
    const result = analyzeSecondaryBroadcastTracks('multi-video', [
      { kind: 'video', source: 'camera', broadcast: 'multi-video' },
      { kind: 'video', source: 'screen', broadcast: 'multi-video' },
    ]);
    expect(result.needsVideo).toBe(true);
    expect(result.videoSourceType).toBe('camera'); // first track wins
    expect(result.warnings).toHaveLength(1);
    expect(result.warnings[0]).toContain('2 video tracks');
    expect(result.warnings[0]).toContain('only the first is used');
  });

  it('collects both audio and multi-video warnings', () => {
    const result = analyzeSecondaryBroadcastTracks('all-warnings', [
      { kind: 'video', source: 'camera', broadcast: 'all-warnings' },
      { kind: 'video', source: 'screen', broadcast: 'all-warnings' },
      { kind: 'audio', source: 'microphone', broadcast: 'all-warnings' },
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
    { kind: 'audio' as const, source: 'microphone' as const, broadcast: null },
    { kind: 'video' as const, source: 'screen' as const, broadcast: null },
    { kind: 'video' as const, source: 'camera' as const, broadcast: 'cam-input' },
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
      { kind: 'video' as const, source: 'screen' as const, broadcast: 'a' },
      { kind: 'video' as const, source: 'camera' as const, broadcast: 'b' },
    ];
    expect(filterSecondaryTracks(explicitTracks, 'a', 'b')).toHaveLength(1);
    expect(filterSecondaryTracks(explicitTracks, 'a', 'a')).toHaveLength(1);
  });
});
