// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use glob::Pattern;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use streamkit_api::PermissionsInfo;

/// Represents a set of permissions granted to a role
///
/// Note: We allow excessive bools here because permissions are inherently
/// independent boolean flags. Each field represents a distinct capability
/// that can be enabled or disabled. Converting to enums or state machines
/// would complicate the API without providing meaningful benefit.
/// Role-based permissions for access control.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
#[allow(clippy::struct_excessive_bools)]
pub struct Permissions {
    /// Can create new sessions
    #[serde(default)]
    pub create_sessions: bool,

    /// Can destroy sessions (their own or any depending on context)
    #[serde(default)]
    pub destroy_sessions: bool,

    /// Can list sessions (their own or all depending on context)
    #[serde(default)]
    pub list_sessions: bool,

    /// Can modify running sessions (add/remove nodes)
    #[serde(default)]
    pub modify_sessions: bool,

    /// Can tune parameters on running nodes
    #[serde(default)]
    pub tune_nodes: bool,

    /// Can upload and load plugins (WASM or native)
    #[serde(default)]
    pub load_plugins: bool,

    /// Can delete plugins
    #[serde(default)]
    pub delete_plugins: bool,

    /// Can view the list of available nodes
    #[serde(default)]
    pub list_nodes: bool,

    /// Can list sample pipelines
    #[serde(default)]
    pub list_samples: bool,

    /// Can read sample pipeline YAML
    #[serde(default)]
    pub read_samples: bool,

    /// Can save/update user pipelines in `[server].samples_dir/user`
    #[serde(default)]
    pub write_samples: bool,

    /// Can delete user pipelines in `[server].samples_dir/user`
    #[serde(default)]
    pub delete_samples: bool,

    /// Allowed sample pipeline paths (supports globs like "oneshot/*.yml").
    ///
    /// Paths are evaluated relative to `[server].samples_dir`.
    /// Empty list means no samples are allowed (deny by default).
    /// Use `["*"]` to allow everything.
    #[serde(default)]
    pub allowed_samples: Vec<String>,

    /// Allowed node types (e.g., "audio::gain", "transport::moq::*")
    /// Empty list means no nodes are allowed (deny by default).
    /// Use `["*"]` to allow everything.
    #[serde(default)]
    pub allowed_nodes: Vec<String>,

    /// Allowed plugin node kinds (e.g., "plugin::native::whisper", "plugin::wasm::gain", "plugin::*")
    /// Empty list means no plugins are allowed (deny by default).
    /// Use `["*"]` to allow everything.
    #[serde(default)]
    pub allowed_plugins: Vec<String>,

    /// Can access any user's sessions (admin capability)
    #[serde(default)]
    pub access_all_sessions: bool,

    /// Can upload audio assets
    #[serde(default)]
    pub upload_assets: bool,

    /// Can delete audio assets (user assets only)
    #[serde(default)]
    pub delete_assets: bool,

    /// Allowed audio asset paths (supports globs like "samples/audio/system/*.opus")
    /// Empty list means no assets are allowed (deny by default).
    /// Use `["*"]` to allow everything.
    #[serde(default)]
    pub allowed_assets: Vec<String>,
}

impl Permissions {
    /// Create admin role permissions with full access
    pub fn admin() -> Self {
        Self {
            create_sessions: true,
            destroy_sessions: true,
            list_sessions: true,
            modify_sessions: true,
            tune_nodes: true,
            load_plugins: true,
            delete_plugins: true,
            list_nodes: true,
            list_samples: true,
            read_samples: true,
            write_samples: true,
            delete_samples: true,
            allowed_samples: vec!["*".to_string()], // Wildcard = allow all
            allowed_nodes: vec!["*".to_string()],   // Wildcard = allow all
            allowed_plugins: vec!["*".to_string()], // Wildcard = allow all
            access_all_sessions: true,
            upload_assets: true,
            delete_assets: true,
            allowed_assets: vec!["*".to_string()], // Wildcard = allow all
        }
    }

