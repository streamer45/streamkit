// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { usePermissionStore, type Permissions } from '@/stores/permissionStore';
import type { PermissionsInfo } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { getApiUrl } from './base';

const logger = getLogger('permissions');

/**
 * Response from the /api/v1/permissions endpoint
 */
interface PermissionsResponse {
  role: string;
  permissions: PermissionsInfo;
}

/**
 * Convert API PermissionsInfo to frontend Permissions
 */
function convertPermissions(apiPerms: PermissionsInfo): Permissions {
  return {
    createSessions: apiPerms.create_sessions,
    destroySessions: apiPerms.destroy_sessions,
    listSessions: apiPerms.list_sessions,
    modifySessions: apiPerms.modify_sessions,
    tuneNodes: apiPerms.tune_nodes,
    listNodes: apiPerms.list_nodes,
    listSamples: apiPerms.list_samples,
    readSamples: apiPerms.read_samples,
    writeSamples: apiPerms.write_samples,
    deleteSamples: apiPerms.delete_samples,
    loadPlugins: apiPerms.load_plugins,
    deletePlugins: apiPerms.delete_plugins,
    uploadAssets: apiPerms.upload_assets,
    deleteAssets: apiPerms.delete_assets,
    accessAllSessions: apiPerms.access_all_sessions,
  };
}

/**
 * Initialize permissions by fetching them from the server via HTTP
 */
export async function initializePermissions(): Promise<void> {
  try {
    const apiUrl = getApiUrl();
    const response = await fetch(`${apiUrl}/api/v1/permissions`, {
      method: 'GET',
      headers: {
        'Content-Type': 'application/json',
      },
    });

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`);
    }

    const data: PermissionsResponse = await response.json();
    const convertedPerms = convertPermissions(data.permissions);

    usePermissionStore.getState().setRole(data.role);
    usePermissionStore.getState().setPermissions(convertedPerms);

    logger.info(`Fetched from server - role: ${data.role}`, convertedPerms);
  } catch (error) {
    logger.error('Error fetching permissions:', error);

    // Fall back to deny-all permissions
    usePermissionStore.getState().setRole('unknown');
    usePermissionStore.getState().setPermissions({
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
    });
  }
}
