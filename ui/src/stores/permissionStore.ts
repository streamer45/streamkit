// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { create } from 'zustand';

export interface Permissions {
  // Session operations
  createSessions: boolean;
  destroySessions: boolean;
  listSessions: boolean;
  modifySessions: boolean;

  // Node operations
  tuneNodes: boolean;
  listNodes: boolean;

  // Sample pipeline operations
  listSamples: boolean;
  readSamples: boolean;
  writeSamples: boolean;
  deleteSamples: boolean;

  // Plugin operations
  loadPlugins: boolean;
  deletePlugins: boolean;

  // Asset operations
  uploadAssets: boolean;
  deleteAssets: boolean;

  // Other
  accessAllSessions: boolean;
}

interface PermissionStore {
  role: string;
  permissions: Permissions | null;
  isLoading: boolean;

  setRole: (role: string) => void;
  setPermissions: (permissions: Permissions) => void;
  setLoading: (isLoading: boolean) => void;
  reset: () => void;
}

// Default permissions - deny all for safety
const defaultPermissions: Permissions = {
  createSessions: false,
  destroySessions: false,
  listSessions: false,
  modifySessions: false,
  tuneNodes: false,
  listNodes: false,
  listSamples: false,
  readSamples: false,
  writeSamples: false,
  deleteSamples: false,
  loadPlugins: false,
  deletePlugins: false,
  uploadAssets: false,
  deleteAssets: false,
  accessAllSessions: false,
};

export const usePermissionStore = create<PermissionStore>((set) => ({
  role: 'unknown',
  permissions: null,
  isLoading: true,

  setRole: (role: string) => set({ role }),

  setPermissions: (permissions: Permissions) => set({ permissions, isLoading: false }),

  setLoading: (isLoading: boolean) => set({ isLoading }),

  reset: () =>
    set({
      role: 'unknown',
      permissions: null,
      isLoading: true,
    }),
}));

// Helper to get current permissions with fallback to defaults
export const getCurrentPermissions = (): Permissions => {
  const { permissions } = usePermissionStore.getState();
  return permissions || defaultPermissions;
};
