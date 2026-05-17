// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  createApiToken,
  createMoqToken,
  fetchAuthMe,
  listTokens,
  loginWithToken,
  logout,
  revokeToken,
} from './auth';

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

beforeEach(() => {
  global.fetch = vi.fn() as never;
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('fetchAuthMe', () => {
  it('GETs /api/v1/auth/me with credentials and returns parsed body', async () => {
    const body = { authenticated: true, auth_enabled: true, role: 'admin', jti: 'tok-1' };
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: body }));

    const result = await fetchAuthMe();

    expect(result).toEqual(body);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/auth/me');
    expect((init as RequestInit).method).toBe('GET');
    expect((init as RequestInit).credentials).toBe('include');
  });

  it('throws on non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error' })
    );

    await expect(fetchAuthMe()).rejects.toThrow(/Server Error/);
  });
});

describe('loginWithToken', () => {
  it('POSTs JSON body to /api/v1/auth/login', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: {} }));

    await loginWithToken('my-token');

    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/auth/login');
    expect((init as RequestInit).method).toBe('POST');
    expect((init as RequestInit).headers).toMatchObject({ 'Content-Type': 'application/json' });
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({ token: 'my-token' });
  });

  it('throws the response body text on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 401, statusText: 'Unauthorized', text: 'bad token' })
    );

    await expect(loginWithToken('x')).rejects.toThrow('bad token');
  });

  it('falls back to status text when body is empty', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 401, statusText: 'Unauthorized', text: '' })
    );

    await expect(loginWithToken('x')).rejects.toThrow(/Unauthorized/);
  });
});

describe('logout', () => {
  it('POSTs to /api/v1/auth/logout', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 204 }));

    await logout();

    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/auth/logout');
    expect((init as RequestInit).method).toBe('POST');
  });

  it('throws on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error', text: '' })
    );

    await expect(logout()).rejects.toThrow(/Server Error/);
  });
});

describe('listTokens', () => {
  it('GETs /api/v1/auth/tokens and returns parsed array', async () => {
    const tokens = [{ jti: 'a', token_type: 'api', role: 'admin' }];
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: tokens }));

    const result = await listTokens();

    expect(result).toEqual(tokens);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/auth/tokens');
    expect((init as RequestInit).method).toBe('GET');
  });

  it('throws on non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 403, statusText: 'Forbidden', text: 'no' })
    );

    await expect(listTokens()).rejects.toThrow('no');
  });
});

describe('createApiToken', () => {
  it('POSTs the request body to /api/v1/auth/tokens', async () => {
    const created = { token: 'jwt', jti: 'jti-1', exp: 123 };
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: created }));

    const result = await createApiToken({ role: 'editor', label: 'CI', ttl_secs: 3600 });

    expect(result).toEqual(created);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/auth/tokens');
    expect((init as RequestInit).method).toBe('POST');
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      role: 'editor',
      label: 'CI',
      ttl_secs: 3600,
    });
  });

  it('throws on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 400, statusText: 'Bad Request', text: 'invalid role' })
    );

    await expect(createApiToken({ role: 'nope' })).rejects.toThrow('invalid role');
  });
});

describe('revokeToken', () => {
  it('DELETEs the URL-encoded jti', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 204 }));

    await revokeToken('abc/def');

    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/auth/tokens/abc%2Fdef');
    expect((init as RequestInit).method).toBe('DELETE');
  });

  it('throws on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 404, statusText: 'Not Found', text: '' })
    );

    await expect(revokeToken('x')).rejects.toThrow(/Not Found/);
  });
});

describe('createMoqToken', () => {
  it('POSTs the request and defaults subscribe/publish to []', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: { token: 'jwt' } }));

    await createMoqToken({ root: 'broadcasts' });

    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/auth/moq-tokens');
    expect((init as RequestInit).method).toBe('POST');
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      root: 'broadcasts',
      subscribe: [],
      publish: [],
    });
  });

  it('passes through user-provided subscribe/publish arrays', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: { token: 'jwt' } }));

    await createMoqToken({ root: 'r', subscribe: ['a', 'b'], publish: ['c'], label: 'L' });

    const sent = JSON.parse(fetchMock().mock.calls[0][1].body as string);
    expect(sent).toEqual({
      root: 'r',
      subscribe: ['a', 'b'],
      publish: ['c'],
      label: 'L',
    });
  });

  it('throws on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 400, statusText: 'Bad Request', text: 'oops' })
    );

    await expect(createMoqToken({ root: 'r' })).rejects.toThrow('oops');
  });
});
