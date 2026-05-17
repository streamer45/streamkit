// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { beforeEach, describe, expect, it } from 'vitest';

import { getCurrentPermissions, usePermissionStore, type Permissions } from './permissionStore';

const ALL_GRANTED: Permissions = {
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
  loadPlugins: true,
  deletePlugins: true,
  uploadAssets: true,
  deleteAssets: true,
  accessAllSessions: true,
};

const DENY_ALL: Permissions = {
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

beforeEach(() => {
  usePermissionStore.getState().reset();
});

describe('usePermissionStore initial state', () => {
  it('starts with role="unknown", null permissions, and isLoading=true', () => {
    const state = usePermissionStore.getState();
    expect(state.role).toBe('unknown');
    expect(state.permissions).toBeNull();
    expect(state.isLoading).toBe(true);
  });
});

describe('usePermissionStore actions', () => {
  it('setRole updates only the role', () => {
    usePermissionStore.getState().setRole('admin');
    expect(usePermissionStore.getState().role).toBe('admin');
    expect(usePermissionStore.getState().permissions).toBeNull();
  });

  it('setPermissions stores the value and flips isLoading to false', () => {
    usePermissionStore.getState().setPermissions(ALL_GRANTED);

    const state = usePermissionStore.getState();
    expect(state.permissions).toEqual(ALL_GRANTED);
    expect(state.isLoading).toBe(false);
  });

  it('setLoading toggles the flag independently', () => {
    usePermissionStore.getState().setLoading(false);
    expect(usePermissionStore.getState().isLoading).toBe(false);

    usePermissionStore.getState().setLoading(true);
    expect(usePermissionStore.getState().isLoading).toBe(true);
  });

  it('reset returns the store to its initial state', () => {
    usePermissionStore.getState().setRole('editor');
    usePermissionStore.getState().setPermissions(ALL_GRANTED);

    usePermissionStore.getState().reset();

    const state = usePermissionStore.getState();
    expect(state.role).toBe('unknown');
    expect(state.permissions).toBeNull();
    expect(state.isLoading).toBe(true);
  });
});

describe('getCurrentPermissions selector', () => {
  it('returns the stored permissions when set', () => {
    usePermissionStore.getState().setPermissions(ALL_GRANTED);
    expect(getCurrentPermissions()).toEqual(ALL_GRANTED);
  });

  it('falls back to deny-all defaults when permissions are null', () => {
    expect(getCurrentPermissions()).toEqual(DENY_ALL);
  });
});
