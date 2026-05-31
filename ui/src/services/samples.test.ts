// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { SamplePipeline } from '@/types/generated/api-types';

import {
  deleteSample,
  listAllSamples,
  listSamples,
  listDynamicSamples,
  saveSample,
} from './samples';

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

const fetchMock = () => global.fetch as ReturnType<typeof vi.fn>;

const SAMPLE: SamplePipeline = {
  id: 'pipe-1',
  name: 'My Pipeline',
  description: 'desc',
  yaml: 'nodes: {}',
  is_system: false,
  mode: 'oneshot',
  is_fragment: false,
  group: null,
  variant: null,
  canonical: false,
  category: null,
  tags: [],
  search_terms: [],
};

beforeEach(() => {
  global.fetch = vi.fn() as never;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('listSamples', () => {
  it('GETs /api/v1/samples/oneshot and returns the parsed array', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [SAMPLE] }));

    const result = await listSamples();

    expect(result).toEqual([SAMPLE]);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/samples/oneshot');
    expect((init as RequestInit).method).toBe('GET');
  });

  it('throws on non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error', text: 'boom' })
    );

    await expect(listSamples()).rejects.toThrow(/Server Error/);
  });
});

describe('listDynamicSamples', () => {
  it('GETs /api/v1/samples/dynamic', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [SAMPLE] }));

    const result = await listDynamicSamples();

    expect(result).toEqual([SAMPLE]);
    expect(fetchMock().mock.calls[0][0]).toBe('http://localhost:4545/api/v1/samples/dynamic');
  });

  it('throws on non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error', text: '' })
    );

    await expect(listDynamicSamples()).rejects.toThrow(/Server Error/);
  });
});

describe('listAllSamples', () => {
  it('merges oneshot + dynamic and dedupes by id', async () => {
    const oneshot = [{ ...SAMPLE, id: 'a' }];
    const dynamic = [
      { ...SAMPLE, id: 'a', mode: 'dynamic' },
      { ...SAMPLE, id: 'b', mode: 'dynamic' },
    ];

    fetchMock().mockImplementation((url: string) => {
      if (url.includes('/oneshot')) {
        return Promise.resolve(mockResponse({ ok: true, status: 200, json: oneshot }));
      }
      return Promise.resolve(mockResponse({ ok: true, status: 200, json: dynamic }));
    });

    const result = await listAllSamples();

    expect(result.map((s) => s.id)).toEqual(['a', 'b']);
  });

  it('rejects when either underlying request fails (Promise.all semantics)', async () => {
    fetchMock().mockImplementation((url: string) => {
      if (url.includes('/oneshot')) {
        return Promise.resolve(mockResponse({ ok: true, status: 200, json: [SAMPLE] }));
      }
      return Promise.resolve(mockResponse({ ok: false, status: 500, statusText: 'Server Error' }));
    });

    await expect(listAllSamples()).rejects.toThrow(/Server Error/);
  });
});

describe('saveSample', () => {
  it('POSTs the SavePipelineRequest as JSON to /api/v1/samples/oneshot', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: SAMPLE }));

    const result = await saveSample({
      name: 'p',
      description: 'd',
      yaml: 'y',
      overwrite: false,
      is_fragment: false,
    });

    expect(result).toEqual(SAMPLE);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/samples/oneshot');
    expect((init as RequestInit).method).toBe('POST');
    expect((init as RequestInit).headers).toMatchObject({ 'Content-Type': 'application/json' });
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      name: 'p',
      description: 'd',
      yaml: 'y',
      overwrite: false,
      is_fragment: false,
    });
  });

  it('throws on non-2xx with status code in message', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 409, statusText: 'Conflict', text: 'exists' })
    );

    await expect(
      saveSample({ name: 'p', description: '', yaml: '', overwrite: false, is_fragment: false })
    ).rejects.toThrow(/409.*exists/);
  });
});

describe('deleteSample', () => {
  it('DELETEs the URL-encoded sample id', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 204 }));

    await deleteSample('foo/bar');

    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/samples/oneshot/foo%2Fbar');
    expect((init as RequestInit).method).toBe('DELETE');
  });

  it('throws on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 404, statusText: 'Not Found', text: 'missing' })
    );

    await expect(deleteSample('x')).rejects.toThrow(/missing/);
  });
});
