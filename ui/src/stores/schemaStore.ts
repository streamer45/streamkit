// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { create } from 'zustand';

import { getApiUrl } from '@/services/base';
import type { PacketTypeMeta } from '@/types/generated/api-types';
import type { NodeDefinition } from '@/types/types';

import { setPacketTypeRegistry } from './packetTypeRegistry';

type SchemaState = {
  nodeDefinitions: NodeDefinition[];
  isLoaded: boolean;
  setNodeDefinitions: (defs: NodeDefinition[]) => void;
  setLoaded: (loaded: boolean) => void;
};

export const useSchemaStore = create<SchemaState>((set) => ({
  nodeDefinitions: [],
  isLoaded: false,
  setNodeDefinitions: (defs) => set(() => ({ nodeDefinitions: defs })),
  setLoaded: (loaded) => set(() => ({ isLoaded: loaded })),
}));

let inFlight: Promise<void> | null = null;

/**
 * Ensures server-driven schemas are loaded exactly once (single-flight).
 * Fetches packet type registry and node definitions, updates zustand stores.
 */
export function ensureSchemasLoaded(): Promise<void> {
  const { isLoaded } = useSchemaStore.getState();
  if (isLoaded) return Promise.resolve();
  if (inFlight) return inFlight;

  inFlight = (async () => {
    const apiUrl = getApiUrl();
    const [typesRes, nodesRes] = await Promise.all([
      fetch(`${apiUrl}/api/v1/schema/packets`),
      fetch(`${apiUrl}/api/v1/schema/nodes`),
    ]);

    if (!typesRes.ok) {
      throw new Error(`Failed to fetch packets: ${typesRes.status} ${typesRes.statusText}`);
    }
    if (!nodesRes.ok) {
      throw new Error(
        `Failed to fetch node definitions: ${nodesRes.status} ${nodesRes.statusText}`
      );
    }

    const metas: PacketTypeMeta[] = await typesRes.json();
    setPacketTypeRegistry(metas);

    const nodeDefs: NodeDefinition[] = await nodesRes.json();
    useSchemaStore.getState().setNodeDefinitions(nodeDefs);
    useSchemaStore.getState().setLoaded(true);
  })().finally(() => {
    // Allow re-attempt on failure by clearing inFlight in finally
    inFlight = null;
  });

  return inFlight;
}

export async function reloadSchemas(): Promise<void> {
  const apiUrl = getApiUrl();
  const [typesRes, nodesRes] = await Promise.all([
    fetch(`${apiUrl}/api/v1/schema/packets`),
    fetch(`${apiUrl}/api/v1/schema/nodes`),
  ]);

  if (!typesRes.ok) {
    throw new Error(`Failed to fetch packets: ${typesRes.status} ${typesRes.statusText}`);
  }
  if (!nodesRes.ok) {
    throw new Error(`Failed to fetch node definitions: ${nodesRes.status} ${nodesRes.statusText}`);
  }

  const metas: PacketTypeMeta[] = await typesRes.json();
  setPacketTypeRegistry(metas);

  const nodeDefs: NodeDefinition[] = await nodesRes.json();
  const state = useSchemaStore.getState();
  state.setNodeDefinitions(nodeDefs);
  state.setLoaded(true);
}
