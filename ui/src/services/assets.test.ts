// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook, waitFor } from '@testing-library/react';
import React from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { AudioAsset } from '@/types/generated/api-types';

import {
  deleteAudioAsset,
  listAudioAssets,
  uploadAudioAsset,
  useAudioAssets,
  useDeleteAudioAsset,
  useUploadAudioAsset,
} from './assets';

vi.mock('@/utils/logger', () => ({
  getLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

vi.mock('./base', () => ({
  getApiUrl: () => 'http://localhost:4545',
  fetchApi: (path: string, options: RequestInit = {}) => {
    const normalized = path.startsWith('/') ? path : `/${path}`;
    return fetch(`http://localhost:4545${normalized}`, { ...options, credentials: 'include' });
  },
}));

type MockResponseInit = {
  ok?: boolean;
  status?: number;
  statusText?: string;
  json?: unknown;
  text?: string;
};

const mockResponse = (init: MockResponseInit) => {
  const status = init.status ?? (init.ok === false ? 500 : 200);
  return {
    ok: init.ok ?? (status >= 200 && status < 300),
    status,
    statusText: init.statusText ?? '',
    json: async () => init.json,
    text: async () => init.text ?? '',
  };
};

const ASSET: AudioAsset = {
  id: 'a.wav',
  name: 'A',
  path: 'samples/audio/system/a.wav',
  format: 'wav',
  size_bytes: 1024,
  license: null,
  is_system: true,
};

const fetchMock = () => global.fetch as ReturnType<typeof vi.fn>;

beforeEach(() => {
  global.fetch = vi.fn() as never;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('listAudioAssets', () => {
  it('GETs /api/v1/assets/audio and returns the parsed array', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [ASSET] }));

    const result = await listAudioAssets();

    expect(result).toEqual([ASSET]);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/assets/audio');
    expect((init as RequestInit).method).toBe('GET');
    expect((init as RequestInit).headers).toMatchObject({ 'Content-Type': 'application/json' });
  });

  it('throws on non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error', text: '' })
    );

    await expect(listAudioAssets()).rejects.toThrow(/Server Error/);
  });
});

describe('uploadAudioAsset', () => {
  it('POSTs FormData containing the file to /api/v1/assets/audio', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: ASSET }));
    const file = new File(['data'], 'a.wav', { type: 'audio/wav' });

    const result = await uploadAudioAsset(file);

    expect(result).toEqual(ASSET);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/assets/audio');
    expect((init as RequestInit).method).toBe('POST');
    expect((init as RequestInit).body).toBeInstanceOf(FormData);
    expect(((init as RequestInit).body as FormData).get('file')).toBeInstanceOf(File);
  });

  it('throws on non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 413, statusText: 'Too Large', text: 'too big' })
    );

    await expect(uploadAudioAsset(new File(['data'], 'x.wav'))).rejects.toThrow('too big');
  });
});

describe('deleteAudioAsset', () => {
  it('DELETEs the URL-encoded id', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 204 }));

    await deleteAudioAsset('foo bar.wav');

    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/assets/audio/foo%20bar.wav');
    expect((init as RequestInit).method).toBe('DELETE');
  });

  it('throws on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 404, statusText: 'Not Found', text: '' })
    );

    await expect(deleteAudioAsset('x')).rejects.toThrow(/Not Found/);
  });
});

describe('useAudioAssets / useUploadAudioAsset / useDeleteAudioAsset', () => {
  let queryClient: QueryClient;
  let wrapper: ({ children }: { children: React.ReactNode }) => React.ReactElement;

  beforeEach(() => {
    queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false, gcTime: 0 }, mutations: { retry: false } },
    });
    wrapper = ({ children }) =>
      React.createElement(QueryClientProvider, { client: queryClient }, children);
  });

  afterEach(() => {
    queryClient.clear();
  });

  it('useAudioAssets resolves to the listed assets', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [ASSET] }));

    const { result } = renderHook(() => useAudioAssets(), { wrapper });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data).toEqual([ASSET]);
  });

  it('useAudioAssets does not fetch when enabled=false', () => {
    renderHook(() => useAudioAssets(false), { wrapper });
    expect(fetchMock()).not.toHaveBeenCalled();
  });

  it('useUploadAudioAsset uploads and invalidates the audioAssets query on success', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: ASSET }));
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries');

    const { result } = renderHook(() => useUploadAudioAsset(), { wrapper });
    await act(async () => {
      const ret = await result.current.mutateAsync(new File(['x'], 'a.wav'));
      expect(ret).toEqual(ASSET);
    });

    expect(fetchMock().mock.calls[0][1]).toMatchObject({ method: 'POST' });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['audioAssets'] });
  });

  it('useDeleteAudioAsset deletes and invalidates the audioAssets query on success', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 204 }));
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries');

    const { result } = renderHook(() => useDeleteAudioAsset(), { wrapper });
    await act(async () => {
      await result.current.mutateAsync('a.wav');
    });

    expect(fetchMock().mock.calls[0][1]).toMatchObject({ method: 'DELETE' });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['audioAssets'] });
  });
});
