// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook, waitFor } from '@testing-library/react';
import React from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { FontAsset } from '@/types/generated/api-types';

import {
  deleteFontAsset,
  fontFamilyForAsset,
  listFontAssets,
  loadFontAssets,
  uploadFontAsset,
  useDeleteFontAsset,
  useFontAssets,
  useUploadFontAsset,
} from './fontAssets';

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

const SYSTEM_ASSET: FontAsset = {
  id: 'Inter.ttf',
  name: 'Inter',
  path: 'samples/fonts/system/Inter.ttf',
  format: 'ttf',
  size_bytes: 12345,
  is_system: true,
};

const fetchMock = () => global.fetch as ReturnType<typeof vi.fn>;

describe('fontFamilyForAsset', () => {
  it('derives sk- prefixed family from system font path', () => {
    expect(fontFamilyForAsset('samples/fonts/system/Inter.ttf')).toBe('sk-Inter');
  });

  it('strips the final file extension', () => {
    expect(fontFamilyForAsset('samples/fonts/system/DejaVuSans.ttf')).toBe('sk-DejaVuSans');
    expect(fontFamilyForAsset('samples/fonts/user/CustomFont.otf')).toBe('sk-CustomFont');
  });

  it('preserves -Bold and other suffixes in the family name', () => {
    expect(fontFamilyForAsset('samples/fonts/system/DejaVuSans-Bold.ttf')).toBe(
      'sk-DejaVuSans-Bold'
    );
  });

  it('handles a bare filename without any directory components', () => {
    expect(fontFamilyForAsset('Roboto.ttf')).toBe('sk-Roboto');
  });

  it('handles a path with no extension', () => {
    expect(fontFamilyForAsset('samples/fonts/system/NoExt')).toBe('sk-NoExt');
  });

  it('only strips the final dotted segment when the filename has multiple dots', () => {
    expect(fontFamilyForAsset('samples/fonts/My.Font.Name.ttf')).toBe('sk-My.Font.Name');
  });
});

describe('listFontAssets', () => {
  beforeEach(() => {
    global.fetch = vi.fn() as never;
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('GETs /api/v1/assets/fonts with credentials and returns the parsed array', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [SYSTEM_ASSET] }));

    const result = await listFontAssets();

    expect(result).toEqual([SYSTEM_ASSET]);
    expect(fetchMock()).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/assets/fonts');
    expect((init as RequestInit).method).toBe('GET');
    expect((init as RequestInit).credentials).toBe('include');
  });

  it('throws with the response statusText on a 4xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 404, statusText: 'Not Found', text: 'nope' })
    );

    await expect(listFontAssets()).rejects.toThrowError(/Not Found/);
  });

  it('rejects when the underlying fetch errors', async () => {
    fetchMock().mockRejectedValueOnce(new Error('boom'));

    await expect(listFontAssets()).rejects.toThrowError('boom');
  });
});

