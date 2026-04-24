// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Embedded MCP (Model Context Protocol) server for StreamKit.
//!
//! Exposes StreamKit control-plane capabilities (node discovery, pipeline
//! validation, session management) as MCP tools over Streamable HTTP.
//! The endpoint reuses the existing Axum application state, auth, and
//! permission model — no separate bridge process required.

use std::future::Future;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, GetPromptRequestParams, GetPromptResult,
    ListPromptsResult, ListToolsResult, PaginatedRequestParams, Prompt, PromptArgument,
    PromptMessage, PromptMessageRole, ServerCapabilities, ServerInfo,
};
use rmcp::schemars;
use rmcp::service::RequestContext;
use rmcp::tool;
use rmcp::tool_router;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};

use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::permissions::Permissions;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Auth helper
// ---------------------------------------------------------------------------

/// Extract `(role_name, permissions)` from the HTTP request parts that `rmcp`
/// injects into the request-context extensions.
fn extract_auth(
    ctx: &RequestContext<RoleServer>,
    app_state: &Arc<AppState>,
) -> Result<(String, Permissions), McpError> {
    let parts = ctx
        .extensions
        .get::<http::request::Parts>()
        .ok_or_else(|| McpError::internal_error("missing HTTP request parts", None))?;
    Ok(crate::role_extractor::get_role_and_permissions(&parts.headers, app_state))
}

