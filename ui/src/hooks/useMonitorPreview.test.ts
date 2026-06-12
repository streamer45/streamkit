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

import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { cleanupPreview, useMonitorPreview } from './useMonitorPreview';

// Mock the sessions service so stopPreview calls don't hit a real server.
vi.mock('@/services/sessions', () => ({
  startPreview: vi.fn(),
  stopPreview: vi.fn().mockResolvedValue(undefined),
}));

const storeState = {
  status: 'disconnected',
  configServerUrl: '',
  serverUrl: '',
  configLoaded: true,
  loadConfig: vi.fn(),
  connect: vi.fn(),
  disconnect: vi.fn(),
  setEnablePublish: vi.fn(),
  setEnableWatch: vi.fn(),
  setServerUrl: vi.fn(),
  setOutputBroadcast: vi.fn(),
  setPipelineOutputTypes: vi.fn(),
  audioEmitter: null,
};

vi.mock('@/stores/streamStore', () => ({
  useStreamStore: Object.assign(
    vi.fn((selector: (s: unknown) => unknown) => selector(storeState)),
    {
      getState: vi.fn(() => storeState),
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

describe('useMonitorPreview session-switch state reset', () => {
  it('clears previewError when the selected session changes', async () => {
    const { startPreview } = await import('@/services/sessions');
    vi.mocked(startPreview).mockRejectedValueOnce(new Error('boom'));

    const { result, rerender } = renderHook(
      ({ id }: { id: string | null }) => useMonitorPreview(id),
      {
        initialProps: { id: 's1' as string | null },
      }
    );

    await act(async () => {
      await result.current.handleStartPreview();
    });
    expect(result.current.previewError).toBe('boom');

    rerender({ id: 's2' });

    expect(result.current.previewError).toBeNull();
    expect(result.current.isPreviewLoading).toBe(false);
  });

  it('keeps state when rerendered with the same session', async () => {
    const { startPreview } = await import('@/services/sessions');
    vi.mocked(startPreview).mockRejectedValueOnce(new Error('boom'));

    const { result, rerender } = renderHook(
      ({ id }: { id: string | null }) => useMonitorPreview(id),
      {
        initialProps: { id: 's1' as string | null },
      }
    );

    await act(async () => {
      await result.current.handleStartPreview();
    });
    rerender({ id: 's1' });

    expect(result.current.previewError).toBe('boom');
  });
});
