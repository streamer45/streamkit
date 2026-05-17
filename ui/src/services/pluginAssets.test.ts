// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook } from '@testing-library/react';
import React from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { PluginAsset } from '@/types/generated/api-types';

import {
  deletePluginAsset,
  listPluginAssets,
  uploadPluginAsset,
  useUploadPluginAsset,
} from './pluginAssets';

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

const ASSET: PluginAsset = {
  id: 'foo.slint',
  name: 'foo',
  path: 'samples/slint/system/foo.slint',
  format: 'slint',
  size_bytes: 256,
  is_system: true,
  type_id: 'slint',
  plugin_id: 'core',
};

const fetchMock = () => global.fetch as ReturnType<typeof vi.fn>;

beforeEach(() => {
  global.fetch = vi.fn() as never;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('listPluginAssets', () => {
  it('GETs /api/v1/assets/plugin/:typeId with URL-encoding', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [ASSET] }));

    const result = await listPluginAssets('slint/foo');

    expect(result).toEqual([ASSET]);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/assets/plugin/slint%2Ffoo');
    expect((init as RequestInit).method).toBe('GET');
  });

  it('throws on non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error', text: '' })
    );

    await expect(listPluginAssets('slint')).rejects.toThrow(/slint assets.*Server Error/);
  });
});

describe('uploadPluginAsset', () => {
  it('POSTs FormData to /api/v1/assets/plugin/:typeId', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: ASSET }));
    const file = new File(['data'], 'foo.slint');

    const result = await uploadPluginAsset('slint', file);

    expect(result).toEqual(ASSET);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/assets/plugin/slint');
    expect((init as RequestInit).method).toBe('POST');
    expect((init as RequestInit).body).toBeInstanceOf(FormData);
  });

  it('throws on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 413, statusText: 'Too Large', text: 'too big' })
    );

    await expect(uploadPluginAsset('slint', new File(['x'], 'x.slint'))).rejects.toThrow(
      /slint asset.*too big/
    );
  });
});

describe('deletePluginAsset', () => {
  it('DELETEs the URL-encoded typeId/id', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 204 }));

    await deletePluginAsset('slint', 'a b.slint');

    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/assets/plugin/slint/a%20b.slint');
    expect((init as RequestInit).method).toBe('DELETE');
  });

  it('throws on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 404, statusText: 'Not Found', text: '' })
    );

    await expect(deletePluginAsset('slint', 'x')).rejects.toThrow(/Not Found/);
  });
});

describe('useUploadPluginAsset', () => {
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

  it('rejects with an explanatory error when typeId is empty', async () => {
    const { result } = renderHook(() => useUploadPluginAsset(''), { wrapper });

    await act(async () => {
      await expect(result.current.mutateAsync(new File(['x'], 'x.slint'))).rejects.toThrow(
        'No plugin asset type selected'
      );
    });
  });

  it('uploads via the underlying service and invalidates the keyed query on success', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: ASSET }));
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries');

    const { result } = renderHook(() => useUploadPluginAsset('slint'), { wrapper });
    await act(async () => {
      const ret = await result.current.mutateAsync(new File(['x'], 'foo.slint'));
      expect(ret).toEqual(ASSET);
    });

    expect(fetchMock().mock.calls[0][1]).toMatchObject({ method: 'POST' });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['pluginAssets', 'slint'] });
  });
});
