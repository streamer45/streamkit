// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use axum::http::HeaderMap;
use std::sync::Arc;
use tracing::debug;

use crate::{permissions::Permissions, state::AppState};

/// Resolve the role name from headers / env / config default.
fn resolve_role_name(headers: &HeaderMap, app_state: &Arc<AppState>) -> String {
    let trusted_header = app_state.config.permissions.role_header.as_deref().map(|h| {
        // Normalize for HeaderMap lookups.
        h.trim().to_ascii_lowercase()
    });

    trusted_header
        .as_deref()
        .and_then(|header_name| headers.get(header_name))
        .and_then(|v| v.to_str().ok())
        .map(std::string::ToString::to_string)
        .or_else(|| std::env::var("SK_ROLE").ok())
        .unwrap_or_else(|| app_state.config.permissions.default_role.clone())
}

/// Augment a [`Permissions`] value with glob patterns for every registered
/// plugin asset type.
///
/// Patterns are derived from the actual `system_dir` / `user_dir` stored in
/// the registry, so they remain correct even when a plugin uses a non-default
/// directory layout.  Roles that allow `upload_assets` get both system and
/// user patterns; others get system-only.
fn augment_plugin_asset_permissions(perms: &mut Permissions, app_state: &Arc<AppState>) {
    let patterns = app_state.plugin_asset_registry.registered_permission_patterns();
    for (system_pattern, user_pattern) in &patterns {
        if !perms.allowed_assets.contains(system_pattern) {
            perms.allowed_assets.push(system_pattern.clone());
        }
        if perms.upload_assets && !perms.allowed_assets.contains(user_pattern) {
            perms.allowed_assets.push(user_pattern.clone());
        }
    }
}

/// Extract permissions from headers and state.
///
/// Reads from: trusted role header → `SK_ROLE` env var → config `default_role`.
/// Plugin asset type patterns are augmented dynamically so that default
/// roles don't need a broad `samples/*/` wildcard.
pub fn get_permissions(headers: &HeaderMap, app_state: &Arc<AppState>) -> Permissions {
    let role_name = resolve_role_name(headers, app_state);
    let mut perms = app_state.config.permissions.get_role(&role_name);
    augment_plugin_asset_permissions(&mut perms, app_state);

    debug!(
        role = %role_name,
        create_sessions = perms.create_sessions,
        destroy_sessions = perms.destroy_sessions,
        modify_sessions = perms.modify_sessions,
        list_samples = perms.list_samples,
        read_samples = perms.read_samples,
        write_samples = perms.write_samples,
        delete_samples = perms.delete_samples,
        load_plugins = perms.load_plugins,
        delete_plugins = perms.delete_plugins,
        "Extracted permissions for request"
    );
    perms
}

/// Extract role name and permissions (returns both for session ownership tracking)
pub fn get_role_and_permissions(
    headers: &HeaderMap,
    app_state: &Arc<AppState>,
) -> (String, Permissions) {
    let role_name = resolve_role_name(headers, app_state);
    let mut perms = app_state.config.permissions.get_role(&role_name);
    augment_plugin_asset_permissions(&mut perms, app_state);

    debug!(
        role = %role_name,
        create_sessions = perms.create_sessions,
        destroy_sessions = perms.destroy_sessions,
        modify_sessions = perms.modify_sessions,
        list_samples = perms.list_samples,
        read_samples = perms.read_samples,
        write_samples = perms.write_samples,
        delete_samples = perms.delete_samples,
        load_plugins = perms.load_plugins,
        delete_plugins = perms.delete_plugins,
        "Extracted role and permissions for request"
    );
    (role_name, perms)
}
