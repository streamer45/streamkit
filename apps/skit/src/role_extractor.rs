// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use axum::http::HeaderMap;
use std::sync::Arc;
use tracing::debug;

use crate::{permissions::Permissions, state::AppState};

/// Helper function to extract permissions from headers and state
///
/// For now, this reads from:
/// 1. A configured trusted role header (set by launcher or auth layer)
/// 2. SK_ROLE environment variable (fallback)
/// 3. Config default_role (final fallback)
pub fn get_permissions(headers: &HeaderMap, app_state: &Arc<AppState>) -> Permissions {
    let trusted_header = app_state.config.permissions.role_header.as_deref().map(|h| {
        // Normalize for HeaderMap lookups.
        h.trim().to_ascii_lowercase()
    });

    // Try to get role from the configured trusted header first (if enabled)
    let role_name = trusted_header
        .as_deref()
        .and_then(|header_name| headers.get(header_name))
        .and_then(|v| v.to_str().ok())
        .map(std::string::ToString::to_string)
        // Fallback to environment variable
        .or_else(|| std::env::var("SK_ROLE").ok())
        // Fallback to default role from config
        .unwrap_or_else(|| app_state.config.permissions.default_role.clone());

    let perms = app_state.config.permissions.get_role(&role_name);
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
    let trusted_header = app_state.config.permissions.role_header.as_deref().map(|h| {
        // Normalize for HeaderMap lookups.
        h.trim().to_ascii_lowercase()
    });

    // Try to get role from the configured trusted header first (if enabled)
    let role_name = trusted_header
        .as_deref()
        .and_then(|header_name| headers.get(header_name))
        .and_then(|v| v.to_str().ok())
        .map(std::string::ToString::to_string)
        // Fallback to environment variable
        .or_else(|| std::env::var("SK_ROLE").ok())
        // Fallback to default role from config
        .unwrap_or_else(|| app_state.config.permissions.default_role.clone());

    let perms = app_state.config.permissions.get_role(&role_name);
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
