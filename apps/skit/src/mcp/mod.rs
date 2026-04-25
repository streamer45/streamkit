// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Embedded MCP (Model Context Protocol) server for StreamKit.
//!
//! Exposes StreamKit control-plane capabilities (node discovery, pipeline
//! validation, session management) as MCP tools over Streamable HTTP.
//! The endpoint reuses the existing Axum application state, auth, and
//! permission model — no separate bridge process required.

mod oneshot;
mod prompts;

use std::sync::Arc;

use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, GetPromptRequestParams, GetPromptResult, ListPromptsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo,
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
use tracing::{debug, info, warn};

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
pub struct TuneNodeArgs {
    /// Session ID or name.
    pub session_id: String,
    /// Node ID to send the control message to.
    pub node_id: String,
    /// The control message (e.g., UpdateParams with a JSON value).
    pub message: streamkit_core::control::NodeControlMessage,
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
        description = "Apply a batch of graph mutations atomically to a running session. All operations succeed or all fail together. Operations: addnode, removenode, connect, disconnect."
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
}

// ---------------------------------------------------------------------------
// ServerHandler trait impl
// ---------------------------------------------------------------------------

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for StreamKitMcp {
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder().enable_tools().enable_prompts().build();
        let mut info = ServerInfo::new(capabilities).with_instructions(
            "StreamKit MCP server. Use list_nodes to discover available \
             processing nodes, validate_pipeline to check YAML, and \
             create_session / list_sessions / get_pipeline / destroy_session \
             to manage dynamic pipeline sessions. Use validate_batch and \
             apply_batch to atomically mutate a running session's graph, \
             tune_node to send control messages, and \
             generate_oneshot_command to get a command for batch processing.",
        );
        info.server_info = rmcp::model::Implementation::new("streamkit", env!("CARGO_PKG_VERSION"));
        info
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
