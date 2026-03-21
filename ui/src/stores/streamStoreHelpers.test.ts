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
      'healthEffect',
    ];

    for (const key of expectedKeys) {
      expect(NULL_MOQ_REFS).toHaveProperty(key, null);
    }
  });
});
