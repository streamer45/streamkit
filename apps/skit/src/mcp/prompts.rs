// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Prompt definitions and content builder helpers for the MCP module.
//!
//! The `#[prompt_router]` impl block defines the MCP prompts exposed via
//! `prompts/list` and `prompts/get`.  Content builder functions remain as
//! plain helpers called by the prompt methods.

use std::fmt::Write;

use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{GetPromptResult, PromptMessage, PromptMessageRole};
use rmcp::service::RequestContext;
use rmcp::{prompt, prompt_router, ErrorData as McpError, RoleServer};
use serde::Deserialize;
use streamkit_api::Pipeline;
use streamkit_core::NodeDefinition;
use tracing::info;

use super::{
    assemble_pipeline_state, extract_auth, filtered_node_definitions, resolve_session, StreamKitMcp,
};

/// Create a [`PromptRouter`] for [`StreamKitMcp`].
///
/// Exposed to the parent module so the struct constructor can store
/// the router alongside the tool router.
pub(super) fn create_prompt_router() -> PromptRouter<StreamKitMcp> {
    StreamKitMcp::prompt_router()
}

// ---------------------------------------------------------------------------
// Prompt argument structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
pub(super) struct DesignPipelinePromptArgs {
    /// Optional natural language description of the desired pipeline.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
pub(super) struct DebugPipelinePromptArgs {
    /// Session ID or name to debug.
    pub session_id: String,
}

// ---------------------------------------------------------------------------
// Prompt router
// ---------------------------------------------------------------------------

#[prompt_router]
impl StreamKitMcp {
    /// Design a StreamKit pipeline from scratch. Provides available node
    /// definitions, YAML format, connection rules, and workflow steps.
    #[prompt(
        name = "design_pipeline",
        description = "Design a StreamKit pipeline with available nodes and YAML format"
    )]
    async fn design_pipeline(
        &self,
        Parameters(args): Parameters<DesignPipelinePromptArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        let (_role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        let definitions = filtered_node_definitions(&self.app_state, &perms)?;

        let content = build_design_pipeline_content(&definitions, args.description.as_deref());

        let messages = vec![PromptMessage::new_text(PromptMessageRole::User, content)];
        Ok(GetPromptResult::new(messages).with_description("Design a StreamKit pipeline"))
    }

    /// Debug a running StreamKit session. Shows pipeline state, node states,
    /// connections, and diagnostic checklist.
    #[prompt(
        name = "debug_pipeline",
        description = "Debug a running StreamKit session with pipeline state and diagnostics"
    )]
    async fn debug_pipeline(
        &self,
        Parameters(args): Parameters<DebugPipelinePromptArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        let (session, _role_name, _perms) = resolve_session(
            &self.app_state,
            &args.session_id,
            &ctx,
            |p| p.list_sessions,
            "list_sessions",
        )
        .await?;

        let api_pipeline = assemble_pipeline_state(&session).await;

        let content = build_debug_pipeline_content(&args.session_id, &api_pipeline)
            .map_err(|e| McpError::internal_error(e, None))?;

        info!(session_id = %args.session_id, "MCP debug_pipeline prompt");

        let messages = vec![PromptMessage::new_text(PromptMessageRole::User, content)];
        Ok(GetPromptResult::new(messages)
            .with_description(format!("Debug StreamKit session '{}'", args.session_id)))
    }
}

// ---------------------------------------------------------------------------
// Content builder helpers
// ---------------------------------------------------------------------------

