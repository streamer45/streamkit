// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { create } from 'zustand';

const keyForNode = (nodeId: string, sessionId?: string) =>
  sessionId ? `${sessionId}::${nodeId}` : nodeId;

type NodeParamsState = {
  paramsById: Record<string, Record<string, unknown>>;
  setParam: (nodeId: string, key: string, value: unknown, sessionId?: string) => void;
  getParam: (nodeId: string, key: string, sessionId?: string) => unknown | undefined;
  getParamsForNode: (nodeId: string, sessionId?: string) => Record<string, unknown> | undefined;
  resetNode: (nodeId: string, sessionId?: string) => void;
  resetSession: (sessionId: string) => void;
  clear: () => void;
};

export const useNodeParamsStore = create<NodeParamsState>((set, get) => ({
  paramsById: {},
  setParam: (nodeId, key, value, sessionId) =>
    set((state) => ({
      paramsById: {
        ...state.paramsById,
        [keyForNode(nodeId, sessionId)]: {
          ...(state.paramsById[keyForNode(nodeId, sessionId)] || {}),
          [key]: value,
        },
      },
    })),
  getParam: (nodeId, key, sessionId) => get().paramsById[keyForNode(nodeId, sessionId)]?.[key],
  getParamsForNode: (nodeId, sessionId) => get().paramsById[keyForNode(nodeId, sessionId)],
  resetNode: (nodeId, sessionId) =>
    set((state) => {
      const next = { ...state.paramsById };
      delete next[keyForNode(nodeId, sessionId)];
      return { paramsById: next };
    }),
  resetSession: (sessionId) =>
    set((state) => {
      const prefix = `${sessionId}::`;
      const next: Record<string, Record<string, unknown>> = {};
      for (const [k, v] of Object.entries(state.paramsById)) {
        if (!k.startsWith(prefix)) {
          next[k] = v;
        }
      }
      return { paramsById: next };
    }),
  clear: () => set({ paramsById: {} }),
}));

export type { NodeParamsState };

export const selectNodeParam =
  (nodeId: string, key: string, sessionId?: string) => (state: NodeParamsState) =>
    state.paramsById[keyForNode(nodeId, sessionId)]?.[key];

export const selectNodeParams = (nodeId: string, sessionId?: string) => (state: NodeParamsState) =>
  state.paramsById[keyForNode(nodeId, sessionId)];