// ---------------------------------------------------------------------------
// MCP tool argument structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OneshotInput {
    /// Input field name matching a node ID in the pipeline (e.g., "input").
    pub field: String,
    /// Path to the input file on the local filesystem.
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GenerateOneshotCommandArgs {
    /// Pipeline YAML for the oneshot run.
    pub yaml: String,
    /// Input file(s) to include in the request.
    pub inputs: Vec<OneshotInput>,
    /// Path where the output should be saved.
    pub output: String,
    /// Server URL (defaults to "http://localhost:4545").
    #[serde(default)]
    pub server_url: Option<String>,
    /// Command format: "curl" or "skit-cli". Defaults to "curl".
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ValidatePipelineArgs {
    /// Pipeline YAML to validate.
    pub yaml: String,
    /// Optional mode: "dynamic" or "oneshot".
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateSessionArgs {
    /// Pipeline YAML for the new session.
    pub yaml: String,
    /// Optional human-readable session name.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SessionIdArgs {
    /// Session ID or name.
    pub session_id: String,
}

// ---------------------------------------------------------------------------
// MCP prompt argument structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DesignPipelinePromptArgs {
    /// Optional natural language description of the desired pipeline.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DebugPipelinePromptArgs {
    /// Session ID or name to debug.
    pub session_id: String,
}

// ---------------------------------------------------------------------------
// StreamKit MCP service
// ---------------------------------------------------------------------------

/// StreamKit MCP service implementing `rmcp::ServerHandler`.
#[derive(Clone)]
pub struct StreamKitMcp {
    app_state: Arc<AppState>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl StreamKitMcp {
    pub fn new(app_state: Arc<AppState>) -> Self {
        Self { app_state, tool_router: Self::tool_router() }
    }

    // -- list_nodes --------------------------------------------------------

    #[tool(
        description = "List available StreamKit node types with their schemas, pins, and categories. Returns permission-filtered node definitions including synthetic oneshot nodes."
    )]
    async fn list_nodes(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (_role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        let mut definitions = self
            .app_state
            .engine
            .registry
            .read()
            .map_err(|e| {
                McpError::internal_error(format!("Failed to read node registry: {e}"), None)
            })?
            .definitions();

        definitions.extend(crate::server::synthetic_node_definitions());

        definitions.retain(|def| {
            if !perms.is_node_allowed(&def.kind) {
                return false;
            }
            if def.kind.starts_with("plugin::") {
                return perms.is_plugin_allowed(&def.kind);
            }
            true
        });

        info!(count = definitions.len(), "MCP list_nodes");

        let json = serde_json::to_string_pretty(&definitions)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    // -- validate_pipeline -------------------------------------------------

    #[tool(
        description = "Validate a StreamKit pipeline YAML without creating a session. Returns diagnostics (errors, warnings) and the parsed graph. Optionally pass mode='dynamic' or mode='oneshot' to apply mode-specific rules."
    )]
    async fn validate_pipeline(
        &self,
        Parameters(args): Parameters<ValidatePipelineArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (_role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        if !perms.create_sessions {
            return Err(McpError::invalid_request(
                "Permission denied: create_sessions required",
                None,
            ));
        }

        let mode = match args.mode.as_deref() {
            Some("dynamic") => Some(crate::server::PipelineMode::Dynamic),
            Some("oneshot") => Some(crate::server::PipelineMode::Oneshot),
            None => None,
            Some(other) => {
                return Err(McpError::invalid_params(
                    format!("Invalid mode '{other}'. Must be 'dynamic' or 'oneshot'."),
                    None,
                ));
            },
        };

        let response =
            crate::server::validate_pipeline_yaml(&self.app_state, &perms, &args.yaml, mode)
                .map_err(|e| McpError::internal_error(e, None))?;

        let json = serde_json::to_string_pretty(&response)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    // -- create_session ----------------------------------------------------

    #[tool(
        description = "Create a new dynamic StreamKit session from pipeline YAML. Returns the session ID, generated name, and creation timestamp."
    )]
    async fn create_session(
        &self,
        Parameters(args): Parameters<CreateSessionArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        if !perms.create_sessions {
            return Err(McpError::invalid_request(
                "Permission denied: cannot create sessions",
                None,
            ));
        }

        // Parse & compile
        let user_pipeline = streamkit_api::yaml::parse_yaml(&args.yaml)
            .map_err(|e| McpError::invalid_params(format!("YAML parse error: {e}"), None))?;

        let engine_pipeline = streamkit_api::yaml::compile(user_pipeline).map_err(|e| {
            McpError::invalid_params(format!("Pipeline compilation error: {e}"), None)
        })?;

        if engine_pipeline.nodes.is_empty() {
            return Err(McpError::invalid_params(
                "Pipeline is empty. Add some nodes before creating a session.",
                None,
            ));
        }

        // Per-node permission and security checks.
        // create_session must reject forbidden nodes with an immediate error,
        // not the deferred diagnostic that check_mode produces.
        for (node_id, node) in &engine_pipeline.nodes {
            if crate::server::is_synthetic_kind(&node.kind) {
                return Err(McpError::invalid_params(
                    format!(
                        "Node '{node_id}' kind '{}' is oneshot-only and cannot be used in dynamic sessions",
                        node.kind
                    ),
                    None,
                ));
            }
            if !perms.is_node_allowed(&node.kind) {
                return Err(McpError::invalid_request(
                    format!("Permission denied: node '{node_id}' kind '{}' not allowed", node.kind),
                    None,
                ));
            }
            if node.kind.starts_with("plugin::") && !perms.is_plugin_allowed(&node.kind) {
                return Err(McpError::invalid_request(
                    format!(
                        "Permission denied: node '{node_id}' plugin '{}' not allowed",
                        node.kind
                    ),
                    None,
                ));
            }
        }

        // File-path security
        crate::server::check_file_path_security(&engine_pipeline, &self.app_state.config.security)
            .map_err(|e| McpError::invalid_params(e, None))?;

        // Pre-flight: reject early if over the session limit or name is taken,
        // avoiding wasted engine allocation.  The checks are re-verified under
        // the lock inside add_session for correctness.
        let (current_count, name_taken) = {
            let sm = self.app_state.session_manager.lock().await;
            (sm.session_count(), args.name.as_deref().is_some_and(|n| sm.is_name_taken(n)))
        };
        if let Some(ref session_name) = args.name {
            if name_taken {
                return Err(McpError::invalid_request(
                    format!("Session with name '{session_name}' already exists"),
                    None,
                ));
            }
        }
        if !self.app_state.config.permissions.can_accept_session(current_count) {
            return Err(McpError::invalid_request(
                "Maximum concurrent sessions limit reached",
                None,
            ));
        }

        let session = crate::session::Session::create(
            &self.app_state.engine,
            &self.app_state.config,
            args.name.clone(),
            self.app_state.event_tx.clone(),
            Some(role_name),
        )
        .await
        .map_err(|e| McpError::internal_error(format!("Failed to create session: {e}"), None))?;

        // Insert under the lock (re-checks limit and name uniqueness).
        let insert_result = {
            let mut sm = self.app_state.session_manager.lock().await;
            let count = sm.session_count();
            if self.app_state.config.permissions.can_accept_session(count) {
                sm.add_session(session.clone())
            } else {
                Err("Maximum concurrent sessions limit reached".to_string())
            }
        };
        if let Err(msg) = insert_result {
            warn!(error = %msg, "MCP create_session failed during insert");
            let _ = session.shutdown_and_wait().await;
            return Err(McpError::invalid_request(msg, None));
        }

        let session_id = session.id.clone();
        let session_name = session.name.clone();
        let created_at = crate::session::system_time_to_rfc3339(session.created_at);

        info!(session_id = %session_id, name = ?session_name, "MCP create_session");

        // Populate pipeline and send to engine
        crate::server::populate_session_pipeline(&session, &engine_pipeline).await;
        crate::server::send_pipeline_to_engine(&session, &engine_pipeline).await;

        // Broadcast event
        let event = streamkit_api::Event {
            message_type: streamkit_api::MessageType::Event,
            correlation_id: None,
            payload: streamkit_api::EventPayload::SessionCreated {
                session_id: session_id.clone(),
                name: session_name.clone(),
                created_at: created_at.clone(),
            },
        };
        if self.app_state.event_tx.send(crate::state::BroadcastEvent::to_all(event)).is_err() {
            debug!("No WebSocket clients connected to receive SessionCreated event");
        }

        let result = serde_json::json!({
            "session_id": session_id,
            "name": session_name,
            "created_at": created_at,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result)
                .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?,
        )]))
    }

    // -- list_sessions -----------------------------------------------------

    #[tool(
        description = "List active StreamKit sessions. Returns session IDs, names, and creation timestamps."
    )]
    async fn list_sessions(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        if !perms.list_sessions {
            return Err(McpError::invalid_request("Permission denied: cannot list sessions", None));
        }

        let sessions = self.app_state.session_manager.lock().await.list_sessions();
        let infos: Vec<streamkit_api::SessionInfo> = sessions
            .into_iter()
            .filter(|s| {
                if perms.access_all_sessions {
                    return true;
                }
                s.created_by.as_ref().is_none_or(|c| c == &role_name)
            })
            .map(|s| streamkit_api::SessionInfo {
                id: s.id,
                name: s.name,
                created_at: crate::session::system_time_to_rfc3339(s.created_at),
            })
            .collect();

        info!(count = infos.len(), "MCP list_sessions");

        let json = serde_json::to_string_pretty(&infos)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    // -- get_pipeline ------------------------------------------------------

    #[tool(
        description = "Get the full pipeline state for a StreamKit session, including nodes, connections, node states, view data, and runtime schemas."
    )]
    async fn get_pipeline(
        &self,
        Parameters(args): Parameters<SessionIdArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        if !perms.list_sessions {
            return Err(McpError::invalid_request("Permission denied: cannot view sessions", None));
        }

        let session = {
            let sm = self.app_state.session_manager.lock().await;
            sm.get_session_by_name_or_id(&args.session_id)
        };

        let Some(session) = session else {
            return Err(McpError::invalid_params(
                format!("Session '{}' not found", args.session_id),
                None,
            ));
        };

        if !perms.access_all_sessions
            && session.created_by.as_ref().is_some_and(|c| c != &role_name)
        {
            return Err(McpError::invalid_request(
                "Permission denied: you do not own this session",
                None,
            ));
        }

        let node_states = session.get_node_states().await.unwrap_or_default();
        let node_view_data = session.get_node_view_data().await.unwrap_or_default();
        let runtime_schemas = session.get_runtime_schemas().await.unwrap_or_default();

        let mut api_pipeline = {
            let pipeline = session.pipeline.lock().await;
            pipeline.clone()
        };
        for (id, node) in &mut api_pipeline.nodes {
            node.state = node_states.get(id).cloned();
        }
        if !node_view_data.is_empty() {
            api_pipeline.view_data = Some(Arc::unwrap_or_clone(node_view_data));
        }
        if !runtime_schemas.is_empty() {
            api_pipeline.runtime_schemas = Some(runtime_schemas);
        }

        info!(session_id = %args.session_id, "MCP get_pipeline");

        let json = serde_json::to_string_pretty(&api_pipeline)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    // -- generate_oneshot_command -------------------------------------------

    #[tool(
        description = "Generate a curl or skit-cli command to execute a oneshot (batch processing) pipeline. The oneshot runs through the HTTP data plane (POST /api/v1/process), not through MCP. Use validate_pipeline with mode='oneshot' first to ensure the YAML is valid."
    )]
    async fn generate_oneshot_command(
        &self,
        Parameters(args): Parameters<GenerateOneshotCommandArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (_role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        if !perms.create_sessions {
            return Err(McpError::invalid_request(
                "Permission denied: create_sessions required",
                None,
            ));
        }

        // Validate the YAML before generating a command.
        let validation = crate::server::validate_pipeline_yaml(
            &self.app_state,
            &perms,
            &args.yaml,
            Some(crate::server::PipelineMode::Oneshot),
        )
        .map_err(|e| McpError::internal_error(e, None))?;

        let validation_json = serde_json::to_value(&validation)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?;

        if validation_json["valid"] == false {
            let pretty = serde_json::to_string_pretty(&validation_json)
                .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?;
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "Pipeline validation failed. Fix the errors before generating a command:\n{pretty}"
            ))]));
        }

        let server_url = args.server_url.as_deref().unwrap_or("http://localhost:4545");
        let format = args.format.as_deref().unwrap_or("curl");

        let command = match format {
            "curl" => generate_curl_command(&args.yaml, &args.inputs, &args.output, server_url),
            "skit-cli" => {
                generate_skit_cli_command(&args.yaml, &args.inputs, &args.output, server_url)
            },
            other => {
                return Err(McpError::invalid_params(
                    format!("Invalid format '{other}'. Must be 'curl' or 'skit-cli'."),
                    None,
                ));
            },
        };

        info!(format, "MCP generate_oneshot_command");

        Ok(CallToolResult::success(vec![Content::text(command)]))
    }

    // -- destroy_session ---------------------------------------------------

    #[tool(
        description = "Destroy (stop and remove) a StreamKit session. Shuts down the engine and frees all resources."
    )]
    async fn destroy_session(
        &self,
        Parameters(args): Parameters<SessionIdArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        if !perms.destroy_sessions {
            return Err(McpError::invalid_request(
                "Permission denied: cannot destroy sessions",
                None,
            ));
        }

        let removed_session = {
            let mut sm = self.app_state.session_manager.lock().await;
            let Some(session) = sm.get_session_by_name_or_id(&args.session_id) else {
                return Err(McpError::invalid_params(
                    format!("Session '{}' not found", args.session_id),
                    None,
                ));
            };

            if !perms.access_all_sessions
                && session.created_by.as_ref().is_some_and(|c| c != &role_name)
            {
                warn!(
                    session_id = %args.session_id,
                    role = %role_name,
                    "MCP: blocked attempt to destroy session: not owner"
                );
                return Err(McpError::invalid_request(
                    "Permission denied: you do not own this session",
                    None,
                ));
            }

            sm.remove_session_by_id(&session.id)
        };

        let Some(session) = removed_session else {
            return Err(McpError::invalid_params(
                format!("Session '{}' not found", args.session_id),
                None,
            ));
        };

        let destroyed_id = session.id.clone();

        // Broadcast event
        let event = streamkit_api::Event {
            message_type: streamkit_api::MessageType::Event,
            correlation_id: None,
            payload: streamkit_api::EventPayload::SessionDestroyed {
                session_id: destroyed_id.clone(),
            },
        };
        if let Err(e) = self.app_state.event_tx.send(crate::state::BroadcastEvent::to_all(event)) {
            tracing::error!("Failed to broadcast SessionDestroyed event: {}", e);
        }

        // Background shutdown
        let shutdown_id = destroyed_id.clone();
        let tracker = self.app_state.shutdown_tracker.clone();
        let handle = tokio::spawn(async move {
            #[cfg(feature = "moq")]
            crate::server::preview::teardown_all_previews(&session).await;

            if let Err(e) = session.shutdown_and_wait().await {
                warn!(session_id = %shutdown_id, error = %e, "Error during engine shutdown");
                opentelemetry::global::meter("skit_server")
                    .u64_counter("session.shutdown.errors")
                    .build()
                    .add(1, &[]);
            } else {
                info!(session_id = %shutdown_id, "Session destroyed successfully via MCP");
            }
        });
        tracker.track(handle).await;

        info!(session_id = %destroyed_id, "MCP destroy_session");

        let result = serde_json::json!({ "session_id": destroyed_id });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result)
                .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?,
        )]))
    }
}

