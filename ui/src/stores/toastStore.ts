// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { create } from 'zustand';

export type ToastType = 'info' | 'success' | 'error';

export interface ToastItem {
  id: number;
  type: ToastType;
  message: string;
}

interface ToastStoreState {
  toasts: ToastItem[];
  nextId: number;
  addToast: (message: string, type: ToastType) => number;
  removeToast: (id: number) => void;
  clear: () => void;
}

export const useToastStore = create<ToastStoreState>((set, get) => ({
  toasts: [],
  nextId: 1,

  addToast: (message, type) => {
    const id = get().nextId;
    set((state) => ({
      toasts: [...state.toasts, { id, type, message }],
      nextId: id + 1,
    }));
    return id;
  },

  removeToast: (id) =>
    set((state) => ({
      toasts: state.toasts.filter((t) => t.id !== id),
    })),

  clear: () => set({ toasts: [] }),
}));

export function showToast(message: string, type: ToastType = 'info'): number {
  const id = useToastStore.getState().addToast(message, type);
  window.setTimeout(() => {
    useToastStore.getState().removeToast(id);
  }, 3000);
  return id;
}
