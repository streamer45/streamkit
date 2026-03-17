// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { throttle } from 'lodash-es';
import { create } from 'zustand';
import { persist, createJSONStorage } from 'zustand/middleware';

interface NodePositionStore {
  positions: Record<string, Record<string, { x: number; y: number }>>;

  updateNodePosition: (
    sessionId: string,
    nodeId: string,
    position: { x: number; y: number }
  ) => void;
  getNodePositions: (sessionId: string) => Record<string, { x: number; y: number }>;
  clearSession: (sessionId: string) => void;
}

// Throttled localStorage wrapper to avoid excessive writes during position updates
const throttledSetItem = throttle(
  (name: string, value: string) => {
    try {
      localStorage.setItem(name, value);
    } catch {
      // ignore
    }
  },
  500,
  { leading: false, trailing: true }
);

const throttledStorage = {
  getItem: (name: string) => {
    try {
      return localStorage.getItem(name);
    } catch {
      return null;
    }
  },
  setItem: (name: string, value: string) => {
    throttledSetItem(name, value);
  },
  removeItem: (name: string) => {
    try {
      localStorage.removeItem(name);
    } catch {
      // ignore
    }
  },
};

export const useNodePositionStore = create<NodePositionStore>()(
  persist(
    (set, get) => ({
      positions: {},

      updateNodePosition: (sessionId, nodeId, position) =>
        set((state) => ({
          positions: {
            ...state.positions,
            [sessionId]: {
              ...state.positions[sessionId],
              [nodeId]: position,
            },
          },
        })),

      getNodePositions: (sessionId) => {
        return get().positions[sessionId] ?? {};
      },

      clearSession: (sessionId) =>
        set((state) => {
          // eslint-disable-next-line @typescript-eslint/no-unused-vars -- Destructure-to-exclude pattern: _removed captures the key to omit it from `rest`
          const { [sessionId]: _removed, ...rest } = state.positions;
          return { positions: rest };
        }),
    }),
    {
      name: 'node-positions-storage',
      version: 1,
      storage: createJSONStorage(() => throttledStorage),
    }
  )
);