describe('uploadFontAsset', () => {
  beforeEach(() => {
    global.fetch = vi.fn() as never;
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('POSTs FormData containing the file to /api/v1/assets/fonts', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: SYSTEM_ASSET }));

    const file = new File(['data'], 'Inter.ttf', { type: 'font/ttf' });
    const result = await uploadFontAsset(file);

    expect(result).toEqual(SYSTEM_ASSET);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/assets/fonts');
    expect((init as RequestInit).method).toBe('POST');
    expect((init as RequestInit).body).toBeInstanceOf(FormData);
    const fd = (init as RequestInit).body as FormData;
    const sent = fd.get('file');
    expect(sent).toBeInstanceOf(File);
    expect((sent as File).name).toBe('Inter.ttf');
  });

  it('throws including the server error body when the upload is rejected (413)', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({
        ok: false,
        status: 413,
        statusText: 'Payload Too Large',
        text: 'file too big',
      })
    );

    await expect(
      uploadFontAsset(new File([''], 'Huge.ttf', { type: 'font/ttf' }))
    ).rejects.toThrowError(/file too big/);
  });

  it('on 409 conflict, refetches the list and reuses the asset matching the sanitized filename', async () => {
    const existing: FontAsset = {
      ...SYSTEM_ASSET,
      id: 'My_Font.ttf',
      name: 'My Font',
      path: 'samples/fonts/user/My_Font.ttf',
      is_system: false,
    };
    fetchMock()
      .mockResolvedValueOnce(mockResponse({ ok: false, status: 409 }))
      .mockResolvedValueOnce(mockResponse({ ok: true, status: 200, json: [existing] }));

    const file = new File(['data'], 'My Font.ttf', { type: 'font/ttf' });
    const result = await uploadFontAsset(file);

    expect(result).toEqual(existing);
    expect(fetchMock()).toHaveBeenCalledTimes(2);
    expect(fetchMock().mock.calls[1][0]).toBe('http://localhost:4545/api/v1/assets/fonts');
  });

  it('on 409 conflict with no matching existing asset, throws', async () => {
    fetchMock()
      .mockResolvedValueOnce(mockResponse({ ok: false, status: 409 }))
      .mockResolvedValueOnce(mockResponse({ ok: true, status: 200, json: [] }));

    await expect(
      uploadFontAsset(new File([''], 'Other.ttf', { type: 'font/ttf' }))
    ).rejects.toThrowError(/already exists/);
  });
});

describe('deleteFontAsset', () => {
  beforeEach(() => {
    global.fetch = vi.fn() as never;
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('DELETEs the encoded id under /api/v1/assets/fonts', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 204 }));

    await deleteFontAsset('My Font.ttf');

    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/assets/fonts/My%20Font.ttf');
    expect((init as RequestInit).method).toBe('DELETE');
    expect((init as RequestInit).credentials).toBe('include');
  });

  it('throws including the server error body on a 404', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({
        ok: false,
        status: 404,
        statusText: 'Not Found',
        text: 'no such font',
      })
    );

    await expect(deleteFontAsset('missing.ttf')).rejects.toThrowError(/no such font/);
  });
});

type FontFaceCall = {
  family: string;
  source: string;
  descriptors?: FontFaceDescriptors;
};

const installFontFaceStub = (load: ReturnType<typeof vi.fn>) => {
  const calls: FontFaceCall[] = [];
  class FakeFontFace {
    family: string;
    source: string;
    descriptors?: FontFaceDescriptors;
    load: typeof load;
    constructor(family: string, source: string, descriptors?: FontFaceDescriptors) {
      this.family = family;
      this.source = source;
      this.descriptors = descriptors;
      this.load = load;
      calls.push({ family, source, descriptors });
    }
  }
  vi.stubGlobal('FontFace', FakeFontFace);
  const addSpy = vi.fn();
  Object.defineProperty(document, 'fonts', {
    configurable: true,
    value: { add: addSpy },
  });
  return { calls, addSpy };
};

