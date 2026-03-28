// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Unit tests for the cleanupPreview helper in useMonitorPreview.
 *
 * These tests verify that the preview cleanup logic does NOT tear down an
 * active StreamView connection when the preview never created its own
 * MoQ connection (the root cause of the "inputs interrupted on view switch"
 * bug).
 */

import { describe, expect, it, vi } from 'vitest';

import { cleanupPreview } from './useMonitorPreview';

// Mock the sessions service so stopPreview calls don't hit a real server.
vi.mock('@/services/sessions', () => ({
  startPreview: vi.fn(),
  stopPreview: vi.fn().mockResolvedValue(undefined),
}));

// Mock the stream store — cleanupPreview doesn't use it directly but the
// module-level import in useMonitorPreview.ts needs it resolvable.
vi.mock('@/stores/streamStore', () => ({
  useStreamStore: Object.assign(
    vi.fn(() => ({})),
    {
      getState: vi.fn(() => ({ status: 'disconnected', configServerUrl: '', serverUrl: '' })),
    }
  ),
}));

// Mock zustand/shallow — required by the useMonitorPreview module.
vi.mock('zustand/shallow', () => ({
  useShallow: vi.fn((fn: unknown) => fn),
}));

// Mock moqPeerSettings utility.
vi.mock('@/utils/moqPeerSettings', () => ({
  updateUrlPath: vi.fn((_base: string, path: string) => path),
}));

/** Create a minimal ref-like object for testing. */
function makeRef<T>(value: T): React.MutableRefObject<T> {
  return { current: value };
}

describe('cleanupPreview', () => {
  it('should NOT call disconnect when ownsConnectionRef is false (no preview started)', async () => {
    const disconnect = vi.fn();

    await cleanupPreview(
      makeRef<string | null>(null),
      makeRef<string | null>(null),
      disconnect,
      makeRef(false)
    );

    expect(disconnect).not.toHaveBeenCalled();
  });

  it('should call disconnect when ownsConnectionRef is true (preview created its own connection)', async () => {
    const disconnect = vi.fn();
    const ownsRef = makeRef(true);

    await cleanupPreview(
      makeRef<string | null>(null),
      makeRef<string | null>(null),
      disconnect,
      ownsRef
    );

    expect(disconnect).toHaveBeenCalledOnce();
    expect(ownsRef.current).toBe(false);
  });

  it('should call stopPreview and disconnect when preview is active and connection is owned', async () => {
    const { stopPreview } = await import('@/services/sessions');
    const disconnect = vi.fn();
    const previewIdRef = makeRef<string | null>('preview-123');
    const sessionIdRef = makeRef<string | null>('session-456');
    const ownsRef = makeRef(true);

    await cleanupPreview(previewIdRef, sessionIdRef, disconnect, ownsRef);

    expect(stopPreview).toHaveBeenCalledWith('session-456', 'preview-123');
    expect(disconnect).toHaveBeenCalledOnce();
    expect(previewIdRef.current).toBeNull();
    expect(sessionIdRef.current).toBeNull();
    expect(ownsRef.current).toBe(false);
  });

  it('should call stopPreview but NOT disconnect when preview is active but connection is not owned', async () => {
    const { stopPreview } = await import('@/services/sessions');
    vi.mocked(stopPreview).mockClear();
    const disconnect = vi.fn();
    const previewIdRef = makeRef<string | null>('preview-abc');
    const sessionIdRef = makeRef<string | null>('session-def');
    const ownsRef = makeRef(false);

    await cleanupPreview(previewIdRef, sessionIdRef, disconnect, ownsRef);

    expect(stopPreview).toHaveBeenCalledWith('session-def', 'preview-abc');
    expect(disconnect).not.toHaveBeenCalled();
    expect(previewIdRef.current).toBeNull();
    expect(sessionIdRef.current).toBeNull();
  });

  it('should handle stopPreview failure gracefully', async () => {
    const { stopPreview } = await import('@/services/sessions');
    vi.mocked(stopPreview).mockRejectedValueOnce(new Error('server gone'));
    const disconnect = vi.fn();
    const ownsRef = makeRef(true);

    await cleanupPreview(
      makeRef<string | null>('p1'),
      makeRef<string | null>('s1'),
      disconnect,
      ownsRef
    );

    // disconnect should still be called despite stopPreview failure
    expect(disconnect).toHaveBeenCalledOnce();
    expect(ownsRef.current).toBe(false);
  });
});
