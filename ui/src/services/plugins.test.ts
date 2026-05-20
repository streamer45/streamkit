// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { deletePlugin, uploadPlugin } from './plugins';

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

const SUMMARY = {
  kind: 'plugin::native::foo',
  original_kind: 'foo',
  file_name: 'foo.so',
  categories: ['audio'],
  loaded_at_ms: 1700000000000,
  plugin_type: 'native',
};

const fetchMock = () => global.fetch as ReturnType<typeof vi.fn>;

beforeEach(() => {
  global.fetch = vi.fn() as never;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('uploadPlugin', () => {
  it('POSTs FormData with the plugin file to /api/v1/plugins', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: SUMMARY }));
    const file = new File(['data'], 'foo.so');

    const result = await uploadPlugin(file);

    expect(result).toEqual(SUMMARY);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/plugins');
    expect((init as RequestInit).method).toBe('POST');
    const body = (init as RequestInit).body as FormData;
    expect(body).toBeInstanceOf(FormData);
    expect(body.get('plugin')).toBeInstanceOf(File);
    expect((body.get('plugin') as File).name).toBe('foo.so');
  });

  it('throws the response text on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 400, text: 'invalid plugin', statusText: 'Bad Request' })
    );

    await expect(uploadPlugin(new File(['x'], 'x.so'))).rejects.toThrow('invalid plugin');
  });

  it('falls back to status code in error message when body is empty', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, text: '', statusText: 'Server Error' })
    );

    await expect(uploadPlugin(new File(['x'], 'x.so'))).rejects.toThrow(/status 500/);
  });
});

describe('deletePlugin', () => {
  it('DELETEs /api/v1/plugins/:kind without keep_file by default', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: SUMMARY }));

    const result = await deletePlugin('plugin::native::foo');

    expect(result).toEqual(SUMMARY);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/plugins/plugin%3A%3Anative%3A%3Afoo');
    expect((init as RequestInit).method).toBe('DELETE');
  });

  it('appends ?keep_file=true when keepFile is set', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: SUMMARY }));

    await deletePlugin('foo', { keepFile: true });

    expect(fetchMock().mock.calls[0][0]).toBe(
      'http://localhost:4545/api/v1/plugins/foo?keep_file=true'
    );
  });

  it('throws on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 404, text: '', statusText: 'Not Found' })
    );

    await expect(deletePlugin('foo')).rejects.toThrow(/status 404/);
  });
});
