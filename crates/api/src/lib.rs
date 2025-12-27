// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! api: Defines the WebSocket API contract for StreamKit.
//!
//! All API communication uses JSON for parameters and payloads.
//! While pipeline YAML files are still supported internally, the WebSocket API
//! contract exclusively uses JSON for consistency and TypeScript compatibility.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// YAML pipeline format compilation
pub mod yaml;

// Re-export types so client crates can use them
pub use streamkit_core::control::{ConnectionMode, NodeControlMessage};
pub use streamkit_core::{NodeDefinition, NodeState, NodeStats};

// --- Message Types ---

/// The type of WebSocket message being sent or received.
///
/// StreamKit uses a request/response pattern with optional events:
/// - **Request**: Client sends to server with correlation_id
/// - **Response**: Server replies with matching correlation_id
/// - **Event**: Server broadcasts to all clients (no correlation_id)
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum MessageType {
    /// Client-initiated request that expects a response
    Request,
    /// Server response to a specific request (matched by correlation_id)
    Response,
    /// Server-initiated broadcast event (no correlation_id)
    Event,
}

// --- Base Message ---

/// Generic WebSocket message container for requests, responses, and events.
///
/// # Example (Request)
/// ```json
/// {
///   "type": "request",
///   "correlation_id": "abc123",
///   "payload": {
///     "action": "createsession",
///     "name": "My Session"
///   }
/// }
/// ```
///
/// # Example (Response)
/// ```json
/// {
///   "type": "response",
///   "correlation_id": "abc123",
///   "payload": {
///     "action": "sessioncreated",
///     "session_id": "sess_xyz",
///     "name": "My Session"
///   }
/// }
/// ```
///
/// # Example (Event)
/// ```json
/// {
///   "type": "event",
///   "payload": {
///     "event": "nodestatechanged",
///     "session_id": "sess_xyz",
///     "node_id": "gain1",
///     "state": { "Running": null }
///   }
/// }
/// ```
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message<T> {
    /// The type of message (Request, Response, or Event)
    #[serde(rename = "type")]
    pub message_type: MessageType,
    /// Optional correlation ID for matching requests with responses.
    /// Present in Request and Response messages, absent in Event messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// The message payload (RequestPayload, ResponsePayload, or EventPayload)
    pub payload: T,
}

// --- Client-to-Server Payloads (Requests) ---

/// Client-to-server request payload types.
///
/// All requests should include a correlation_id in the outer Message wrapper
/// to match responses.
///
/// # Session Management
/// - `CreateSession`: Create a new dynamic pipeline session
/// - `DestroySession`: Destroy an existing session
/// - `ListSessions`: List all sessions visible to the current role
///
/// # Pipeline Manipulation
/// - `AddNode`: Add a node to a session's pipeline
/// - `RemoveNode`: Remove a node from a session's pipeline
/// - `Connect`: Connect two nodes in a session's pipeline
/// - `Disconnect`: Disconnect two nodes in a session's pipeline
/// - `TuneNode`: Send control message to a node (with response)
/// - `TuneNodeAsync`: Send control message to a node (fire-and-forget)
///
/// # Batch Operations
/// - `ValidateBatch`: Validate multiple operations without applying
/// - `ApplyBatch`: Apply multiple operations atomically
///
/// # Discovery
/// - `ListNodes`: List all available node types
/// - `GetPipeline`: Get current pipeline state for a session
/// - `GetPermissions`: Get current user's permissions
#[derive(Serialize, Deserialize, Debug, TS)]
#[ts(export)]
#[serde(tag = "action")]
#[serde(rename_all = "lowercase")]
pub enum RequestPayload {
    /// Create a new dynamic pipeline session
    CreateSession {
        /// Optional session name for identification
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Destroy an existing session and clean up resources
    DestroySession {
        /// The session ID to destroy
        session_id: String,
    },
    /// List all sessions visible to the current user/role
    ListSessions,
    /// List all available node types and their schemas
    ListNodes,
    /// Add a node to a session's pipeline
    AddNode {
        /// The session ID to add the node to
        session_id: String,
        /// Unique identifier for this node instance
        node_id: String,
        /// Node type (e.g., "audio::gain", "plugin::native::whisper")
        kind: String,
        /// Optional JSON configuration parameters for the node
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(type = "JsonValue")]
        params: Option<serde_json::Value>,
    },
    /// Remove a node from a session's pipeline
    RemoveNode {
        /// The session ID containing the node
        session_id: String,
        /// The node ID to remove
        node_id: String,
    },
    /// Connect two nodes in a session's pipeline
    Connect {
        /// The session ID containing the nodes
        session_id: String,
        /// Source node ID
        from_node: String,
        /// Source output pin name
        from_pin: String,
        /// Destination node ID
        to_node: String,
        /// Destination input pin name
        to_pin: String,
        /// Connection mode (reliable or best-effort). Defaults to Reliable.
        #[serde(default)]
        mode: ConnectionMode,
    },
    /// Disconnect two nodes in a session's pipeline
    Disconnect {
        /// The session ID containing the nodes
        session_id: String,
        /// Source node ID
        from_node: String,
        /// Source output pin name
        from_pin: String,
        /// Destination node ID
        to_node: String,
        /// Destination input pin name
        to_pin: String,
    },
    /// Send a control message to a node and wait for response
    TuneNode {
        /// The session ID containing the node
        session_id: String,
        /// The node ID to send the message to
        node_id: String,
        /// The control message (UpdateParams, Start, or Shutdown)
        message: NodeControlMessage,
    },
    /// Fire-and-forget version of TuneNode for frequent updates.
    /// No response is sent, making it suitable for high-frequency parameter updates.
    TuneNodeAsync {
        /// The session ID containing the node
        session_id: String,
        /// The node ID to send the message to
        node_id: String,
        /// The control message (typically UpdateParams)
        message: NodeControlMessage,
    },
    /// Get the current pipeline state for a session
    GetPipeline {
        /// The session ID to query
        session_id: String,
    },
    /// Validate a batch of operations without applying them.
    /// Returns validation errors if any operations would fail.
    ValidateBatch {
        /// The session ID to validate operations against
        session_id: String,
        /// List of operations to validate
        operations: Vec<BatchOperation>,
    },
    /// Apply a batch of operations atomically.
    /// All operations succeed or all fail together.
    ApplyBatch {
        /// The session ID to apply operations to
        session_id: String,
        /// List of operations to apply atomically
        operations: Vec<BatchOperation>,
    },
    /// Get current user's permissions based on their role
    GetPermissions,
}

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
#[serde(tag = "action")]
#[serde(rename_all = "lowercase")]
pub enum BatchOperation {
    AddNode {
        node_id: String,
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(type = "JsonValue")]
        params: Option<serde_json::Value>,
    },
    RemoveNode {
        node_id: String,
    },
    Connect {
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
        #[serde(default)]
        mode: ConnectionMode,
    },
    Disconnect {
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
    },
}

