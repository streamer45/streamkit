// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { fetchConfig } from './config';

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

describe('fetchConfig', () => {
  it('GETs /api/v1/config and maps moq_gateway_url to camelCase', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: true, status: 200, json: { moq_gateway_url: 'http://moq.example/' } })
    );

    const result = await fetchConfig();

    expect(result).toEqual({ moqGatewayUrl: 'http://moq.example/' });
    expect(fetchMock().mock.calls[0][0]).toBe('http://localhost:4545/api/v1/config');
  });

  it('returns moqGatewayUrl=undefined when the server omits it', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: {} }));

    const result = await fetchConfig();

    expect(result).toEqual({ moqGatewayUrl: undefined });
  });

  it('throws on non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error' })
    );

    await expect(fetchConfig()).rejects.toThrow(/Server Error/);
  });
});