/// Build the `design_pipeline` prompt content string.
fn build_design_pipeline_content(
    definitions: &[NodeDefinition],
    description: Option<&str>,
) -> String {
    // Group definitions by first category (or "uncategorized").
    let mut by_category: std::collections::BTreeMap<String, Vec<&NodeDefinition>> =
        std::collections::BTreeMap::new();
    for def in definitions {
        let cat = def.categories.first().cloned().unwrap_or_else(|| "uncategorized".to_string());
        by_category.entry(cat).or_default().push(def);
    }

    let mut content = String::with_capacity(8192);

    content.push_str(
        "You are helping design a StreamKit pipeline. StreamKit pipelines are \
         defined in YAML with two sections: `nodes` and `connections`.\n\n",
    );

    // YAML format explanation
    content.push_str("## YAML Format\n\n");
    content.push_str("```yaml\nnodes:\n  <node_id>:\n    kind: <node_kind>\n");
    content.push_str("    params:  # optional, node-specific\n      <key>: <value>\n");
    content.push_str("connections:\n  - from_node: <id>\n    from_pin: <pin_name>\n");
    content.push_str("    to_node: <id>\n    to_pin: <pin_name>\n");
    content.push_str("    mode: reliable  # or best_effort\n```\n\n");

    // Available nodes by category
    content.push_str("## Available Nodes (by category)\n\n");

    for (category, defs) in &by_category {
        let _ = write!(content, "### {category}\n\n");
        for def in defs {
            let _ = write!(content, "- **`{}`**", def.kind);
            if let Some(desc) = &def.description {
                let _ = write!(content, " — {desc}");
            }
            content.push('\n');

            if !def.inputs.is_empty() {
                let pins: Vec<String> = def
                    .inputs
                    .iter()
                    .map(|p| format!("`{}` ({:?})", p.name, p.accepts_types))
                    .collect();
                let _ = write!(content, "  - Inputs: {}\n", pins.join(", "));
            }
            if !def.outputs.is_empty() {
                let pins: Vec<String> = def
                    .outputs
                    .iter()
                    .map(|p| format!("`{}` ({:?})", p.name, p.produces_type))
                    .collect();
                let _ = write!(content, "  - Outputs: {}\n", pins.join(", "));
            }
            // Param schema summary (skip trivially empty schemas)
            if def.param_schema != serde_json::json!({})
                && def.param_schema != serde_json::json!(null)
            {
                if let Some(props) = def.param_schema.get("properties") {
                    if let Some(obj) = props.as_object() {
                        if !obj.is_empty() {
                            let keys: Vec<&String> = obj.keys().collect();
                            let _ = write!(
                                content,
                                "  - Params: {}\n",
                                keys.iter()
                                    .map(|k| format!("`{k}`"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                        }
                    }
                }
            }
        }
        content.push('\n');
    }

    // Connection rules
    content.push_str("## Connection Rules\n\n");
    content.push_str(
        "- Pins have types (RawAudio, RawVideo, EncodedAudio, EncodedVideo, \
         Text, Transcription, Binary, Any, Passthrough, Custom) — only \
         matching types can connect. `Any` accepts all types; `Passthrough` \
         adapts to the connected input type.\n",
    );
    content.push_str(
        "- Pin cardinality: `One` (single connection), `Broadcast` (fan-out \
         to many), `Dynamic` (runtime-created pin family, e.g. mixer inputs).\n",
    );
    content.push_str(
        "- Connection modes: `reliable` (backpressure — sender blocks if \
         receiver is slow), `best_effort` (drop packets if the receiver \
         can't keep up).\n\n",
    );

    // Pipeline modes
    content.push_str("## Pipeline Modes\n\n");
    content.push_str(
        "- **dynamic**: Real-time, hot-reconfigurable pipeline. Nodes run \
         continuously and the graph can be mutated at runtime.\n",
    );
    content.push_str(
        "- **oneshot**: Stateless batch/request-response pipeline. Processes \
         a single request and exits. Uses synthetic `streamkit::http_input` / \
         `streamkit::http_output` nodes.\n\n",
    );

    // Workflow
    content.push_str("## Workflow\n\n");
    content.push_str("1. Design the YAML based on user requirements.\n");
    content.push_str(
        "2. Call the `validate_pipeline` tool to check for errors before creating a session.\n",
    );
    content.push_str("3. Fix any issues reported by validation.\n");
    content.push_str("4. Call `create_session` to start the pipeline.\n");

    // Optional user description
    if let Some(desc) = description {
        let _ = write!(content, "\n## User Request\n\n{desc}\n");
    }

    content
}

/// Build the `debug_pipeline` prompt content string.
fn build_debug_pipeline_content(
    session_id: &str,
    api_pipeline: &Pipeline,
) -> Result<String, String> {
    let pipeline_json = serde_json::to_string_pretty(api_pipeline)
        .map_err(|e| format!("serialization error: {e}"))?;

    let mut content = String::with_capacity(4096);

    let _ = write!(content, "You are debugging StreamKit session `{session_id}`.\n\n");

    // Current pipeline state
    content.push_str("## Current Pipeline State\n\n");
    content.push_str("```json\n");
    content.push_str(&pipeline_json);
    content.push_str("\n```\n\n");

    // Per-node state summary
    content.push_str("## Node States\n\n");
    let mut has_errors = false;
    for (id, node) in &api_pipeline.nodes {
        let state_str =
            node.state.as_ref().map_or_else(|| "unknown".to_string(), |s| format!("{s:?}"));
        let _ = write!(content, "- **`{id}`** (`{}`): {state_str}", node.kind);
        if let Some(ref state) = node.state {
            match state {
                streamkit_core::NodeState::Failed { reason } => {
                    has_errors = true;
                    let _ = write!(content, " — error: {reason}");
                },
                streamkit_core::NodeState::Recovering { reason, .. } => {
                    let _ = write!(content, " — recovering: {reason}");
                },
                streamkit_core::NodeState::Degraded { reason, .. } => {
                    let _ = write!(content, " — degraded: {reason}");
                },
                _ => {},
            }
        }
        content.push('\n');
    }

    // Connection summary
    if !api_pipeline.connections.is_empty() {
        content.push_str("\n## Connections\n\n");
        for conn in &api_pipeline.connections {
            let _ = write!(
                content,
                "- `{}`.`{}` → `{}`.`{}` ({})\n",
                conn.from_node,
                conn.from_pin,
                conn.to_node,
                conn.to_pin,
                match conn.mode {
                    streamkit_core::control::ConnectionMode::Reliable => "reliable",
                    streamkit_core::control::ConnectionMode::BestEffort => "best_effort",
                },
            );
        }
    }

    // Diagnostic guidance
    content.push_str("\n## Diagnostic Checklist\n\n");
    content.push_str("1. Are all nodes in a **running** state?\n");
    if has_errors {
        content.push_str(
            "2. **Errors detected** — review the error messages above and \
             check node parameters.\n",
        );
    } else {
        content.push_str("2. No errors reported so far.\n");
    }
    content.push_str(
        "3. Are all connections type-compatible? (Check that output pin types \
         match the connected input pin's accepted types.)\n",
    );
    content.push_str("4. Are all required node parameters set correctly?\n");
    content.push_str(
        "5. Are connection modes appropriate? (`reliable` for lossless \
         processing, `best_effort` for real-time streaming where drops are \
         acceptable.)\n\n",
    );

    // Remediation tools
    content.push_str("## Available Tools for Fixing Issues\n\n");
    content.push_str(
        "- `validate_batch` — dry-run a set of graph mutations (add/remove \
         nodes, connect/disconnect) without applying them.\n",
    );
    content.push_str(
        "- `apply_batch` — atomically apply validated mutations to the \
         running session.\n",
    );
    content.push_str(
        "- `tune_node` — send a control message to a specific node \
         (e.g. UpdateParams to change parameters at runtime).\n",
    );
    content.push_str(
        "- `validate_pipeline` — re-validate the pipeline YAML to catch \
         structural issues.\n",
    );
    content.push_str(
        "- `get_pipeline` — fetch the latest pipeline state (node states may \
         change over time).\n",
    );
    content.push_str("- `destroy_session` — tear down the session if it is unrecoverable.\n");

    Ok(content)
}
