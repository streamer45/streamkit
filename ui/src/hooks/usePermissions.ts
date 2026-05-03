// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { usePermissionStore, getCurrentPermissions } from '../stores/permissionStore';

export function usePermissions() {
  const { role, permissions, isLoading } = usePermissionStore();
  const currentPerms = permissions || getCurrentPermissions();

  return {
    role,
    isLoading,
    permissions: currentPerms,

    can: {
      createSession: currentPerms.createSessions,
      destroySession: currentPerms.destroySessions,
      listSessions: currentPerms.listSessions,
      modifySession: currentPerms.modifySessions,
      tuneNodes: currentPerms.tuneNodes,
      listNodes: currentPerms.listNodes,
      loadPlugin: currentPerms.loadPlugins,
      deletePlugin: currentPerms.deletePlugins,
      uploadAsset: currentPerms.uploadAssets,
      deleteAsset: currentPerms.deleteAssets,
      accessAllSessions: currentPerms.accessAllSessions,
      enterStaging: currentPerms.modifySessions,
      saveTemplate: currentPerms.createSessions,
      commitBatchChanges: currentPerms.modifySessions,
    },

    isAdmin: () => role === 'admin',
    hasAccess: () => currentPerms.listSessions || currentPerms.listNodes,
  };
}
