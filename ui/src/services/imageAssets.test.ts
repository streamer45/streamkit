// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook, waitFor } from '@testing-library/react';
import React from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { ImageAsset } from '@/types/generated/api-types';

import {
  deleteImageAsset,
  listImageAssets,
  uploadImageAsset,
  useDeleteImageAsset,
  useImageAssets,
  useUploadImageAsset,
} from './imageAssets';

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

const ASSET: ImageAsset = {
  id: 'logo.png',
  name: 'logo',
  path: 'samples/images/system/logo.png',
  format: 'png',
  width: 100,
  height: 100,
  size_bytes: 2048,
  is_system: true,
};

const fetchMock = () => global.fetch as ReturnType<typeof vi.fn>;

beforeEach(() => {
  global.fetch = vi.fn() as never;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('listImageAssets', () => {
  it('GETs /api/v1/assets/images and returns the parsed array', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [ASSET] }));

    const result = await listImageAssets();

    expect(result).toEqual([ASSET]);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/assets/images');
    expect((init as RequestInit).method).toBe('GET');
  });

  it('throws on non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error', text: '' })
    );

    await expect(listImageAssets()).rejects.toThrow(/Server Error/);
  });
});

describe('uploadImageAsset', () => {
  it('POSTs FormData containing the file to /api/v1/assets/images', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: ASSET }));
    const file = new File(['data'], 'logo.png', { type: 'image/png' });

    const result = await uploadImageAsset(file);

    expect(result).toEqual(ASSET);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/assets/images');
    expect((init as RequestInit).method).toBe('POST');
    expect((init as RequestInit).body).toBeInstanceOf(FormData);
  });

  it('on 409, returns the existing asset matching the sanitized filename', async () => {
    const existing: ImageAsset = { ...ASSET, id: 'weird_name.png', name: 'weird' };
    fetchMock()
      .mockResolvedValueOnce(mockResponse({ ok: false, status: 409, statusText: 'Conflict' }))
      .mockResolvedValueOnce(mockResponse({ ok: true, status: 200, json: [existing] }));

    const result = await uploadImageAsset(new File(['data'], 'weird name.png'));

    expect(result).toEqual(existing);
    expect(fetchMock().mock.calls[0][0]).toBe('http://localhost:4545/api/v1/assets/images');
    expect(fetchMock().mock.calls[0][1].method).toBe('POST');
    expect(fetchMock().mock.calls[1][0]).toBe('http://localhost:4545/api/v1/assets/images');
    expect(fetchMock().mock.calls[1][1].method).toBe('GET');
  });

  it('on 409 with no existing match, throws an explanatory error', async () => {
    fetchMock()
      .mockResolvedValueOnce(mockResponse({ ok: false, status: 409, statusText: 'Conflict' }))
      .mockResolvedValueOnce(mockResponse({ ok: true, status: 200, json: [] }));

    await expect(uploadImageAsset(new File(['data'], 'logo.png'))).rejects.toThrow(
      'Image asset already exists: logo.png'
    );
  });

  it('throws on non-2xx (non-409)', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 413, statusText: 'Too Large', text: 'too big' })
    );

    await expect(uploadImageAsset(new File(['x'], 'x.png'))).rejects.toThrow('too big');
  });
});

describe('deleteImageAsset', () => {
  it('DELETEs the URL-encoded id', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 204 }));

    await deleteImageAsset('foo/bar.png');

    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/assets/images/foo%2Fbar.png');
    expect((init as RequestInit).method).toBe('DELETE');
  });

  it('throws on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 404, statusText: 'Not Found', text: '' })
    );

    await expect(deleteImageAsset('x')).rejects.toThrow(/Not Found/);
  });
});

describe('useImageAssets / useUploadImageAsset / useDeleteImageAsset', () => {
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

  it('useImageAssets resolves to the listed assets', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [ASSET] }));

    const { result } = renderHook(() => useImageAssets(), { wrapper });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data).toEqual([ASSET]);
  });

  it('useImageAssets does not fetch when enabled=false', () => {
    renderHook(() => useImageAssets(false), { wrapper });
    expect(fetchMock()).not.toHaveBeenCalled();
  });

  it('useUploadImageAsset uploads and invalidates the imageAssets query on success', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: ASSET }));
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries');

    const { result } = renderHook(() => useUploadImageAsset(), { wrapper });
    await act(async () => {
      const ret = await result.current.mutateAsync(new File(['x'], 'logo.png'));
      expect(ret).toEqual(ASSET);
    });

    expect(fetchMock().mock.calls[0][1]).toMatchObject({ method: 'POST' });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['imageAssets'] });
  });

  it('useDeleteImageAsset deletes and invalidates the imageAssets query on success', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 204 }));
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries');

    const { result } = renderHook(() => useDeleteImageAsset(), { wrapper });
    await act(async () => {
      await result.current.mutateAsync('logo.png');
    });

    expect(fetchMock().mock.calls[0][1]).toMatchObject({ method: 'DELETE' });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['imageAssets'] });
  });
});