    /// Create user role permissions with moderate access
    pub fn user() -> Self {
        Self {
            create_sessions: true,
            destroy_sessions: true,
            list_sessions: true,
            modify_sessions: true,
            tune_nodes: true,
            load_plugins: false,   // Users cannot load plugins (security risk)
            delete_plugins: false, // Users cannot delete plugins
            list_nodes: true,
            list_samples: true,
            read_samples: true,
            write_samples: true,
            delete_samples: true,
            allowed_samples: vec![
                // Users can access all standard samples
                "oneshot/*.yml".to_string(),
                "oneshot/*.yaml".to_string(),
                "dynamic/*.yml".to_string(),
                "dynamic/*.yaml".to_string(),
                "user/*.yml".to_string(),
                "user/*.yaml".to_string(),
            ],
            allowed_nodes: vec![
                // Users can use most nodes
                "audio::*".to_string(),
                "containers::*".to_string(),
                // Transport: allow MoQ, deny HTTP fetcher by default (SSRF risk)
                "transport::moq::*".to_string(),
                // Core: explicitly allow safe-ish nodes; deny core::file_writer by default (arbitrary write risk)
                "core::passthrough".to_string(),
                "core::file_reader".to_string(),
                "core::pacer".to_string(),
                "core::json_serialize".to_string(),
                "core::text_chunker".to_string(),
                "core::script".to_string(),
                "core::telemetry_tap".to_string(),
                "core::telemetry_out".to_string(),
                "core::sink".to_string(),
                // Plugins are represented as node kinds too (e.g. plugin::native::whisper).
                // This must be aligned with allowed_plugins for RBAC to work as expected.
                "plugin::*".to_string(),
            ],
            allowed_plugins: vec![
                // Users can use already-loaded plugins.
                //
                // Plugin kinds are always namespaced by the host:
                // - WASM:   plugin::wasm::<kind>
                // - Native: plugin::native::<kind>
                //
                // IMPORTANT: Native plugins run in-process with no sandbox; only load trusted
                // native plugins and restrict who can upload them (`load_plugins`) and whether
                // HTTP management is enabled (`[plugins].allow_http_management`).
                "plugin::*".to_string(),
            ],
            access_all_sessions: false, // Can only access own sessions
            upload_assets: true,
            delete_assets: true,
            allowed_assets: vec![
                // Users can access system and user audio assets
                "samples/audio/system/*".to_string(),
                "samples/audio/user/*".to_string(),
            ],
        }
    }

    /// Convert to API PermissionsInfo (without allowlists)
    pub const fn to_info(&self) -> PermissionsInfo {
        PermissionsInfo {
            create_sessions: self.create_sessions,
            destroy_sessions: self.destroy_sessions,
            list_sessions: self.list_sessions,
            modify_sessions: self.modify_sessions,
            tune_nodes: self.tune_nodes,
            load_plugins: self.load_plugins,
            delete_plugins: self.delete_plugins,
            list_nodes: self.list_nodes,
            list_samples: self.list_samples,
            read_samples: self.read_samples,
            write_samples: self.write_samples,
            delete_samples: self.delete_samples,
            access_all_sessions: self.access_all_sessions,
            upload_assets: self.upload_assets,
            delete_assets: self.delete_assets,
        }
    }

    /// Check if a sample pipeline path is allowed
    pub fn is_sample_allowed(&self, path: &str) -> bool {
        // Empty list means nothing is allowed (secure by default)
        // Use ["*"] wildcard to allow everything
        if self.allowed_samples.is_empty() {
            return false;
        }

        // Check against glob patterns
        self.allowed_samples
            .iter()
            .any(|pattern| Pattern::new(pattern).ok().is_some_and(|p| p.matches(path)))
    }

    /// Check if a node type is allowed
    pub fn is_node_allowed(&self, node_type: &str) -> bool {
        // Empty list means nothing is allowed (secure by default)
        // Use ["*"] wildcard to allow everything
        if self.allowed_nodes.is_empty() {
            return false;
        }

        // Check against patterns (supports wildcards like "audio::*")
        self.allowed_nodes
            .iter()
            .any(|pattern| Pattern::new(pattern).ok().is_some_and(|p| p.matches(node_type)))
    }

    /// Check if a plugin is allowed
    pub fn is_plugin_allowed(&self, plugin_name: &str) -> bool {
        // Empty list means nothing is allowed (secure by default)
        // Use ["*"] wildcard to allow everything
        if self.allowed_plugins.is_empty() {
            return false;
        }

        // Check against patterns (supports wildcards like "plugin::*")
        self.allowed_plugins
            .iter()
            .any(|pattern| Pattern::new(pattern).ok().is_some_and(|p| p.matches(plugin_name)))
    }

    /// Check if an audio asset path is allowed
    pub fn is_asset_allowed(&self, path: &str) -> bool {
        // Empty list means nothing is allowed (secure by default)
        // Use ["*"] wildcard to allow everything
        if self.allowed_assets.is_empty() {
            return false;
        }

        // Check against glob patterns
        self.allowed_assets
            .iter()
            .any(|pattern| Pattern::new(pattern).ok().is_some_and(|p| p.matches(path)))
    }
}

