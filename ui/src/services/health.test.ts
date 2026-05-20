// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { fetchHealth } from './health';

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
};

const mockResponse = (init: MockResponseInit) => {
  const status = init.status ?? (init.ok === false ? 500 : 200);
  return {
    ok: init.ok ?? (status >= 200 && status < 300),
    status,
    statusText: init.statusText ?? '',
    json: async () => init.json,
    text: async () => '',
  };
};

const fetchMock = () => global.fetch as ReturnType<typeof vi.fn>;

beforeEach(() => {
  global.fetch = vi.fn() as never;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('fetchHealth', () => {
  it('GETs /health and normalizes the response', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({
        ok: true,
        status: 200,
        json: { status: 'ok', version: '1.2.3', build_hash: 'abc' },
      })
    );

    const result = await fetchHealth();

    expect(result).toEqual({ status: 'ok', version: '1.2.3', buildHash: 'abc' });
    expect(fetchMock().mock.calls[0][0]).toBe('http://localhost:4545/health');
  });

  it('prefers snake_case build_hash but falls back to buildHash camelCase', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: true, status: 200, json: { buildHash: 'camel' } })
    );

    const result = await fetchHealth();

    expect(result).toEqual({ status: 'unknown', version: 'unknown', buildHash: 'camel' });
  });

  it('fills missing fields with "unknown"', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: {} }));

    const result = await fetchHealth();

    expect(result).toEqual({ status: 'unknown', version: 'unknown', buildHash: 'unknown' });
  });

  it('forwards the AbortSignal', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: {} }));
    const controller = new AbortController();

    await fetchHealth(controller.signal);

    const init = fetchMock().mock.calls[0][1] as RequestInit;
    expect(init.signal).toBe(controller.signal);
  });

  it('throws on non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 503, statusText: 'Service Unavailable' })
    );

    await expect(fetchHealth()).rejects.toThrow(/Service Unavailable/);
  });
});
