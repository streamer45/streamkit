// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Embedded MCP (Model Context Protocol) server for StreamKit.
//!
//! Exposes StreamKit control-plane capabilities (node discovery, pipeline
//! validation, session management) as MCP tools over Streamable HTTP or
//! STDIO.  The endpoint reuses the existing Axum application state, auth,
//! and permission model — no separate bridge process required.
//!
//! # Security — STDIO transport
//!
//! `skit mcp` runs unauthenticated: the STDIO caller is implicitly trusted
//! with admin-level permissions (see [`extract_auth`]).  Only expose its
//! stdin to trusted local processes (e.g. Devin, Claude Desktop, Cursor).

mod oneshot;
mod prompts;

use std::sync::Arc;

use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    Annotated, CallToolResult, Content, GetPromptRequestParams, GetPromptResult, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams, RawResource,
    RawResourceTemplate, ReadResourceRequestParams, ReadResourceResult, ResourceContents,
    ServerCapabilities, ServerInfo,
};
use rmcp::schemars;
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{prompt_handler, tool, tool_handler, tool_router};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};

use serde::{Deserialize, Serialize};
use streamkit_api::Pipeline;
use streamkit_core::NodeDefinition;
use tracing::{info, warn};

use crate::permissions::Permissions;
use crate::session::Session;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Auth helper
// ---------------------------------------------------------------------------