/// Permission configuration section for skit.toml.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PermissionsConfig {
    /// Default role for unauthenticated requests
    ///
    /// Note: StreamKit does not implement authentication by itself; this value becomes the
    /// effective role for any request that is not assigned a role by an external auth layer.
    /// For production deployments, set this to a least-privileged role and put an auth layer
    /// (or reverse proxy) in front of the server.
    #[serde(default = "default_default_role")]
    pub default_role: String,

    /// Optional trusted HTTP header used to select a role (e.g. "x-role" or "x-streamkit-role").
    ///
    /// If unset, StreamKit ignores role headers entirely and uses `SK_ROLE`/`default_role`.
    ///
    /// Security note: Only enable this when running behind a trusted reverse proxy or
    /// auth layer that (a) authenticates the caller and (b) strips any incoming header
    /// with the same name before setting it.
    #[serde(default)]
    pub role_header: Option<String>,

    /// Allow starting the server on a non-loopback address without a trusted role header.
    ///
    /// StreamKit does not implement authentication; without `role_header`, all requests fall back to
    /// `SK_ROLE`/`default_role`. Binding to a non-loopback address without a trusted auth layer is
    /// unsafe and the server will refuse to start unless this flag is set.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,

    /// Map of role name -> permissions
    #[serde(default = "default_roles")]
    pub roles: HashMap<String, Permissions>,

    /// Maximum concurrent dynamic sessions (global limit, applies to all users)
    /// None = unlimited
    #[serde(default)]
    pub max_concurrent_sessions: Option<usize>,

    /// Maximum concurrent oneshot pipelines (global limit)
    /// None = unlimited
    #[serde(default)]
    pub max_concurrent_oneshots: Option<usize>,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        Self {
            default_role: default_default_role(),
            role_header: None,
            allow_insecure_no_auth: false,
            roles: default_roles(),
            max_concurrent_sessions: None,
            max_concurrent_oneshots: None,
        }
    }
}

fn default_default_role() -> String {
    "admin".to_string()
}

fn default_roles() -> HashMap<String, Permissions> {
    let mut roles = HashMap::new();
    roles.insert("admin".to_string(), Permissions::admin());
    roles.insert("user".to_string(), Permissions::user());
    roles
}

impl PermissionsConfig {
    /// Get permissions for a role, falling back to default if not found
    pub fn get_role(&self, role_name: &str) -> Permissions {
        use tracing::warn;

        self.roles.get(role_name).map_or_else(
            || {
                warn!(
                    role = %role_name,
                    available_roles = ?self.roles.keys().collect::<Vec<_>>(),
                    default_role = %self.default_role,
                    "Role not found, falling back to default"
                );
                self.roles.get(&self.default_role).cloned().unwrap_or_default()
            },
            |perms| {
                tracing::debug!(
                    role = %role_name,
                    delete_plugins = perms.delete_plugins,
                    "Found role in config"
                );
                perms.clone()
            },
        )
    }

    /// Get the default role permissions
    #[allow(dead_code)]
    pub fn get_default(&self) -> Permissions {
        self.get_role(&self.default_role)
    }

    /// Check if we can accept a new session (global limit check)
    pub const fn can_accept_session(&self, current_count: usize) -> bool {
        match self.max_concurrent_sessions {
            None => true,
            Some(max) => current_count < max,
        }
    }

