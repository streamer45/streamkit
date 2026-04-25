// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Oneshot command generation helpers for the MCP module.

use std::fmt::Write;

use super::OneshotInput;

/// Shell-quote a value by wrapping it in single quotes and escaping any
/// embedded single quotes (`'` → `'\''`).
pub(super) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Return a heredoc delimiter that does not appear in `content`.
pub(super) fn unique_heredoc_delimiter(content: &str) -> String {
    let base = "PIPELINE_EOF";
    if !content.contains(base) {
        return base.to_string();
    }
    for i in 0u32.. {
        let candidate = format!("{base}_{i}");
        if !content.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

pub(super) fn generate_curl_command(
    yaml: &str,
    inputs: &[OneshotInput],
    output: &str,
    server_url: &str,
) -> String {
    let delim = unique_heredoc_delimiter(yaml);

    let mut cmd = String::new();
    let _ = writeln!(cmd, "# Save pipeline YAML to a temporary file, then run curl.");
    let _ = writeln!(cmd, "PIPELINE=$(mktemp /tmp/pipeline-XXXXXX.yaml)");
    let _ = writeln!(cmd, "cat > \"$PIPELINE\" <<'{delim}'");
    let _ = writeln!(cmd, "{yaml}");
    let _ = writeln!(cmd, "{delim}");
    let _ = writeln!(cmd);
    let url = format!("{server_url}/api/v1/process");
    let _ = write!(cmd, "curl -X POST {} \\\n  -F 'config=<'\"$PIPELINE\"''", shell_quote(&url));
    for input in inputs {
        let _ =
            write!(cmd, " \\\n  -F {}", shell_quote(&format!("{}=@{}", input.field, input.path)));
    }
    let _ = write!(cmd, " \\\n  -o {}", shell_quote(output));
    cmd
}

pub(super) fn generate_skit_cli_command(
    yaml: &str,
    inputs: &[OneshotInput],
    output: &str,
    server_url: &str,
) -> String {
    let delim = unique_heredoc_delimiter(yaml);

    let mut cmd = String::new();
    let _ = writeln!(cmd, "# Save pipeline YAML to a temporary file, then run the CLI.");
    let _ = writeln!(cmd, "PIPELINE=$(mktemp /tmp/pipeline-XXXXXX.yaml)");
    let _ = writeln!(cmd, "cat > \"$PIPELINE\" <<'{delim}'");
    let _ = writeln!(cmd, "{yaml}");
    let _ = writeln!(cmd, "{delim}");
    let _ = writeln!(cmd);

    // The CLI takes one positional input mapped to the "media" field,
    // plus optional --input field=path for additional inputs.
    let (primary, extras): (Vec<_>, Vec<_>) = inputs.iter().partition(|i| i.field == "media");

    if let Some(primary_input) = primary.first() {
        let _ = write!(
            cmd,
            "streamkit-client oneshot \"$PIPELINE\" {}",
            shell_quote(&primary_input.path)
        );
    } else if let Some(first) = inputs.first() {
        // No input named "media" — use the first as positional and re-add
        // it via --input so the server receives the correct field name.
        let _ = write!(cmd, "streamkit-client oneshot \"$PIPELINE\" {}", shell_quote(&first.path));
    } else {
        let _ = write!(cmd, "streamkit-client oneshot \"$PIPELINE\" <INPUT_FILE>");
    }

    let _ = write!(cmd, " {}", shell_quote(output));

    // Emit --input flags: when a "media" input exists, only extras need
    // flags; otherwise all inputs are emitted (the first was used as the
    // positional arg but with a non-"media" field name).
    if primary.is_empty() {
        for input in inputs {
            let _ =
                write!(cmd, " --input {}", shell_quote(&format!("{}={}", input.field, input.path)));
        }
    } else {
        for input in &extras {
            let _ =
                write!(cmd, " --input {}", shell_quote(&format!("{}={}", input.field, input.path)));
        }
    }

    let _ = write!(cmd, " --server {}", shell_quote(server_url));
    cmd
}
