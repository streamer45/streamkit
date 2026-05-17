// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { PluginSummary } from '@/types/types';

import { ensurePluginsLoaded, reloadPlugins, usePluginStore } from './pluginStore';

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

const PLUGIN_A: PluginSummary = {
  kind: 'plugin::native::a',
  original_kind: 'a',
  file_name: 'a.so',
  categories: ['audio'],
  loaded_at_ms: 1,
  plugin_type: 'native',
} as unknown as PluginSummary;

const PLUGIN_B: PluginSummary = {
  kind: 'plugin::native::b',
  original_kind: 'b',
  file_name: 'b.so',
  categories: ['video'],
  loaded_at_ms: 2,
  plugin_type: 'native',
} as unknown as PluginSummary;

const fetchMock = () => global.fetch as ReturnType<typeof vi.fn>;

beforeEach(() => {
  global.fetch = vi.fn() as never;
  usePluginStore.setState({ plugins: [], isLoaded: false });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('usePluginStore initial state', () => {
  it('starts with an empty plugin list and isLoaded=false', () => {
    const state = usePluginStore.getState();
    expect(state.plugins).toEqual([]);
    expect(state.isLoaded).toBe(false);
  });
});

describe('usePluginStore actions', () => {
  it('setPlugins replaces the list', () => {
    usePluginStore.getState().setPlugins([PLUGIN_A, PLUGIN_B]);
    expect(usePluginStore.getState().plugins).toEqual([PLUGIN_A, PLUGIN_B]);
  });

  it('setLoaded toggles the flag', () => {
    usePluginStore.getState().setLoaded(true);
    expect(usePluginStore.getState().isLoaded).toBe(true);
  });

  it('upsertPlugin prepends a new plugin and marks the store loaded', () => {
    usePluginStore.getState().upsertPlugin(PLUGIN_A);

    const state = usePluginStore.getState();
    expect(state.plugins).toEqual([PLUGIN_A]);
    expect(state.isLoaded).toBe(true);
  });

  it('upsertPlugin replaces an existing plugin with the same kind', () => {
    usePluginStore.getState().setPlugins([PLUGIN_A, PLUGIN_B]);

    const updated = { ...PLUGIN_A, file_name: 'a-v2.so' };
    usePluginStore.getState().upsertPlugin(updated);

    expect(usePluginStore.getState().plugins.map((p) => p.kind)).toEqual([
      PLUGIN_A.kind,
      PLUGIN_B.kind,
    ]);
    expect(usePluginStore.getState().plugins[0]).toEqual(updated);
  });

  it('removePlugin drops the plugin matching the kind', () => {
    usePluginStore.getState().setPlugins([PLUGIN_A, PLUGIN_B]);

    usePluginStore.getState().removePlugin(PLUGIN_A.kind);

    expect(usePluginStore.getState().plugins).toEqual([PLUGIN_B]);
  });

  it('removePlugin is a no-op for an unknown kind', () => {
    usePluginStore.getState().setPlugins([PLUGIN_A]);

    usePluginStore.getState().removePlugin('plugin::native::missing');

    expect(usePluginStore.getState().plugins).toEqual([PLUGIN_A]);
  });
});

describe('ensurePluginsLoaded', () => {
  it('GETs /api/v1/plugins, populates the store, and flips isLoaded', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [PLUGIN_A] }));

    await ensurePluginsLoaded();

    const state = usePluginStore.getState();
    expect(state.plugins).toEqual([PLUGIN_A]);
    expect(state.isLoaded).toBe(true);
    expect(fetchMock().mock.calls[0][0]).toBe('http://localhost:4545/api/v1/plugins');
  });

  it('short-circuits when plugins are already loaded', async () => {
    usePluginStore.setState({ isLoaded: true });

    await ensurePluginsLoaded();

    expect(fetchMock()).not.toHaveBeenCalled();
  });

  it('deduplicates concurrent calls (single-flight)', async () => {
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [PLUGIN_A] }));

    await Promise.all([ensurePluginsLoaded(), ensurePluginsLoaded(), ensurePluginsLoaded()]);

    expect(fetchMock()).toHaveBeenCalledTimes(1);
  });

  it('clears in-flight state on failure so a later call can retry', async () => {
    fetchMock().mockResolvedValueOnce(
      mockResponse({ ok: false, status: 500, statusText: 'Server Error' })
    );
    await expect(ensurePluginsLoaded()).rejects.toThrow(/Failed to fetch plugins.*500/);

    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [PLUGIN_A] }));
    await ensurePluginsLoaded();

    expect(usePluginStore.getState().plugins).toEqual([PLUGIN_A]);
  });
});

describe('reloadPlugins', () => {
  it('replaces the list and flips isLoaded', async () => {
    usePluginStore.setState({ plugins: [PLUGIN_A], isLoaded: false });
    fetchMock().mockResolvedValue(mockResponse({ ok: true, status: 200, json: [PLUGIN_B] }));

    await reloadPlugins();

    expect(usePluginStore.getState().plugins).toEqual([PLUGIN_B]);
    expect(usePluginStore.getState().isLoaded).toBe(true);
  });

  it('throws on non-2xx', async () => {
    fetchMock().mockResolvedValue(
      mockResponse({ ok: false, status: 503, statusText: 'Unavailable' })
    );

    await expect(reloadPlugins()).rejects.toThrow(/Failed to fetch plugins.*503/);
  });
});