/// Extract `(role_name, permissions)` from the HTTP request parts that `rmcp`
/// injects into the request-context extensions.
///
/// For STDIO transport there are no HTTP parts in the context — the caller is
/// a local, trusted process.  In that case we fall back to admin-level
/// permissions (resolved via the configured `default_role`, which defaults to
/// `"admin"`).
#[allow(clippy::unnecessary_wraps)]
fn extract_auth(
    ctx: &RequestContext<RoleServer>,
    app_state: &Arc<AppState>,
) -> Result<(String, Permissions), McpError> {
    Ok(ctx.extensions.get::<http::request::Parts>().map_or_else(
        || {
            // STDIO transport — no HTTP context.  Treat as local/trusted.
            let empty_headers = axum::http::HeaderMap::new();
            crate::role_extractor::get_role_and_permissions(&empty_headers, app_state)
        },
        |parts| crate::role_extractor::get_role_and_permissions(&parts.headers, app_state),
    ))
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Look up a session by name or ID, verify permission, and return the session
/// along with the caller's role name and permissions.
async fn resolve_session(
    app_state: &Arc<AppState>,
    session_id: &str,
    ctx: &RequestContext<RoleServer>,
    check_perm: impl FnOnce(&Permissions) -> bool,
    perm_label: &str,
) -> Result<(Session, String, Permissions), McpError> {
    let (role_name, perms) = extract_auth(ctx, app_state)?;

    if !check_perm(&perms) {
        return Err(McpError::invalid_request(
            format!("Permission denied: {perm_label} required"),
            None,
        ));
    }

    let session = {
        let sm = app_state.session_manager.lock().await;
        sm.get_session_by_name_or_id(session_id)
    };

    let Some(session) = session else {
        return Err(McpError::invalid_params(format!("Session '{session_id}' not found"), None));
    };

    if !perms.access_all_sessions && session.created_by.as_ref().is_some_and(|c| c != &role_name) {
        return Err(McpError::invalid_request(
            "Permission denied: you do not own this session",
            None,
        ));
    }

    Ok((session, role_name, perms))
}

/// Serialize a value to pretty-printed JSON and wrap it in a successful
/// `CallToolResult`.
fn json_tool_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

/// Return permission-filtered node definitions, including synthetic oneshot
/// nodes.
fn filtered_node_definitions(
    app_state: &Arc<AppState>,
    perms: &Permissions,
) -> Result<Vec<NodeDefinition>, McpError> {
    let mut definitions = app_state
        .engine
        .registry
        .read()
        .map_err(|e| McpError::internal_error(format!("Failed to read node registry: {e}"), None))?
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

    Ok(definitions)
}

/// Assemble the full pipeline state for a session, merging node states, view
/// data, and runtime schemas into the cloned pipeline.
async fn assemble_pipeline_state(session: &Session) -> Pipeline {
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

    api_pipeline
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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ValidateBatchArgs {
    /// Session ID or name.
    pub session_id: String,
    /// List of batch operations to validate.
    pub operations: Vec<streamkit_api::BatchOperation>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ApplyBatchArgs {
    /// Session ID or name.
    pub session_id: String,
    /// List of batch operations to apply atomically.
    pub operations: Vec<streamkit_api::BatchOperation>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetLogsArgs {
    /// Maximum number of log lines to return (default: 100, max: 500).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Log level filter: "debug", "info", "warn", "error" (default: all levels).
    #[serde(default)]
    pub level: Option<String>,
    /// Optional text filter to search within log messages.
    #[serde(default)]
    pub filter: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListSamplesArgs {
    /// Filter by mode: "oneshot", "dynamic", or omit for all.
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TuneNodeArgs {
    /// Session ID or name.
    pub session_id: String,
    /// Node ID to send the control message to.
    pub node_id: String,
    /// The control message (e.g., UpdateParams with a JSON value).
    pub message: streamkit_core::control::NodeControlMessage,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetNodeDefinitionArgs {
    /// Node kind to look up (e.g., "audio::gain", "core::passthrough").
    pub kind: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdatePipelineArgs {
    /// Session ID or name.
    pub session_id: String,
    /// New pipeline YAML defining the desired state. The tool will diff
    /// this against the current pipeline and apply the minimal set of
    /// batch operations (addnode, removenode, connect, disconnect) to
    /// reconcile the running session.
    pub yaml: String,
}

// ---------------------------------------------------------------------------
// StreamKit MCP service
// ---------------------------------------------------------------------------

/// StreamKit MCP service implementing `rmcp::ServerHandler`.
#[derive(Clone)]
pub struct StreamKitMcp {
    app_state: Arc<AppState>,
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
}

#[tool_router]
impl StreamKitMcp {
    pub fn new(app_state: Arc<AppState>) -> Self {
        Self {
            app_state,
            tool_router: Self::tool_router(),
            prompt_router: prompts::create_prompt_router(),
        }
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

        let definitions = filtered_node_definitions(&self.app_state, &perms)?;

        info!(count = definitions.len(), "MCP list_nodes");

        json_tool_result(&definitions)
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

        json_tool_result(&response)
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

        let r = crate::server::create_dynamic_session(
            &self.app_state,
            &args.yaml,
            args.name,
            role_name,
            &perms,
        )
        .await
        .map_err(|e| match e {
            crate::server::CreateSessionError::InvalidInput(msg) => {
                McpError::invalid_params(msg, None)
            },
            crate::server::CreateSessionError::Forbidden(msg)
            | crate::server::CreateSessionError::Conflict(msg)
            | crate::server::CreateSessionError::LimitReached(msg) => {
                McpError::invalid_request(msg, None)
            },
            crate::server::CreateSessionError::Internal(msg) => McpError::internal_error(msg, None),
        })?;

        let result = serde_json::json!({
            "session_id": r.session_id,
            "name": r.name,
            "created_at": r.created_at,
        });
        json_tool_result(&result)
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

        json_tool_result(&infos)
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
        let (session, _role_name, _perms) = resolve_session(
            &self.app_state,
            &args.session_id,
            &ctx,
            |p| p.list_sessions,
            "list_sessions",
        )
        .await?;

        let api_pipeline = assemble_pipeline_state(&session).await;

        info!(session_id = %args.session_id, "MCP get_pipeline");

        json_tool_result(&api_pipeline)
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

        if !validation.valid {
            let pretty = serde_json::to_string_pretty(&validation)
                .map_err(|e| McpError::internal_error(format!("serialization error: {e}"), None))?;
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "Pipeline validation failed. Fix the errors before generating a command:\n{pretty}"
            ))]));
        }

        let server_url = args.server_url.as_deref().unwrap_or("http://localhost:4545");
        let format = args.format.as_deref().unwrap_or("curl");

        let command = match format {
            "curl" => {
                oneshot::generate_curl_command(&args.yaml, &args.inputs, &args.output, server_url)
            },
            "skit-cli" => oneshot::generate_skit_cli_command(
                &args.yaml,
                &args.inputs,
                &args.output,
                server_url,
            ),
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
        json_tool_result(&result)
    }

    // -- validate_batch ----------------------------------------------------

    #[tool(
        description = "Validate a batch of graph mutations against a running session without applying them. Returns validation errors for any operations that would fail. Operations: addnode, removenode, connect, disconnect."
    )]
    async fn validate_batch(
        &self,
        Parameters(args): Parameters<ValidateBatchArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (session, _role_name, perms) = resolve_session(
            &self.app_state,
            &args.session_id,
            &ctx,
            |p| p.modify_sessions,
            "modify_sessions",
        )
        .await?;

        let errors = crate::server::validate_batch_operations(
            &session,
            &args.operations,
            &perms,
            &self.app_state.config.security,
        )
        .await;

        info!(
            session_id = %args.session_id,
            operation_count = args.operations.len(),
            error_count = errors.len(),
            "MCP validate_batch"
        );

        json_tool_result(&errors)
    }

    // -- apply_batch -------------------------------------------------------

    #[tool(
        description = "Apply a batch of graph mutations to a running session as a single validated batch. All operations are validated before any are applied; if validation fails, none are applied. Note: engine-side errors after validation do not roll back already-applied operations. Operations: addnode, removenode, connect, disconnect."
    )]
    async fn apply_batch(
        &self,
        Parameters(args): Parameters<ApplyBatchArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (session, _role_name, perms) = resolve_session(
            &self.app_state,
            &args.session_id,
            &ctx,
            |p| p.modify_sessions,
            "modify_sessions",
        )
        .await?;

        crate::server::apply_batch_operations(
            &session,
            args.operations,
            &perms,
            &self.app_state.config.security,
        )
        .await
        .map_err(|e| McpError::invalid_params(e, None))?;

        info!(session_id = %args.session_id, "MCP apply_batch");

        let result = serde_json::json!({ "success": true });
        json_tool_result(&result)
    }

    // -- get_logs ----------------------------------------------------------

    #[tool(
        description = "Retrieve recent server logs. Requires admin-level permissions (access_all_sessions). Returns the most recent log lines, optionally filtered by level and text. Useful for diagnosing pipeline and node errors that aren't visible in node states alone."
    )]
    async fn get_logs(
        &self,
        Parameters(args): Parameters<GetLogsArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        const DEFAULT_LIMIT: usize = 100;
        const MAX_LIMIT: usize = 500;

        let (_role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        if !perms.access_all_sessions {
            return Err(McpError::invalid_request(
                "Permission denied: access_all_sessions required",
                None,
            ));
        }

        if !self.app_state.config.log.file_enable {
            return Err(McpError::invalid_request(
                "File logging is not enabled. Set log.file_enable = true in server config.",
                None,
            ));
        }

        let log_path = crate::log_viewer::resolve_log_path(&self.app_state.config.log.file_path)
            .map_err(|_| McpError::internal_error("Failed to resolve log file path", None))?;

        if !log_path.exists() {
            return Err(McpError::internal_error(
                "Log file does not exist yet — the server may not have written any logs",
                None,
            ));
        }

        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);

        let file = tokio::fs::File::open(&log_path)
            .await
            .map_err(|e| McpError::internal_error(format!("Failed to open log file: {e}"), None))?;

        let metadata = file.metadata().await.map_err(|e| {
            McpError::internal_error(format!("Failed to read log file metadata: {e}"), None)
        })?;
        let file_size = metadata.len();

        let response = crate::log_viewer::read_backward(
            file,
            file_size,
            None,
            limit,
            args.level.as_deref(),
            args.filter.as_deref(),
        )
        .await
        .map_err(|status| {
            McpError::internal_error(format!("Failed to read log file (HTTP {status})"), None)
        })?;

        info!(lines = response.lines.len(), "MCP get_logs");

        let text = response.lines.join("\n");
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    // -- list_samples ------------------------------------------------------

    #[tool(
        description = "List available sample/template pipelines with their names, descriptions, and modes. Use these as starting points when designing new pipelines."
    )]
    async fn list_samples(
        &self,
        Parameters(args): Parameters<ListSamplesArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (_role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        if !perms.list_samples {
            return Err(McpError::invalid_request(
                "Permission denied: list_samples required",
                None,
            ));
        }

        let samples = crate::samples::list_samples(&self.app_state, &perms)
            .await
            .map_err(|e| McpError::internal_error(format!("failed to list samples: {e}"), None))?;

        let filtered: Vec<&streamkit_api::SamplePipeline> = match args.mode.as_deref() {
            Some("oneshot") => samples.iter().filter(|s| s.mode == "oneshot").collect(),
            Some("dynamic") => samples.iter().filter(|s| s.mode == "dynamic").collect(),
            None => samples.iter().collect(),
            Some(other) => {
                return Err(McpError::invalid_params(
                    format!("Invalid mode '{other}'. Must be 'oneshot' or 'dynamic'."),
                    None,
                ));
            },
        };

        info!(count = filtered.len(), mode = ?args.mode, "MCP list_samples");

        json_tool_result(&filtered)
    }

    // -- get_server_info ---------------------------------------------------

    #[tool(
        description = "Get StreamKit server configuration: enabled features (MoQ, marketplace, oneshot), limits, and version information. Useful for understanding the capabilities of the instance you're connected to."
    )]
    async fn get_server_info(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (role_name, _perms) = extract_auth(&ctx, &self.app_state)?;

        if role_name == "viewer" {
            return Err(McpError::invalid_request(
                "Permission denied: viewers cannot access server info",
                None,
            ));
        }

        let config = &self.app_state.config;
        let plugin_count = self.app_state.plugin_manager.lock().await.list_plugins().len();

        let info = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "features": {
                "moq": cfg!(feature = "moq"),
                "mcp": config.mcp.enabled,
                "oneshot": true,
                "marketplace": config.plugins.marketplace.marketplace_enabled,
            },
            "limits": {
                "max_body_size": config.server.max_body_size,
            },
            "plugins": {
                "count": plugin_count,
            },
            "auth": {
                "enabled": self.app_state.auth.is_enabled(),
            },
        });

        tracing::info!("MCP get_server_info");

        json_tool_result(&info)
    }

    // -- tune_node ---------------------------------------------------------

    #[tool(
        description = "Send a control message to a specific node in a running session. Commonly used with UpdateParams to modify node parameters at runtime."
    )]
    async fn tune_node(
        &self,
        Parameters(args): Parameters<TuneNodeArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (session, _role_name, _perms) = resolve_session(
            &self.app_state,
            &args.session_id,
            &ctx,
            |p| p.tune_nodes,
            "tune_nodes",
        )
        .await?;

        crate::server::tune_session_node(
            &session,
            args.node_id.clone(),
            args.message,
            &self.app_state.config.security,
            &self.app_state.event_tx,
        )
        .await
        .map_err(|e| McpError::invalid_params(e, None))?;

        info!(session_id = %args.session_id, node_id = %args.node_id, "MCP tune_node");

        let result = serde_json::json!({ "success": true });
        json_tool_result(&result)
    }

    // -- list_plugins ------------------------------------------------------

    #[tool(
        description = "List installed StreamKit plugins with their kind, version, type (native/wasm), and categories."
    )]
    async fn list_plugins(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (_role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        let mut plugins = self.app_state.plugin_manager.lock().await.list_plugins();
        plugins.retain(|plugin| perms.is_plugin_allowed(&plugin.kind));

        info!(count = plugins.len(), "MCP list_plugins");

        json_tool_result(&plugins)
    }

    // -- get_node_definition -----------------------------------------------

    #[tool(
        description = "Get the full definition (schema, pins, categories) for a specific node kind. Use this when you need the param schema or pin layout for a particular node."
    )]
    async fn get_node_definition(
        &self,
        Parameters(args): Parameters<GetNodeDefinitionArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (_role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        if !perms.is_node_allowed(&args.kind) {
            return Err(McpError::invalid_request(
                format!("Permission denied: node kind '{}' is not allowed", args.kind),
                None,
            ));
        }

        if args.kind.starts_with("plugin::") && !perms.is_plugin_allowed(&args.kind) {
            return Err(McpError::invalid_request(
                format!("Permission denied: plugin '{}' is not allowed", args.kind),
                None,
            ));
        }

        let definitions = filtered_node_definitions(&self.app_state, &perms)?;
        let definition = definitions.into_iter().find(|d| d.kind == args.kind);

        let Some(definition) = definition else {
            return Err(McpError::invalid_params(
                format!("Node kind '{}' not found", args.kind),
                None,
            ));
        };

        info!(kind = %args.kind, "MCP get_node_definition");

        json_tool_result(&definition)
    }

    // -- update_pipeline ---------------------------------------------------

    #[tool(
        description = "Update a running session's pipeline to match new YAML. Diffs the desired state against the current pipeline and applies the minimal set of batch operations (addnode, removenode, connect, disconnect). Parameter changes on surviving nodes are applied automatically via tune_node when the caller has tune_nodes permission; otherwise they are returned in params_changed for manual follow-up. Note: the pipeline is snapshotted before diffing and the lock is released during diff computation — concurrent mutations may cause 'node already exists' errors; retry in that case. Engine-side errors after the durable Pipeline mutation do not roll back already-applied operations."
    )]
    async fn update_pipeline(
        &self,
        Parameters(args): Parameters<UpdatePipelineArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (session, _role_name, perms) = resolve_session(
            &self.app_state,
            &args.session_id,
            &ctx,
            |p| p.modify_sessions,
            "modify_sessions",
        )
        .await?;

        // Parse and compile the desired YAML.
        let user_pipeline = streamkit_api::yaml::parse_yaml(&args.yaml)
            .map_err(|e| McpError::invalid_params(format!("Invalid pipeline YAML: {e}"), None))?;

        let desired = streamkit_api::yaml::compile(user_pipeline).map_err(|e| {
            McpError::invalid_params(format!("Pipeline compilation error: {e}"), None)
        })?;

        // Snapshot the current pipeline state.
        let current = { session.pipeline.lock().await.clone() };

        // Compute diff → batch operations + param changes.
        let diff = diff_pipeline(&current, &desired);

        if diff.operations.is_empty() && diff.params_changed.is_empty() {
            info!(session_id = %args.session_id, "MCP update_pipeline (no changes)");
            let result = serde_json::json!({
                "operations_applied": 0,
                "operations": [],
                "params_changed": [],
            });
            return json_tool_result(&result);
        }

        // Apply structural changes via the shared batch path (validates + applies).
        if !diff.operations.is_empty() {
            crate::server::apply_batch_operations(
                &session,
                diff.operations.clone(),
                &perms,
                &self.app_state.config.security,
            )
            .await
            .map_err(|e| McpError::invalid_params(e, None))?;
        }

        // Apply param changes via tune_node when permitted.
        let mut params_applied = Vec::new();
        let mut params_deferred = Vec::new();
        for (node_id, new_params) in &diff.params_changed {
            if perms.tune_nodes {
                crate::server::tune_session_node(
                    &session,
                    node_id.clone(),
                    streamkit_core::control::NodeControlMessage::UpdateParams(new_params.clone()),
                    &self.app_state.config.security,
                    &self.app_state.event_tx,
                )
                .await
                .map_err(|e| McpError::invalid_params(e, None))?;
                params_applied.push(node_id.clone());
            } else {
                params_deferred.push(node_id.clone());
            }
        }

        info!(
            session_id = %args.session_id,
            ops = diff.operations.len(),
            params_tuned = params_applied.len(),
            params_deferred = params_deferred.len(),
            "MCP update_pipeline"
        );

        let mut result = serde_json::json!({
            "operations_applied": diff.operations.len(),
            "operations": diff.operations,
            "params_changed": diff.params_changed.iter()
                .map(|(id, v)| serde_json::json!({ "node_id": id, "params": v }))
                .collect::<Vec<_>>(),
        });
        if !params_applied.is_empty() {
            result["params_applied"] = serde_json::json!(params_applied);
        }
        if !params_deferred.is_empty() {
            result["params_deferred"] = serde_json::json!(params_deferred);
            result["params_deferred_reason"] =
                serde_json::json!("caller lacks tune_nodes permission; use tune_node manually");
        }
        json_tool_result(&result)
    }
}

