// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { createLogStream, fetchLogs } from './logs';

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

describe('fetchLogs', () => {
  const PAYLOAD = { lines: ['a', 'b'], next_offset: 10, has_more: false, file_size: 100 };

  it('GETs /api/v1/logs without query when no params provided', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: PAYLOAD }));

    const result = await fetchLogs();

    expect(result).toEqual(PAYLOAD);
    expect(fetchMock().mock.calls[0][0]).toBe('http://localhost:4545/api/v1/logs');
  });

  it('serializes every supported query parameter', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: PAYLOAD }));

    await fetchLogs({
      offset: 50,
      limit: 100,
      direction: 'backward',
      filter: 'foo bar',
      level: 'error',
    });

    const calledUrl = fetchMock().mock.calls[0][0] as string;
    expect(calledUrl).toContain('offset=50');
    expect(calledUrl).toContain('limit=100');
    expect(calledUrl).toContain('direction=backward');
    expect(calledUrl).toContain('filter=foo+bar');
    expect(calledUrl).toContain('level=error');
    expect(calledUrl.startsWith('http://localhost:4545/api/v1/logs?')).toBe(true);
  });

  it('includes offset=0 explicitly when passed', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: PAYLOAD }));

    await fetchLogs({ offset: 0 });

    expect(fetchMock().mock.calls[0][0]).toBe('http://localhost:4545/api/v1/logs?offset=0');
  });

  it('throws a specific message on 404 (file logging disabled)', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 404, statusText: 'Not Found' })
    );

    await expect(fetchLogs()).rejects.toThrow(/Log file not available/);
  });

  it('throws on other non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error' })
    );

    await expect(fetchLogs()).rejects.toThrow(/Server Error/);
  });
});

describe('createLogStream', () => {
  let createdUrls: string[] = [];
  let createdInits: EventSourceInit[] = [];

  beforeEach(() => {
    createdUrls = [];
    createdInits = [];
    class FakeEventSource {
      url: string;
      init: EventSourceInit;
      close() {}
      constructor(url: string, init: EventSourceInit = {}) {
        this.url = url;
        this.init = init;
        createdUrls.push(url);
        createdInits.push(init);
      }
    }
    vi.stubGlobal('EventSource', FakeEventSource as unknown as typeof EventSource);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('constructs an EventSource with withCredentials at the bare stream URL', () => {
    createLogStream();

    expect(createdUrls[0]).toBe('http://localhost:4545/api/v1/logs/stream');
    expect(createdInits[0]).toEqual({ withCredentials: true });
  });

  it('appends filter and level query parameters', () => {
    createLogStream({ filter: 'foo', level: 'warn' });

    const url = new URL(createdUrls[0]);
    expect(url.searchParams.get('filter')).toBe('foo');
    expect(url.searchParams.get('level')).toBe('warn');
  });
});