describe('loadFontAssets', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('registers each asset with FontFace using the sk- family and adds it to document.fonts', async () => {
    const load = vi.fn().mockResolvedValue(undefined);
    const { calls, addSpy } = installFontFaceStub(load);

    await loadFontAssets([
      {
        ...SYSTEM_ASSET,
        id: 'load1-Regular.ttf',
        path: 'load1/system/Regular.ttf',
      },
      {
        ...SYSTEM_ASSET,
        id: 'load1-DejaVuSans-Bold.ttf',
        path: 'load1/system/DejaVuSans-Bold.ttf',
        name: 'DejaVuSans Bold',
      },
    ]);

    expect(load).toHaveBeenCalledTimes(2);
    expect(addSpy).toHaveBeenCalledTimes(2);

    expect(calls[0].family).toBe('sk-Regular');
    expect(calls[0].source).toBe('url(/api/v1/assets/fonts/file/system/Regular.ttf)');
    expect(calls[0].descriptors?.weight).toBe('400');

    expect(calls[1].family).toBe('sk-DejaVuSans-Bold');
    expect(calls[1].descriptors?.weight).toBe('700');
  });

  it('URL-encodes special characters in the asset path when building the served URL', async () => {
    const load = vi.fn().mockResolvedValue(undefined);
    const { calls } = installFontFaceStub(load);

    await loadFontAssets([
      {
        ...SYSTEM_ASSET,
        id: 'load2-My Font.ttf',
        path: 'load2/user/My Font.ttf',
        is_system: false,
      },
    ]);

    expect(calls[0].source).toBe('url(/api/v1/assets/fonts/file/user/My%20Font.ttf)');
  });

  it('does not reject when a single FontFace.load fails — it logs and continues', async () => {
    const load = vi.fn().mockRejectedValue(new Error('decode error'));
    const { addSpy } = installFontFaceStub(load);

    await expect(
      loadFontAssets([{ ...SYSTEM_ASSET, id: 'load3-broken.ttf', path: 'load3/system/broken.ttf' }])
    ).resolves.toBeUndefined();

    expect(load).toHaveBeenCalledTimes(1);
    expect(addSpy).not.toHaveBeenCalled();
  });

  it('caches loaded fonts so repeated calls for the same path do not re-register', async () => {
    const load = vi.fn().mockResolvedValue(undefined);
    installFontFaceStub(load);

    const asset: FontAsset = {
      ...SYSTEM_ASSET,
      id: 'load4-Once.ttf',
      path: 'load4/system/Once.ttf',
    };
    await loadFontAssets([asset]);
    expect(load).toHaveBeenCalledTimes(1);

    await loadFontAssets([asset]);
    expect(load).toHaveBeenCalledTimes(1);
  });
});

const makeWrapper = (client: QueryClient) => {
  return ({ children }: { children: React.ReactNode }) =>
    React.createElement(QueryClientProvider, { client }, children);
};

describe('useFontAssets / useUploadFontAsset / useDeleteFontAsset', () => {
  let queryClient: QueryClient;
  let wrapper: ReturnType<typeof makeWrapper>;

  beforeEach(() => {
    global.fetch = vi.fn() as never;
    queryClient = new QueryClient({
      defaultOptions: {
        queries: { retry: false, gcTime: 0 },
        mutations: { retry: false },
      },
    });
    wrapper = makeWrapper(queryClient);
  });

  afterEach(() => {
    queryClient.clear();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('useFontAssets resolves to the listed assets', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [SYSTEM_ASSET] }));

    const { result } = renderHook(() => useFontAssets(), { wrapper });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data).toEqual([SYSTEM_ASSET]);
  });

  it('useFontAssets surfaces fetch errors as an error state', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error', text: '' })
    );

    const { result } = renderHook(() => useFontAssets(), { wrapper });

    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.error).toBeInstanceOf(Error);
  });

  it('useFontAssets does not fetch when enabled=false', () => {
    renderHook(() => useFontAssets(false), { wrapper });

    expect(fetchMock()).not.toHaveBeenCalled();
  });

  it('useUploadFontAsset uploads via uploadFontAsset and invalidates the fontAssets query on success', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: SYSTEM_ASSET }));
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries');

    const { result } = renderHook(() => useUploadFontAsset(), { wrapper });
    const file = new File(['data'], 'Inter.ttf', { type: 'font/ttf' });

    await act(async () => {
      const ret = await result.current.mutateAsync(file);
      expect(ret).toEqual(SYSTEM_ASSET);
    });

    expect(fetchMock().mock.calls[0][1]).toMatchObject({ method: 'POST' });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['fontAssets'] });
  });

  it('useDeleteFontAsset deletes via deleteFontAsset and invalidates the fontAssets query on success', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 204 }));
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries');

    const { result } = renderHook(() => useDeleteFontAsset(), { wrapper });

    await act(async () => {
      await result.current.mutateAsync('Inter.ttf');
    });

    expect(fetchMock().mock.calls[0][1]).toMatchObject({ method: 'DELETE' });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['fontAssets'] });
  });
});