// ---------------------------------------------------------------------------
// Pipeline diffing
// ---------------------------------------------------------------------------

/// Result of diffing two pipelines.
struct DiffResult {
    /// Structural batch operations (disconnect, remove, add, connect).
    operations: Vec<streamkit_api::BatchOperation>,
    /// Nodes whose params changed (node_id, desired_params). Only includes
    /// surviving nodes whose kind did NOT change (replaced nodes get their
    /// params via the AddNode operation).
    params_changed: Vec<(String, serde_json::Value)>,
}

/// Compute the minimal set of `BatchOperation`s to reconcile `current` into
/// `desired`.  Ordering: disconnects → removes → adds → connects.
///
/// Also detects param-only changes on surviving nodes and returns them
/// separately so the caller can apply them via `tune_node`.
fn diff_pipeline(current: &Pipeline, desired: &Pipeline) -> DiffResult {
    use std::collections::HashSet;
    use streamkit_api::BatchOperation;
    use streamkit_core::control::ConnectionMode;

    // Connection identity includes mode so that Reliable↔BestEffort flips
    // are detected as a disconnect+reconnect.
    type ConnKey = (String, String, String, String, ConnectionMode);
    fn conn_key(c: &streamkit_api::Connection) -> ConnKey {
        (c.from_node.clone(), c.from_pin.clone(), c.to_node.clone(), c.to_pin.clone(), c.mode)
    }

    let mut ops: Vec<BatchOperation> = Vec::new();

    let current_node_ids: HashSet<&str> = current.nodes.keys().map(String::as_str).collect();
    let desired_node_ids: HashSet<&str> = desired.nodes.keys().map(String::as_str).collect();

    // Nodes whose kind changed need to be replaced (remove + re-add).
    let replaced_node_ids: HashSet<&str> = current_node_ids
        .intersection(&desired_node_ids)
        .filter(|id| {
            let cur = &current.nodes[**id];
            let des = &desired.nodes[**id];
            cur.kind != des.kind
        })
        .copied()
        .collect();

    let current_conns: HashSet<ConnKey> = current.connections.iter().map(conn_key).collect();
    let desired_conns: HashSet<ConnKey> = desired.connections.iter().map(conn_key).collect();

    // 1. Disconnect removed/changed connections.
    //    We skip connections where either endpoint is being fully removed
    //    (the engine tears those down with RemoveNode). Connections on
    //    replaced nodes are explicitly disconnected because the node is
    //    re-added as a new instance. HashSet<ConnKey> collapses true
    //    parallel edges (same endpoints + same mode) into one entry;
    //    this is acceptable because the engine also deduplicates them.
    for conn in &current.connections {
        let key = conn_key(conn);
        let from_replaced = replaced_node_ids.contains(conn.from_node.as_str());
        let to_replaced = replaced_node_ids.contains(conn.to_node.as_str());
        let from_survives = desired_node_ids.contains(conn.from_node.as_str());
        let to_survives = desired_node_ids.contains(conn.to_node.as_str());

        if (!desired_conns.contains(&key) || from_replaced || to_replaced)
            && from_survives
            && to_survives
        {
            ops.push(BatchOperation::Disconnect {
                from_node: conn.from_node.clone(),
                from_pin: conn.from_pin.clone(),
                to_node: conn.to_node.clone(),
                to_pin: conn.to_pin.clone(),
            });
        }
    }

    // 2. Remove nodes that no longer exist or whose kind changed.
    //    Iterating current.nodes (IndexMap) gives deterministic ordering.
    for node_id in current.nodes.keys() {
        if !desired_node_ids.contains(node_id.as_str())
            || replaced_node_ids.contains(node_id.as_str())
        {
            ops.push(BatchOperation::RemoveNode { node_id: node_id.clone() });
        }
    }

    // 3. Add new nodes and re-add replaced nodes with new kind.
    for (node_id, node) in &desired.nodes {
        if !current_node_ids.contains(node_id.as_str())
            || replaced_node_ids.contains(node_id.as_str())
        {
            ops.push(BatchOperation::AddNode {
                node_id: node_id.clone(),
                kind: node.kind.clone(),
                params: node.params.clone(),
            });
        }
    }

    // 4. Connect new connections and re-connect connections touching replaced nodes.
    for conn in &desired.connections {
        let touches_replaced = replaced_node_ids.contains(conn.from_node.as_str())
            || replaced_node_ids.contains(conn.to_node.as_str());
        if !current_conns.contains(&conn_key(conn)) || touches_replaced {
            ops.push(BatchOperation::Connect {
                from_node: conn.from_node.clone(),
                from_pin: conn.from_pin.clone(),
                to_node: conn.to_node.clone(),
                to_pin: conn.to_pin.clone(),
                mode: conn.mode,
            });
        }
    }

    // 5. Detect param-only changes on surviving, non-replaced nodes.
    let mut params_changed = Vec::new();
    for (node_id, desired_node) in &desired.nodes {
        if replaced_node_ids.contains(node_id.as_str())
            || !current_node_ids.contains(node_id.as_str())
        {
            continue;
        }
        let current_node = &current.nodes[node_id.as_str()];
        if desired_node.params != current_node.params {
            if let Some(ref p) = desired_node.params {
                params_changed.push((node_id.clone(), p.clone()));
            }
        }
    }

    DiffResult { operations: ops, params_changed }
}