pub type Request = Message<RequestPayload>;

// --- Server-to-Client Payloads (Responses & Events) ---

// Allowed: This is an API contract where explicit boolean fields provide clarity
// for TypeScript consumers. Using bitflags would complicate the API without benefit.
#[allow(clippy::struct_excessive_bools)]
#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export, export_to = "bindings/")]
pub struct PermissionsInfo {
    pub create_sessions: bool,
    pub destroy_sessions: bool,
    pub list_sessions: bool,
    pub modify_sessions: bool,
    pub tune_nodes: bool,
    pub load_plugins: bool,
    pub delete_plugins: bool,
    pub list_nodes: bool,
    pub list_samples: bool,
    pub read_samples: bool,
    pub write_samples: bool,
    pub delete_samples: bool,
    pub access_all_sessions: bool,
    pub upload_assets: bool,
    pub delete_assets: bool,
}

#[derive(Serialize, Deserialize, Debug, TS)]
#[ts(export)]
#[serde(tag = "action")]
#[serde(rename_all = "lowercase")]
pub enum ResponsePayload {
    SessionCreated {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// ISO 8601 formatted timestamp when the session was created
        created_at: String,
    },
    SessionDestroyed {
        session_id: String,
    },
    SessionsListed {
        sessions: Vec<SessionInfo>,
    },
    NodesListed {
        nodes: Vec<NodeDefinition>,
    },
    Pipeline {
        pipeline: ApiPipeline,
    },
    ValidationResult {
        errors: Vec<ValidationError>,
    },
    BatchApplied {
        success: bool,
        errors: Vec<String>,
    },
    Permissions {
        role: String,
        permissions: PermissionsInfo,
    },
    Success,
    Error {
        message: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
pub struct ValidationError {
    pub error_type: ValidationErrorType,
    pub message: String,
    pub node_id: Option<String>,
    pub connection_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum ValidationErrorType {
    Error,
    Warning,
}

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
pub struct SessionInfo {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// ISO 8601 formatted timestamp when the session was created
    pub created_at: String,
}

pub type Response = Message<ResponsePayload>;

// --- Event Payloads (Server-to-Client) ---

/// Events are asynchronous notifications sent from the server to subscribed clients.
/// Unlike responses, events are not correlated to specific requests.
#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
#[serde(tag = "event")]
#[serde(rename_all = "lowercase")]
pub enum EventPayload {
    /// A node's state has changed (e.g., from Running to Recovering).
    /// Clients can use this to update UI indicators and monitor pipeline health.
    NodeStateChanged {
        session_id: String,
        node_id: String,
        state: NodeState,
        /// ISO 8601 formatted timestamp
        timestamp: String,
    },
    /// A node's statistics have been updated (packets processed, discarded, errored).
    /// These updates are throttled at the source to prevent overload.
    NodeStatsUpdated {
        session_id: String,
        node_id: String,
        stats: NodeStats,
        /// ISO 8601 formatted timestamp
        timestamp: String,
    },
    /// A node's parameters have been updated.
    /// Clients can use this to keep their view of the pipeline state in sync.
    NodeParamsChanged {
        session_id: String,
        node_id: String,
        #[ts(type = "JsonValue")]
        params: serde_json::Value,
    },
    // --- Session Lifecycle Events ---
    SessionCreated {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// ISO 8601 formatted timestamp when the session was created
        created_at: String,
    },
    SessionDestroyed {
        session_id: String,
    },
    // --- Pipeline Structure Events ---
    NodeAdded {
        session_id: String,
        node_id: String,
        kind: String,
        #[ts(type = "JsonValue")]
        params: Option<serde_json::Value>,
    },
    NodeRemoved {
        session_id: String,
        node_id: String,
    },
    ConnectionAdded {
        session_id: String,
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
    },
    ConnectionRemoved {
        session_id: String,
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
    },
    // --- Telemetry Events ---
    /// Telemetry event from a node (transcription results, VAD events, LLM responses, etc.).
    /// The data payload contains event-specific fields including event_type for filtering.
    /// These events are best-effort and may be dropped under load.
    NodeTelemetry {
        /// The session this event belongs to
        session_id: String,
        /// The node that emitted this event
        node_id: String,
        /// Packet type identifier (e.g., "core::telemetry/event@1")
        type_id: String,
        /// Event payload containing event_type, correlation_id, turn_id, and event-specific data
        #[ts(type = "JsonValue")]
        data: serde_json::Value,
        /// Microsecond timestamp from the packet metadata (if available)
        #[serde(skip_serializing_if = "Option::is_none")]
        timestamp_us: Option<u64>,
        /// RFC 3339 formatted timestamp for convenience
        timestamp: String,
    },
}

pub type Event = Message<EventPayload>;

// --- Pipeline Types (merged from pipeline crate) ---

/// Engine execution mode
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum EngineMode {
    /// One-shot file conversion pipeline (requires http_input/http_output)
    #[serde(rename = "oneshot")]
    OneShot,
    /// Long-running dynamic pipeline (for real-time processing)
    #[default]
    Dynamic,
}

/// Represents a connection between two nodes in the graph.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, TS)]
#[ts(export)]
pub struct Connection {
    pub from_node: String,
    pub from_pin: String,
    pub to_node: String,
    pub to_pin: String,
    /// How this connection handles backpressure. Defaults to `Reliable`.
    #[serde(default, skip_serializing_if = "is_default_mode")]
    pub mode: ConnectionMode,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde skip_serializing_if requires reference
fn is_default_mode(mode: &ConnectionMode) -> bool {
    *mode == ConnectionMode::Reliable
}

/// Represents a single node's configuration within the pipeline.
#[derive(Debug, Deserialize, Serialize, Clone, TS)]
#[ts(export)]
pub struct Node {
    pub kind: String,
    #[ts(type = "JsonValue")]
    pub params: Option<serde_json::Value>,
    /// Runtime state (only populated in API responses)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<NodeState>,
}

/// The top-level structure for a pipeline definition, used by the engine and API.
#[derive(Debug, Deserialize, Serialize, Default, Clone, TS)]
#[ts(export)]
pub struct Pipeline {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub mode: EngineMode,
    #[ts(type = "Record<string, Node>")]
    pub nodes: indexmap::IndexMap<String, Node>,
    pub connections: Vec<Connection>,
}

// Type aliases for backwards compatibility
pub type ApiConnection = Connection;
pub type ApiNode = Node;
pub type ApiPipeline = Pipeline;

// --- Sample Pipelines (for oneshot converter) ---

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
pub struct SamplePipeline {
    pub id: String,
    pub name: String,
    pub description: String,
    pub yaml: String,
    pub is_system: bool,
    pub mode: String,
    /// Whether this is a reusable fragment (partial pipeline) vs a complete pipeline
    #[serde(default)]
    pub is_fragment: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
pub struct SavePipelineRequest {
    pub name: String,
    pub description: String,
    pub yaml: String,
    #[serde(default)]
    pub overwrite: bool,
    /// Whether this is a fragment (partial pipeline) vs a complete pipeline
    #[serde(default)]
    pub is_fragment: bool,
}

// --- Audio Assets ---

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
pub struct AudioAsset {
    /// Unique identifier (filename, including extension)
    pub id: String,
    /// Display name
    pub name: String,
    /// Server-relative path suitable for `core::file_reader` (e.g., `samples/audio/system/foo.wav`)
    pub path: String,
    /// File extension/format (opus, ogg, flac, mp3, wav)
    pub format: String,
    /// File size in bytes
    pub size_bytes: u64,
    /// License information from .license file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Whether this is a system asset (true) or user asset (false)
    pub is_system: bool,
}