    /// Check if we can accept a new oneshot pipeline (global limit check)
    #[allow(dead_code)]
    pub const fn can_accept_oneshot(&self, current_count: usize) -> bool {
        match self.max_concurrent_oneshots {
            None => true,
            Some(max) => current_count < max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_path_matching() {
        let perms = Permissions {
            allowed_samples: vec!["demo/*.yml".to_string(), "custom/special.yml".to_string()],
            ..Default::default()
        };

        assert!(perms.is_sample_allowed("demo/moq_gain.yml"));
        assert!(perms.is_sample_allowed("demo/test.yml"));
        assert!(perms.is_sample_allowed("custom/special.yml"));
        assert!(!perms.is_sample_allowed("production/pipeline.yml"));
        assert!(!perms.is_sample_allowed("custom/other.yml"));
    }

    #[test]
    fn test_node_type_matching() {
        let perms = Permissions {
            allowed_nodes: vec![
                "audio::gain".to_string(),
                "audio::opus::*".to_string(),
                "transport::moq::*".to_string(),
            ],
            ..Default::default()
        };

        assert!(perms.is_node_allowed("audio::gain"));
        assert!(perms.is_node_allowed("audio::opus::encoder"));
        assert!(perms.is_node_allowed("audio::opus::decoder"));
        assert!(perms.is_node_allowed("transport::moq::peer"));
        assert!(!perms.is_node_allowed("audio::mixer"));
        assert!(!perms.is_node_allowed("transport::http::client"));
    }

    #[test]
    fn test_plugin_matching() {
        let perms = Permissions {
            allowed_plugins: vec![
                "plugin::wasm::gain".to_string(),
                "plugin::native::audio*".to_string(),
            ],
            ..Default::default()
        };

        assert!(perms.is_plugin_allowed("plugin::wasm::gain"));
        assert!(perms.is_plugin_allowed("plugin::native::audio_effect"));
        assert!(!perms.is_plugin_allowed("plugin::native::network"));
    }

    #[test]
    fn test_default_user_allows_plugin_nodes() {
        let user = Permissions::user();
        assert!(user.is_node_allowed("plugin::native::whisper"));
        assert!(user.is_node_allowed("plugin::native::kokoro"));
        assert!(user.is_node_allowed("plugin::wasm::gain_filter_rust"));
    }

    #[test]
    fn test_global_session_limits() {
        let config = PermissionsConfig { max_concurrent_sessions: Some(10), ..Default::default() };

        assert!(config.can_accept_session(0));
        assert!(config.can_accept_session(9));
        assert!(!config.can_accept_session(10));
        assert!(!config.can_accept_session(11));
    }

    #[test]
    fn test_global_oneshot_limits() {
        let config = PermissionsConfig { max_concurrent_oneshots: Some(5), ..Default::default() };

        assert!(config.can_accept_oneshot(0));
        assert!(config.can_accept_oneshot(4));
        assert!(!config.can_accept_oneshot(5));
    }

    #[test]
    fn test_user_role_defaults() {
        let user = Permissions::user();

        assert!(user.create_sessions);
        assert!(user.tune_nodes);
        assert!(!user.load_plugins); // SECURITY: users cannot load plugins (could be malicious)
        assert!(!user.delete_plugins);
        assert!(!user.allowed_samples.is_empty());
        assert!(!user.allowed_nodes.is_empty());
        assert!(!user.allowed_plugins.is_empty()); // Users can use already-loaded WASM and vetted native plugins
        assert!(!user.access_all_sessions);
    }

    #[test]
    fn test_empty_allowlist_denies_all() {
        let perms = Permissions::default(); // Has empty lists

        // Empty lists should deny everything
        assert!(!perms.is_sample_allowed("demo/test.yml"));
        assert!(!perms.is_node_allowed("audio::gain"));
        assert!(!perms.is_plugin_allowed("plugin::wasm::gain"));
    }

    #[test]
    fn test_wildcard_allows_all() {
        let perms = Permissions {
            allowed_samples: vec!["*".to_string()],
            allowed_nodes: vec!["*".to_string()],
            allowed_plugins: vec!["*".to_string()],
            ..Default::default()
        };

        // Wildcards should allow everything
        assert!(perms.is_sample_allowed("demo/test.yml"));
        assert!(perms.is_sample_allowed("any/path/file.yml"));
        assert!(perms.is_node_allowed("audio::gain"));
        assert!(perms.is_node_allowed("anything::else"));
        assert!(perms.is_plugin_allowed("plugin::wasm::gain"));
        assert!(perms.is_plugin_allowed("plugin::native::anything"));
    }

    #[test]
    fn test_session_ownership_filtering() {
        // Simulate sessions with different creators
        struct MockSession {
            id: String,
            created_by: Option<String>,
        }

        let sessions = [
            MockSession { id: "session1".to_string(), created_by: Some("user_alice".to_string()) },
            MockSession { id: "session2".to_string(), created_by: Some("user_bob".to_string()) },
            MockSession { id: "session3".to_string(), created_by: Some("user_alice".to_string()) },
            MockSession { id: "session4".to_string(), created_by: None }, // Legacy session
        ];

        // User without access_all_sessions permission
        let user_perms = Permissions::user();
        assert!(!user_perms.access_all_sessions);

        // Alice should only see her own sessions + legacy
        let alice_sessions: Vec<_> = sessions
            .iter()
            .filter(|s| {
                if user_perms.access_all_sessions {
                    return true;
                }
                s.created_by.as_ref().is_none_or(|creator| creator == "user_alice")
            })
            .collect();
        assert_eq!(alice_sessions.len(), 3); // session1, session3, session4
        assert_eq!(alice_sessions[0].id, "session1");
        assert_eq!(alice_sessions[1].id, "session3");
        assert_eq!(alice_sessions[2].id, "session4");

        // Bob should only see his own sessions + legacy
        let bob_sessions: Vec<_> = sessions
            .iter()
            .filter(|s| {
                if user_perms.access_all_sessions {
                    return true;
                }
                s.created_by.as_ref().is_none_or(|creator| creator == "user_bob")
            })
            .collect();
        assert_eq!(bob_sessions.len(), 2); // session2, session4
        assert_eq!(bob_sessions[0].id, "session2");
        assert_eq!(bob_sessions[1].id, "session4");

        // Admin with access_all_sessions should see everything
        let admin_perms = Permissions::admin();
        assert!(admin_perms.access_all_sessions);

        let admin_session_count = sessions
            .iter()
            .filter(|s| {
                if admin_perms.access_all_sessions {
                    return true;
                }
                s.created_by.as_ref().is_none_or(|creator| creator == "admin")
            })
            .count();
        assert_eq!(admin_session_count, 4); // All sessions
    }
}