// ---------------------------------------------------------------------------
// ServerHandler trait impl
// ---------------------------------------------------------------------------

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for StreamKitMcp {
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_prompts()
            .enable_resources()
            .build();
        let mut info = ServerInfo::new(capabilities).with_instructions(
            "StreamKit MCP server. Use list_nodes to discover available \
             processing nodes, get_node_definition to look up a specific \
             node's schema/pins/categories, list_plugins to see installed \
             plugins, validate_pipeline to check YAML, and \
             create_session / list_sessions / get_pipeline / destroy_session \
             to manage dynamic pipeline sessions. Use validate_batch and \
             apply_batch to mutate a running session's graph as a validated batch, \
             tune_node to send control messages, \
             update_pipeline to apply a YAML diff to a running session, \
             generate_oneshot_command to get a command for batch processing, \
             get_logs to retrieve recent server logs for debugging, and \
             resources/list to browse sample pipeline templates. \
             Use list_samples to browse sample/template pipelines as starting \
             points, and get_server_info to inspect server capabilities, \
             enabled features, and limits.",
        );
        info.server_info = rmcp::model::Implementation::new("streamkit", env!("CARGO_PKG_VERSION"));
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let (_role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        if !perms.list_samples {
            return Err(McpError::invalid_request(
                "Permission denied: list_samples required",
                None,
            ));
        }

        let samples = crate::samples::list_samples(&self.app_state, &perms)
            .await
            .map_err(|e| McpError::internal_error(format!("failed to list samples: {e}"), None))?;

        let resources: Vec<_> = samples
            .into_iter()
            .map(|s| {
                let uri = format!("streamkit://samples/{}", s.id);
                let raw = RawResource::new(&uri, &s.name)
                    .with_description(&s.description)
                    .with_mime_type("application/x-yaml");
                Annotated::new(raw, None)
            })
            .collect();

        info!(count = resources.len(), "MCP list_resources");

        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let (_role_name, perms) = extract_auth(&ctx, &self.app_state)?;

        if !perms.read_samples {
            return Err(McpError::invalid_request(
                "Permission denied: read_samples required",
                None,
            ));
        }

        let uri = &request.uri;
        let path = uri.strip_prefix("streamkit://samples/").ok_or_else(|| {
            McpError::invalid_params(
                format!(
                    "Invalid resource URI '{uri}': expected streamkit://samples/{{mode}}/{{id}}"
                ),
                None,
            )
        })?;

        let (mode, id) = path.split_once('/').ok_or_else(|| {
            McpError::invalid_params(
                format!(
                    "Malformed resource URI '{uri}': expected streamkit://samples/{{mode}}/{{id}}"
                ),
                None,
            )
        })?;

        if !matches!(mode, "oneshot" | "dynamic" | "demo" | "user") {
            return Err(McpError::invalid_params(
                format!("Unknown sample mode '{mode}' in URI '{uri}'"),
                None,
            ));
        }

        if id.contains("..") || id.contains('/') || id.contains('\\') {
            return Err(McpError::invalid_params(
                format!("Invalid resource ID in URI '{uri}'"),
                None,
            ));
        }

        let sample_id = format!("{mode}/{id}");
        let sample =
            crate::samples::get_sample(&self.app_state, &sample_id, &perms).await.map_err(|e| {
                warn!(uri = %uri, error = %e, "MCP read_resource failed");
                McpError::invalid_params(format!("Resource not found: {uri}"), None)
            })?;

        info!(uri = %uri, "MCP read_resource");

        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(sample.yaml, uri.clone()).with_mime_type("application/x-yaml")
        ]))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        let raw = RawResourceTemplate::new("streamkit://samples/{mode}/{id}", "Sample Pipeline")
            .with_description(
                "A curated sample pipeline template. Modes: 'oneshot' (batch processing), \
             'dynamic' (real-time streaming), 'demo', or 'user'.",
            )
            .with_mime_type("application/x-yaml");

        let template = Annotated::new(raw, None);

        Ok(ListResourceTemplatesResult::with_all_items(vec![template]))
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
/// When the list is empty (default), the `Host`-header check is disabled.
/// This is acceptable because Axum's `auth_guard_middleware` (bearer-token
/// validation) already prevents DNS rebinding exploitation — a rebound
/// request cannot supply a valid token.  `origin_guard_middleware`
/// additionally restricts browser-initiated cross-origin requests.
/// For deployments exposed to untrusted networks *without* auth enabled,
/// populate `mcp.allowed_hosts` to re-enable `Host`-header validation.
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