// ---------------------------------------------------------------------------
// Command generation helpers
// ---------------------------------------------------------------------------

/// Shell-quote a value by wrapping it in single quotes and escaping any
/// embedded single quotes (`'` → `'\''`).
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Return a heredoc delimiter that does not appear in `content`.
fn unique_heredoc_delimiter(content: &str) -> String {
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

fn generate_curl_command(
    yaml: &str,
    inputs: &[OneshotInput],
    output: &str,
    server_url: &str,
) -> String {
    use std::fmt::Write;

    let delim = unique_heredoc_delimiter(yaml);

    let mut cmd = String::new();
    let _ = writeln!(cmd, "# Save pipeline YAML to a temporary file, then run curl.");
    let _ = writeln!(cmd, "cat > /tmp/pipeline.yaml <<'{delim}'");
    let _ = writeln!(cmd, "{yaml}");
    let _ = writeln!(cmd, "{delim}");
    let _ = writeln!(cmd);
    let url = format!("{server_url}/api/v1/process");
    let _ = write!(cmd, "curl -X POST {} \\\n  -F 'config=</tmp/pipeline.yaml'", shell_quote(&url));
    for input in inputs {
        let _ =
            write!(cmd, " \\\n  -F {}", shell_quote(&format!("{}=@{}", input.field, input.path)));
    }
    let _ = write!(cmd, " \\\n  -o {}", shell_quote(output));
    cmd
}

fn generate_skit_cli_command(
    yaml: &str,
    inputs: &[OneshotInput],
    output: &str,
    server_url: &str,
) -> String {
    use std::fmt::Write;

    let delim = unique_heredoc_delimiter(yaml);

    let mut cmd = String::new();
    let _ = writeln!(cmd, "# Save pipeline YAML to a temporary file, then run the CLI.");
    let _ = writeln!(cmd, "cat > /tmp/pipeline.yaml <<'{delim}'");
    let _ = writeln!(cmd, "{yaml}");
    let _ = writeln!(cmd, "{delim}");
    let _ = writeln!(cmd);

    // The CLI takes one positional input mapped to the "media" field,
    // plus optional --input field=path for additional inputs.
    let (primary, extras): (Vec<_>, Vec<_>) = inputs.iter().partition(|i| i.field == "media");

    if let Some(primary_input) = primary.first() {
        let _ = write!(
            cmd,
            "streamkit-client oneshot /tmp/pipeline.yaml {}",
            shell_quote(&primary_input.path)
        );
    } else if let Some(first) = inputs.first() {
        // No input named "media" — use the first as positional and re-add
        // it via --input so the server receives the correct field name.
        let _ =
            write!(cmd, "streamkit-client oneshot /tmp/pipeline.yaml {}", shell_quote(&first.path));
    } else {
        let _ = write!(cmd, "streamkit-client oneshot /tmp/pipeline.yaml <INPUT_FILE>");
    }

    let _ = write!(cmd, " {}", shell_quote(output));

    // Emit --input flags: when a "media" input exists, only extras need
    // flags; otherwise all inputs are emitted (the first was used as the
    // positional arg but with a non-"media" field name).
    if !primary.is_empty() {
        for input in &extras {
            let _ =
                write!(cmd, " --input {}", shell_quote(&format!("{}={}", input.field, input.path)));
        }
    } else {
        for input in inputs {
            let _ =
                write!(cmd, " --input {}", shell_quote(&format!("{}={}", input.field, input.path)));
        }
    }

    let _ = write!(cmd, " --server {}", shell_quote(server_url));
    cmd
}

// ---------------------------------------------------------------------------
// ServerHandler trait impl
// ---------------------------------------------------------------------------

impl StreamKitMcp {
    // -- prompt helpers ----------------------------------------------------

    /// Build the prompt metadata list exposed via `prompts/list`.
    fn prompt_metadata() -> Vec<Prompt> {
        vec![
            Prompt::new(
                "design_pipeline",
                Some(
                    "Generate a StreamKit pipeline YAML from a natural-language \
                     description. Returns the full node catalogue (permission-filtered) \
                     together with format rules and a design workflow.",
                ),
                Some(vec![PromptArgument::new("description")
                    .with_description("Natural language description of the desired pipeline.")
                    .with_required(false)]),
            ),
            Prompt::new(
                "debug_pipeline",
                Some(
                    "Diagnose a running StreamKit session. Returns the current \
                     pipeline structure, per-node states, errors, and suggested \
                     diagnostic steps.",
                ),
                Some(vec![PromptArgument::new("session_id")
                    .with_description("Session ID or name to debug.")
                    .with_required(true)]),
            ),
        ]
    }

    /// Build the `design_pipeline` prompt content.
    async fn build_design_pipeline_prompt(
        &self,
        args: DesignPipelinePromptArgs,
        ctx: &RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        let (_role_name, perms) = extract_auth(ctx, &self.app_state)?;

        let mut definitions = self
            .app_state
            .engine
            .registry
            .read()
            .map_err(|e| {
                McpError::internal_error(format!("Failed to read node registry: {e}"), None)
            })?
            .definitions();

        definitions.extend(crate::server::synthetic_node_definitions());

        definitions.retain(|def| {
            if !perms.is_node_allowed(&def.kind) {
                return false;
            }
            if def.kind.starts_with("plugin::") {
                return perms.is_plugin_allowed(&def.kind);
            }
            true
        });

        // Group definitions by first category (or "uncategorized").
        let mut by_category: std::collections::BTreeMap<
            String,
            Vec<&streamkit_core::NodeDefinition>,
        > = std::collections::BTreeMap::new();
        for def in &definitions {
            let cat =
                def.categories.first().cloned().unwrap_or_else(|| "uncategorized".to_string());
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
            content.push_str(&format!("### {category}\n\n"));
            for def in defs {
                content.push_str(&format!("- **`{}`**", def.kind));
                if let Some(desc) = &def.description {
                    content.push_str(&format!(" — {desc}"));
                }
                content.push('\n');

                if !def.inputs.is_empty() {
                    let pins: Vec<String> = def
                        .inputs
                        .iter()
                        .map(|p| format!("`{}` ({:?})", p.name, p.accepts_types))
                        .collect();
                    content.push_str(&format!("  - Inputs: {}\n", pins.join(", ")));
                }
                if !def.outputs.is_empty() {
                    let pins: Vec<String> = def
                        .outputs
                        .iter()
                        .map(|p| format!("`{}` ({:?})", p.name, p.produces_type))
                        .collect();
                    content.push_str(&format!("  - Outputs: {}\n", pins.join(", ")));
                }
                // Param schema summary (skip trivially empty schemas)
                if def.param_schema != serde_json::json!({})
                    && def.param_schema != serde_json::json!(null)
                {
                    if let Some(props) = def.param_schema.get("properties") {
                        if let Some(obj) = props.as_object() {
                            if !obj.is_empty() {
                                let keys: Vec<&String> = obj.keys().collect();
                                content.push_str(&format!(
                                    "  - Params: {}\n",
                                    keys.iter()
                                        .map(|k| format!("`{k}`"))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ));
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
        if let Some(desc) = &args.description {
            content.push_str(&format!("\n## User Request\n\n{desc}\n"));
        }

        Ok(GetPromptResult::new(vec![PromptMessage::new_text(PromptMessageRole::User, content)])
            .with_description("Design a StreamKit pipeline"))
    }

    /// Build the `debug_pipeline` prompt content.
    async fn build_debug_pipeline_prompt(
        &self,
        args: DebugPipelinePromptArgs,
        ctx: &RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        let (role_name, perms) = extract_auth(ctx, &self.app_state)?;

        if !perms.list_sessions {
            return Err(McpError::invalid_request("Permission denied: cannot view sessions", None));
        }

        let session = {
            let sm = self.app_state.session_manager.lock().await;
            sm.get_session_by_name_or_id(&args.session_id)
        };

        let Some(session) = session else {
            return Err(McpError::invalid_params(
                format!("Session '{}' not found", args.session_id),
                None,
            ));
        };

        if !perms.access_all_sessions
            && session.created_by.as_ref().is_some_and(|c| c != &role_name)
        {
            return Err(McpError::invalid_request(
                "Permission denied: you do not own this session",
                None,
            ));
        }

        let node_states = session.get_node_states().await.unwrap_or_default();
        let node_view_data = session.get_node_view_data().await.unwrap_or_default();
        let runtime_schemas = session.get_runtime_schemas().await.unwrap_or_default();

        let mut api_pipeline = {
            let pipeline = session.pipeline.lock().await;
            pipeline.clone()
        };
        for (id, node) in &mut api_pipeline.nodes {
            node.state = node_states.get(id).cloned();
        }
        if !node_view_data.is_empty() {
            api_pipeline.view_data = Some(Arc::unwrap_or_clone(node_view_data));
        }
        if !runtime_schemas.is_empty() {
            api_pipeline.runtime_schemas = Some(runtime_schemas);
        }

        let pipeline_json = serde_json::to_string_pretty(&api_pipeline)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?;

        let mut content = String::with_capacity(4096);

        content
            .push_str(&format!("You are debugging StreamKit session `{}`.\n\n", args.session_id));

        // Current pipeline state
        content.push_str("## Current Pipeline State\n\n");
        content.push_str("```json\n");
        content.push_str(&pipeline_json);
        content.push_str("\n```\n\n");

        // Per-node state summary
        content.push_str("## Node States\n\n");
        let mut has_errors = false;
        for (id, node) in &api_pipeline.nodes {
            let state_str = node
                .state
                .as_ref()
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "unknown".to_string());
            content.push_str(&format!("- **`{id}`** (`{}`): {state_str}", node.kind));
            if let Some(ref state) = node.state {
                match state {
                    streamkit_core::NodeState::Failed { reason } => {
                        has_errors = true;
                        content.push_str(&format!(" — error: {reason}"));
                    },
                    streamkit_core::NodeState::Recovering { reason, .. } => {
                        content.push_str(&format!(" — recovering: {reason}"));
                    },
                    streamkit_core::NodeState::Degraded { reason, .. } => {
                        content.push_str(&format!(" — degraded: {reason}"));
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
                content.push_str(&format!(
                    "- `{}`.`{}` → `{}`.`{}` ({})\n",
                    conn.from_node,
                    conn.from_pin,
                    conn.to_node,
                    conn.to_pin,
                    match conn.mode {
                        streamkit_core::control::ConnectionMode::Reliable => "reliable",
                        streamkit_core::control::ConnectionMode::BestEffort => "best_effort",
                    },
                ));
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
            "- `validate_pipeline` — re-validate the pipeline YAML to catch \
             structural issues.\n",
        );
        content.push_str(
            "- `get_pipeline` — fetch the latest pipeline state (node states may \
             change over time).\n",
        );
        content.push_str("- `destroy_session` — tear down the session if it is unrecoverable.\n");

        info!(session_id = %args.session_id, "MCP debug_pipeline prompt");

        Ok(GetPromptResult::new(vec![PromptMessage::new_text(PromptMessageRole::User, content)])
            .with_description(format!("Debug StreamKit session '{}'", args.session_id)))
    }
}

impl ServerHandler for StreamKitMcp {
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder().enable_tools().enable_prompts().build();
        let mut info = ServerInfo::new(capabilities).with_instructions(
            "StreamKit MCP server. Use list_nodes to discover available \
             processing nodes, validate_pipeline to check YAML, \
             create_session / list_sessions / get_pipeline / destroy_session \
             to manage dynamic pipeline sessions, and \
             generate_oneshot_command to get a curl or skit-cli command for \
             batch processing via the HTTP data plane. Two built-in prompts \
             are available: design_pipeline (guided pipeline creation) and \
             debug_pipeline (session diagnostics).",
        );
        info.server_info = rmcp::model::Implementation::new("streamkit", env!("CARGO_PKG_VERSION"));
        info
    }

    fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(self.tool_router.list_all())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let ctx = ToolCallContext::new(self, request, context);
        self.tool_router.call(ctx).await
    }

    fn list_prompts(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListPromptsResult::with_all_items(Self::prompt_metadata())))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        match request.name.as_str() {
            "design_pipeline" => {
                let args: DesignPipelinePromptArgs = match &request.arguments {
                    Some(map) => serde_json::from_value(serde_json::Value::Object(map.clone()))
                        .map_err(|e| {
                            McpError::invalid_params(
                                format!("Invalid design_pipeline arguments: {e}"),
                                None,
                            )
                        })?,
                    None => DesignPipelinePromptArgs { description: None },
                };
                self.build_design_pipeline_prompt(args, &context).await
            },
            "debug_pipeline" => {
                let args: DebugPipelinePromptArgs = match &request.arguments {
                    Some(map) => serde_json::from_value(serde_json::Value::Object(map.clone()))
                        .map_err(|e| {
                            McpError::invalid_params(
                                format!("Invalid debug_pipeline arguments: {e}"),
                                None,
                            )
                        })?,
                    None => {
                        return Err(McpError::invalid_params(
                            "debug_pipeline requires a 'session_id' argument",
                            None,
                        ));
                    },
                };
                self.build_debug_pipeline_prompt(args, &context).await
            },
            other => Err(McpError::invalid_params(format!("Unknown prompt: '{other}'"), None)),
        }
    }
}

// ---------------------------------------------------------------------------
// Service factory
// ---------------------------------------------------------------------------

/// Create the `StreamableHttpService` tower service for mounting in the Axum
/// router via `nest_service`.
///
/// ## `StreamableHttpServerConfig` defaults (rmcp 1.5)
///
/// | Field              | Default                              |
/// |--------------------|--------------------------------------|
/// | `sse_keep_alive`   | 15 s                                 |
/// | `sse_retry`        | 3 s                                  |
/// | `stateful_mode`    | true                                 |
/// | `json_response`    | false                                |
/// | `allowed_hosts`    | localhost, 127.0.0.1, ::1            |
///
/// ## `SessionConfig` defaults (rmcp 1.5)
///
/// | Field                  | Default  |
/// |------------------------|----------|
/// | `channel_capacity`     | 16       |
/// | `keep_alive`           | 5 min    |
/// | `sse_retry`            | 3 s      |
/// | `completed_cache_ttl`  | 60 s     |
///
/// The 5-minute `keep_alive` TTL automatically evicts idle MCP sessions,
/// preventing unbounded growth from dropped connections.  All sessions
/// require authentication, which further bounds creation rate.
///
/// `allowed_hosts` is configured from `mcp.allowed_hosts` in the config.
/// When the list is empty (default), `allowed_hosts` is disabled — acceptable
/// when the endpoint sits behind Axum's `auth_guard_middleware` and
/// `origin_guard_middleware`.  For deployments exposed to untrusted
/// networks, populate `mcp.allowed_hosts` to re-enable DNS rebinding
/// protection on the `Host` header.
pub fn streamable_http_service(
    app_state: Arc<AppState>,
) -> StreamableHttpService<StreamKitMcp, LocalSessionManager> {
    let mut config = StreamableHttpServerConfig::default();
    if app_state.config.mcp.allowed_hosts.is_empty() {
        config = config.disable_allowed_hosts();
    } else {
        config = config.with_allowed_hosts(app_state.config.mcp.allowed_hosts.clone());
    }
    StreamableHttpService::new(
        move || Ok(StreamKitMcp::new(Arc::clone(&app_state))),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}
