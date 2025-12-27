// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { create } from 'zustand';

import { getApiUrl } from '@/services/base';
import type { PluginSummary } from '@/types/types';

type PluginState = {
  plugins: PluginSummary[];
  isLoaded: boolean;
  setPlugins: (plugins: PluginSummary[]) => void;
  setLoaded: (loaded: boolean) => void;
  upsertPlugin: (plugin: PluginSummary) => void;
  removePlugin: (kind: string) => void;
};

export const usePluginStore = create<PluginState>((set) => ({
  plugins: [],
  isLoaded: false,
  setPlugins: (plugins) => set(() => ({ plugins })),
  setLoaded: (loaded) => set(() => ({ isLoaded: loaded })),
  upsertPlugin: (plugin) =>
    set((state) => ({
      plugins: [plugin, ...state.plugins.filter((p) => p.kind !== plugin.kind)],
      isLoaded: true,
    })),
  removePlugin: (kind) =>
    set((state) => ({
      plugins: state.plugins.filter((p) => p.kind !== kind),
    })),
}));

let inFlight: Promise<void> | null = null;

export function ensurePluginsLoaded(): Promise<void> {
  const { isLoaded } = usePluginStore.getState();
  if (isLoaded) return Promise.resolve();
  if (inFlight) return inFlight;

  const apiUrl = getApiUrl();
  inFlight = fetch(`${apiUrl}/api/v1/plugins`)
    .then((res) => {
      if (!res.ok) {
        throw new Error(`Failed to fetch plugins: ${res.status} ${res.statusText}`);
      }
      return res.json() as Promise<PluginSummary[]>;
    })
    .then((plugins) => {
      const state = usePluginStore.getState();
      state.setPlugins(plugins);
      state.setLoaded(true);
    })
    .finally(() => {
      inFlight = null;
    });

  return inFlight;
}

export async function reloadPlugins(): Promise<void> {
  const apiUrl = getApiUrl();
  const res = await fetch(`${apiUrl}/api/v1/plugins`);
  if (!res.ok) {
    throw new Error(`Failed to fetch plugins: ${res.status} ${res.statusText}`);
  }

  const plugins: PluginSummary[] = await res.json();
  const state = usePluginStore.getState();
  state.setPlugins(plugins);
  state.setLoaded(true);
}
