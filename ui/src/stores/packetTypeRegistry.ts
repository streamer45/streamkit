// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { create } from 'zustand';

import type { PacketTypeMeta } from '@/types/generated/api-types';

type PacketTypeRegistryState = {
  metasById: Record<string, PacketTypeMeta>;
  isLoaded: boolean;
  setMetas: (metas: PacketTypeMeta[]) => void;
  clear: () => void;
};

export const usePacketTypeRegistryStore = create<PacketTypeRegistryState>((set) => ({
  metasById: {},
  isLoaded: false,
  setMetas: (metas) =>
    set(() => ({
      metasById: Object.fromEntries(metas.map((m) => [m.id, m])),
      isLoaded: true,
    })),
  clear: () => set({ metasById: {}, isLoaded: false }),
}));

export function setPacketTypeRegistry(metas: PacketTypeMeta[]): void {
  usePacketTypeRegistryStore.getState().setMetas(metas);
}

export function getPacketTypeMeta(kind: string): PacketTypeMeta | undefined {
  return usePacketTypeRegistryStore.getState().metasById[kind];
}

export function isPacketTypeRegistryLoaded(): boolean {
  return usePacketTypeRegistryStore.getState().isLoaded;
}
