// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(clippy::expect_used)]

use figment::{
    providers::{Format, Serialized, Toml},
    Figment,
};
use std::path::PathBuf;

#[test]
fn samples_skit_toml_parses_and_matches_expected_defaults() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(|parent| parent.parent())
        .expect("streamkit-server should live under workspace_root/apps/skit");
    let sample_path = repo_root.join("samples/skit.toml");

    let figment = Figment::new().merge(Serialized::defaults(streamkit_server::Config::default()));
    let config: streamkit_server::Config = match figment.merge(Toml::file(&sample_path)).extract() {
        Ok(cfg) => cfg,
        Err(e) => panic!("samples/skit.toml should parse as streamkit_server::Config: {e}"),
    };

    assert_eq!(config.plugins.directory, ".plugins");
    assert_eq!(config.server.samples_dir, "./samples/pipelines");
    assert_eq!(config.server.max_body_size, 104_857_600);
    assert_eq!(
        config.server.cors.allowed_origins,
        vec![
            "http://localhost".to_string(),
            "https://localhost".to_string(),
            "http://localhost:*".to_string(),
            "https://localhost:*".to_string(),
            "http://127.0.0.1".to_string(),
            "https://127.0.0.1".to_string(),
            "http://127.0.0.1:*".to_string(),
            "https://127.0.0.1:*".to_string(),
        ]
    );

    assert_eq!(config.security.allowed_file_paths, vec!["samples/**".to_string()]);

    assert_eq!(config.permissions.default_role, "admin");
    assert_eq!(config.permissions.role_header, None);
    assert!(!config.permissions.allow_insecure_no_auth);
    assert!(config.permissions.roles.contains_key("admin"));
    assert!(config.permissions.roles.contains_key("demo"));
    assert!(config.permissions.roles.contains_key("user"));
    assert!(config.permissions.roles.contains_key("readonly"));

    let readonly = config.permissions.get_role("readonly");
    assert!(!readonly.create_sessions);
    assert!(!readonly.modify_sessions);
    assert!(!readonly.upload_assets);
    assert!(!readonly.delete_assets);

    assert!(config.script.global_fetch_allowlist.is_empty());
}
