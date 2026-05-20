// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { createSession, listSessions, startPreview, stopPreview } from './sessions';

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

beforeEach(() => {
  global.fetch = vi.fn() as never;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('listSessions', () => {
  it('GETs /api/v1/sessions and returns the parsed array', async () => {
    const sessions = [{ id: 's1', name: null, created_at: '2025-01-01T00:00:00Z' }];
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: sessions }));

    const result = await listSessions();

    expect(result).toEqual(sessions);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/sessions');
    expect((init as RequestInit).method).toBe('GET');
    expect((init as RequestInit).headers).toMatchObject({ 'Content-Type': 'application/json' });
  });

  it('forwards the AbortSignal', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [] }));
    const controller = new AbortController();

    await listSessions(controller.signal);

    const init = fetchMock().mock.calls[0][1] as RequestInit;
    expect(init.signal).toBe(controller.signal);
  });

  it('throws with statusText on non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error' })
    );

    await expect(listSessions()).rejects.toThrow(/Server Error/);
  });
});

describe('createSession', () => {
  it('POSTs a trimmed name and yaml body to /api/v1/sessions', async () => {
    const created = { session_id: 's1', name: 'Foo', created_at: 'now' };
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 201, json: created }));

    const result = await createSession('  Foo  ', 'nodes: {}');

    expect(result).toEqual(created);
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/sessions');
    expect((init as RequestInit).method).toBe('POST');
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      name: 'Foo',
      yaml: 'nodes: {}',
    });
  });

  it('coerces empty/whitespace name to null', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: true, status: 201, json: { session_id: 'x', name: null, created_at: '' } })
    );

    await createSession('   ', 'yaml');

    const sent = JSON.parse(fetchMock().mock.calls[0][1].body as string);
    expect(sent.name).toBeNull();
  });

  it('accepts an explicit null name', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: true, status: 201, json: { session_id: 'x', name: null, created_at: '' } })
    );

    await createSession(null, 'yaml');

    const sent = JSON.parse(fetchMock().mock.calls[0][1].body as string);
    expect(sent.name).toBeNull();
  });

  it('throws the error text on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 400, statusText: 'Bad Request', text: 'invalid yaml' })
    );

    await expect(createSession(null, 'bad')).rejects.toThrow('invalid yaml');
  });
});

describe('startPreview', () => {
  it('POSTs to /api/v1/sessions/:id/preview with URL-encoded id', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({
        ok: true,
        status: 200,
        json: {
          preview_id: 'p1',
          gateway_path: '/g',
          broadcast: 'b',
          audio: true,
          video: false,
        },
      })
    );

    const result = await startPreview('sess/1', 'node-a', 'pin-b');

    expect(result.preview_id).toBe('p1');
    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/sessions/sess%2F1/preview');
    expect((init as RequestInit).method).toBe('POST');
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      tap_node: 'node-a',
      tap_pin: 'pin-b',
    });
  });

  it('omits tap_node/tap_pin when undefined', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({
        ok: true,
        status: 200,
        json: { preview_id: 'p', gateway_path: '', broadcast: '', audio: false, video: false },
      })
    );

    await startPreview('s');

    expect(JSON.parse(fetchMock().mock.calls[0][1].body as string)).toEqual({});
  });

  it('throws the error text on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 404, statusText: 'Not Found', text: 'no session' })
    );

    await expect(startPreview('s')).rejects.toThrow('no session');
  });
});

describe('stopPreview', () => {
  it('DELETEs the URL-encoded session+preview id', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 204 }));

    await stopPreview('s 1', 'p/2');

    const [url, init] = fetchMock().mock.calls[0];
    expect(url).toBe('http://localhost:4545/api/v1/sessions/s%201/preview/p%2F2');
    expect((init as RequestInit).method).toBe('DELETE');
  });

  it('throws on failure', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error', text: '' })
    );

    await expect(stopPreview('s', 'p')).rejects.toThrow(/Server Error/);
  });
});
