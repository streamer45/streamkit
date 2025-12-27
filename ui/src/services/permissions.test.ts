// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { beforeEach, describe, expect, it, vi } from 'vitest';

import { usePermissionStore } from '@/stores/permissionStore';
import type { PermissionsInfo } from '@/types/generated/api-types';

import { initializePermissions } from './permissions';

// Mock getApiUrl to return a consistent test URL
vi.mock('./base', () => ({
  getApiUrl: () => 'http://localhost:4545',
}));

const DENY_ALL_PERMISSIONS = {
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
} as const;

function mockPermissionsFetchFailure(mock: unknown) {
  if (mock instanceof Error) {
    global.fetch = vi.fn().mockRejectedValue(mock);
    return;
  }

  global.fetch = vi.fn().mockResolvedValue(mock);
}

function expectDenyAllPermissions(testCaseName: string) {
  const state = usePermissionStore.getState();
  expect(state.role, `${testCaseName}: role`).toBe('unknown');
  expect(state.permissions, `${testCaseName}: permissions`).toEqual(DENY_ALL_PERMISSIONS);
}

describe('permissions', () => {
  beforeEach(() => {
    // Reset store to initial state
    usePermissionStore.getState().reset();

    // Clear all mocks
    vi.clearAllMocks();
  });

  describe('initializePermissions', () => {
    it('should fetch permissions and update store on success', async () => {
      const mockPermissions: PermissionsInfo = {
        create_sessions: true,
        destroy_sessions: true,
        list_sessions: true,
        modify_sessions: true,
        tune_nodes: true,
        list_nodes: true,
        list_samples: true,
        read_samples: true,
        write_samples: true,
        delete_samples: true,
        load_plugins: false,
        delete_plugins: false,
        upload_assets: true,
        delete_assets: false,
        access_all_sessions: true,
      };

      // Mock successful fetch
      global.fetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        json: async () => ({
          role: 'admin',
          permissions: mockPermissions,
        }),
      });

      await initializePermissions();

      const state = usePermissionStore.getState();

      // Verify role is set
      expect(state.role).toBe('admin');

      // Verify permissions are converted from snake_case to camelCase
      expect(state.permissions).toEqual({
        createSessions: true,
        destroySessions: true,
        listSessions: true,
        modifySessions: true,
        tuneNodes: true,
        listNodes: true,
        listSamples: true,
        readSamples: true,
        writeSamples: true,
        deleteSamples: true,
        loadPlugins: false,
        deletePlugins: false,
        uploadAssets: true,
        deleteAssets: false,
        accessAllSessions: true,
      });

      // Verify isLoading is set to false
      expect(state.isLoading).toBe(false);

      // Verify fetch was called with correct URL
      expect(global.fetch).toHaveBeenCalledWith('http://localhost:4545/api/v1/permissions', {
        method: 'GET',
        headers: {
          'Content-Type': 'application/json',
        },
      });
    });

    it('should handle different roles correctly', async () => {
      const mockPermissions: PermissionsInfo = {
        create_sessions: false,
        destroy_sessions: false,
        list_sessions: true,
        modify_sessions: false,
        tune_nodes: false,
        list_nodes: true,
        list_samples: true,
        read_samples: true,
        write_samples: false,
        delete_samples: false,
        load_plugins: false,
        delete_plugins: false,
        upload_assets: false,
        delete_assets: false,
        access_all_sessions: false,
      };

      global.fetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        json: async () => ({
          role: 'viewer',
          permissions: mockPermissions,
        }),
      });

      await initializePermissions();

      const state = usePermissionStore.getState();
      expect(state.role).toBe('viewer');
      expect(state.permissions?.listSessions).toBe(true);
      expect(state.permissions?.createSessions).toBe(false);
    });

    it('should fallback to deny-all on HTTP error', async () => {
      // Mock HTTP 500 error
      global.fetch = vi.fn().mockResolvedValue({
        ok: false,
        status: 500,
        statusText: 'Internal Server Error',
      });

      await initializePermissions();

      const state = usePermissionStore.getState();

      // Verify role is set to 'unknown'
      expect(state.role).toBe('unknown');

      // Verify all permissions are denied
      expect(state.permissions).toEqual({
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
    });

    it('should fallback to deny-all on HTTP 404', async () => {
      global.fetch = vi.fn().mockResolvedValue({
        ok: false,
        status: 404,
        statusText: 'Not Found',
      });

      await initializePermissions();

      const state = usePermissionStore.getState();
      expect(state.role).toBe('unknown');
      expect(state.permissions?.createSessions).toBe(false);
      expect(state.permissions?.listSessions).toBe(false);
    });

    it('should fallback to deny-all on network error', async () => {
      // Mock network error
      global.fetch = vi.fn().mockRejectedValue(new Error('Network error'));

      await initializePermissions();

      const state = usePermissionStore.getState();

      // Verify role is set to 'unknown'
      expect(state.role).toBe('unknown');

      // Verify all permissions are denied
      expect(state.permissions).toEqual(DENY_ALL_PERMISSIONS);
    });

    it('should fallback to deny-all on malformed JSON', async () => {
      // Mock response with invalid JSON
      global.fetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        json: async () => {
          throw new Error('Invalid JSON');
        },
      });

      await initializePermissions();

      const state = usePermissionStore.getState();
      expect(state.role).toBe('unknown');
      expect(state.permissions?.createSessions).toBe(false);
    });

    it('should preserve deny-all semantics across all failure modes', async () => {
      const testCases = [
        {
          name: 'HTTP 401',
          mock: { ok: false, status: 401, statusText: 'Unauthorized' },
        },
        {
          name: 'HTTP 403',
          mock: { ok: false, status: 403, statusText: 'Forbidden' },
        },
        {
          name: 'Timeout',
          mock: new Error('Request timeout'),
        },
      ];

      for (const testCase of testCases) {
        // Reset store
        usePermissionStore.getState().reset();

        mockPermissionsFetchFailure(testCase.mock);

        await initializePermissions();

        // All failures should result in deny-all
        expectDenyAllPermissions(testCase.name);
      }
    });
  });
});
