// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Build scripts communicate with Cargo via stdout (`cargo:` directives).
// Allow `println!` here to emit those directives.
#![allow(clippy::disallowed_macros)]

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-env-changed=SKIT_BUILD_HASH");
    println!("cargo:rerun-if-env-changed=GIT_SHA");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");

    let git_head = find_git_head();
    if let Some(head_path) = git_head.as_ref() {
        println!("cargo:rerun-if-changed={}", head_path.display());

        if let Ok(head_ref) = fs::read_to_string(head_path) {
            if let Some(reference) = head_ref.trim().strip_prefix("ref: ") {
                if let Some(repo_root) = head_path.parent().and_then(|dir| dir.parent()) {
                    let ref_path = repo_root.join(reference);
                    println!("cargo:rerun-if-changed={}", ref_path.display());
                }
            }
        }
    }

    let hash = read_env_hash("SKIT_BUILD_HASH")
        .or_else(|| read_env_hash("GIT_SHA"))
        .or_else(|| read_env_hash("GITHUB_SHA"))
        .or_else(|| {
            git_hash(git_head.as_ref().and_then(|path| path.parent()).and_then(|p| p.parent()))
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=SKIT_BUILD_HASH={hash}");
}

fn read_env_hash(var: &str) -> Option<String> {
    let value = env::var(var).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn git_hash(repo_root: Option<&Path>) -> Option<String> {
    let mut command = Command::new("git");
    command.args(["rev-parse", "HEAD"]);

    if let Some(root) = repo_root {
        command.current_dir(root);
    }

    let output = command.output().ok()?;

    if !output.status.success() {
        return None;
    }

    let hash = String::from_utf8(output.stdout).ok()?;
    let trimmed = hash.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn find_git_head() -> Option<PathBuf> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").ok()?;
    let mut dir = PathBuf::from(manifest_dir);

    loop {
        let head_path = dir.join(".git").join("HEAD");
        if head_path.exists() {
            return Some(head_path);
        }

        if !dir.pop() {
            break;
        }
    }

    None
}
