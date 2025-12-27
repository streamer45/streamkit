// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { usePermissionStore, getCurrentPermissions } from '../stores/permissionStore';

/**
 * Hook to access user permissions throughout the UI
 */
export function usePermissions() {
  const { role, permissions, isLoading } = usePermissionStore();
  const currentPerms = permissions || getCurrentPermissions();

  return {
    role,
    isLoading,
    permissions: currentPerms,

    // Convenience flags for common permission checks
    can: {
      // Session operations
      createSession: currentPerms.createSessions,
      destroySession: currentPerms.destroySessions,
      listSessions: currentPerms.listSessions,
      modifySession: currentPerms.modifySessions,

      // Node operations
      tuneNodes: currentPerms.tuneNodes,
      listNodes: currentPerms.listNodes,

      // Plugin operations
      loadPlugin: currentPerms.loadPlugins,
      deletePlugin: currentPerms.deletePlugins,

      // Asset operations
      uploadAsset: currentPerms.uploadAssets,
      deleteAsset: currentPerms.deleteAssets,

      // Advanced
      accessAllSessions: currentPerms.accessAllSessions,

      // Composite permissions
      enterStaging: currentPerms.modifySessions,
      saveTemplate: currentPerms.createSessions, // Saving templates requires creating sessions
      commitBatchChanges: currentPerms.modifySessions,
    },

    // Helper to check if user is admin
    isAdmin: () => role === 'admin',

    // Helper to check if user has basic access
    hasAccess: () => currentPerms.listSessions || currentPerms.listNodes,
  };
}
