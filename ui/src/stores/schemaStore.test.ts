// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { PacketTypeMeta } from '@/types/generated/api-types';
import type { NodeDefinition } from '@/types/types';

import { usePacketTypeRegistryStore } from './packetTypeRegistry';
import {
  ensureSchemasLoaded,
  reloadSchemas,
  syncPluginSchemas,
  useSchemaStore,
} from './schemaStore';

vi.mock('@/services/base', () => ({
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

const META: PacketTypeMeta = {
  id: 'RawAudio',
  label: 'Raw Audio',
  color: '#ff0000',
  display_template: null,
  compatibility: 'Strict',
} as unknown as PacketTypeMeta;

const NODE: NodeDefinition = {
  kind: 'core::mic',
  param_schema: {},
  inputs: [],
  outputs: [],
  categories: ['audio'],
  bidirectional: false,
} as unknown as NodeDefinition;

const fetchMock = () => global.fetch as ReturnType<typeof vi.fn>;

const mockSchemaFetch = (
  typesRes: ReturnType<typeof mockResponse>,
  nodesRes: ReturnType<typeof mockResponse>
) => {
  fetchMock().mockImplementation((url: string) => {
    if (url.includes('/packets')) return Promise.resolve(typesRes);
    if (url.includes('/nodes')) return Promise.resolve(nodesRes);
    return Promise.reject(new Error(`unexpected url: ${url}`));
  });
};

beforeEach(() => {
  global.fetch = vi.fn() as never;
  useSchemaStore.setState({ nodeDefinitions: [], isLoaded: false });
  usePacketTypeRegistryStore.getState().clear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useSchemaStore initial state', () => {
  it('starts with empty node definitions and isLoaded=false', () => {
    const state = useSchemaStore.getState();
    expect(state.nodeDefinitions).toEqual([]);
    expect(state.isLoaded).toBe(false);
  });

  it('setNodeDefinitions updates the array', () => {
    useSchemaStore.getState().setNodeDefinitions([NODE]);
    expect(useSchemaStore.getState().nodeDefinitions).toEqual([NODE]);
  });

  it('setLoaded toggles the flag', () => {
    useSchemaStore.getState().setLoaded(true);
    expect(useSchemaStore.getState().isLoaded).toBe(true);
  });
});

describe('ensureSchemasLoaded', () => {
  it('fetches packets and node definitions, populates both stores, and flips isLoaded', async () => {
    mockSchemaFetch(
      mockResponse({ ok: true, status: 200, json: [META] }),
      mockResponse({ ok: true, status: 200, json: [NODE] })
    );

    await ensureSchemasLoaded();

    expect(useSchemaStore.getState().nodeDefinitions).toEqual([NODE]);
    expect(useSchemaStore.getState().isLoaded).toBe(true);
    expect(usePacketTypeRegistryStore.getState().metasById).toEqual({ RawAudio: META });
  });

  it('short-circuits when schemas are already loaded', async () => {
    useSchemaStore.setState({ isLoaded: true });

    await ensureSchemasLoaded();

    expect(fetchMock()).not.toHaveBeenCalled();
  });

  it('deduplicates concurrent calls (single-flight)', async () => {
    mockSchemaFetch(
      mockResponse({ ok: true, status: 200, json: [META] }),
      mockResponse({ ok: true, status: 200, json: [NODE] })
    );

    await Promise.all([ensureSchemasLoaded(), ensureSchemasLoaded(), ensureSchemasLoaded()]);

    expect(fetchMock()).toHaveBeenCalledTimes(2);
  });

  it('throws when /packets fails', async () => {
    mockSchemaFetch(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error' }),
      mockResponse({ ok: true, status: 200, json: [NODE] })
    );

    await expect(ensureSchemasLoaded()).rejects.toThrow(/Failed to fetch packets.*500/);
  });

  it('throws when /nodes fails', async () => {
    mockSchemaFetch(
      mockResponse({ ok: true, status: 200, json: [META] }),
      mockResponse({ ok: false, status: 503, statusText: 'Unavailable' })
    );

    await expect(ensureSchemasLoaded()).rejects.toThrow(/Failed to fetch node definitions.*503/);
  });

  it('clears in-flight state on failure so a later call can retry', async () => {
    mockSchemaFetch(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error' }),
      mockResponse({ ok: true, status: 200, json: [NODE] })
    );

    await expect(ensureSchemasLoaded()).rejects.toThrow();

    // Second attempt with successful responses should re-fetch.
    mockSchemaFetch(
      mockResponse({ ok: true, status: 200, json: [META] }),
      mockResponse({ ok: true, status: 200, json: [NODE] })
    );

    await ensureSchemasLoaded();
    expect(useSchemaStore.getState().isLoaded).toBe(true);
  });
});

describe('reloadSchemas', () => {
  it('refreshes both packets and node definitions and marks loaded', async () => {
    useSchemaStore.setState({ nodeDefinitions: [NODE], isLoaded: false });
    mockSchemaFetch(
      mockResponse({ ok: true, status: 200, json: [META] }),
      mockResponse({ ok: true, status: 200, json: [{ ...NODE, kind: 'core::other' }] })
    );

    await reloadSchemas();

    expect(useSchemaStore.getState().nodeDefinitions.map((d) => d.kind)).toEqual(['core::other']);
    expect(useSchemaStore.getState().isLoaded).toBe(true);
    expect(usePacketTypeRegistryStore.getState().metasById).toEqual({ RawAudio: META });
  });

  it('throws when /packets fails', async () => {
    mockSchemaFetch(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error' }),
      mockResponse({ ok: true, status: 200, json: [NODE] })
    );

    await expect(reloadSchemas()).rejects.toThrow(/Failed to fetch packets/);
  });
});

describe('syncPluginSchemas', () => {
  it('returns immediately when all kinds are present in the current schema', async () => {
    useSchemaStore.setState({ nodeDefinitions: [NODE], isLoaded: true });

    await syncPluginSchemas(['core::mic']);

    expect(fetchMock()).not.toHaveBeenCalled();
  });

  it('triggers a reload when any kind is missing', async () => {
    useSchemaStore.setState({ nodeDefinitions: [NODE], isLoaded: true });
    mockSchemaFetch(
      mockResponse({ ok: true, status: 200, json: [META] }),
      mockResponse({ ok: true, status: 200, json: [NODE, { ...NODE, kind: 'plugin::whisper' }] })
    );

    await syncPluginSchemas(['plugin::whisper']);

    expect(useSchemaStore.getState().nodeDefinitions.map((d) => d.kind)).toEqual([
      'core::mic',
      'plugin::whisper',
    ]);
  });

  it('deduplicates concurrent calls', async () => {
    useSchemaStore.setState({ nodeDefinitions: [NODE], isLoaded: true });
    mockSchemaFetch(
      mockResponse({ ok: true, status: 200, json: [META] }),
      mockResponse({ ok: true, status: 200, json: [NODE, { ...NODE, kind: 'plugin::whisper' }] })
    );

    await Promise.all([
      syncPluginSchemas(['plugin::whisper']),
      syncPluginSchemas(['plugin::whisper']),
    ]);

    expect(fetchMock()).toHaveBeenCalledTimes(2);
  });
});
