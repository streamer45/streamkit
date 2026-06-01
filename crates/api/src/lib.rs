// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! api: Defines the WebSocket API contract for StreamKit.
//!
//! All API communication uses JSON for parameters and payloads.
//! While pipeline YAML files are still supported internally, the WebSocket API
//! contract exclusively uses JSON for consistency and TypeScript compatibility.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

pub mod yaml;

pub use streamkit_core::control::{ConnectionMode, NodeControlMessage};
pub use streamkit_core::{NodeDefinition, NodeState, NodeStats};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum MessageType {
    Request,
    Response,
    Event,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message<T> {
    #[serde(rename = "type")]
    pub message_type: MessageType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub payload: T,
}

#[derive(Serialize, Deserialize, Debug, TS)]
#[ts(export)]
#[serde(tag = "action")]
#[serde(rename_all = "lowercase")]
pub enum RequestPayload {
    CreateSession {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    DestroySession {
        session_id: String,
    },
    ListSessions,
    ListNodes,
    AddNode {
        session_id: String,
        node_id: String,
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(type = "JsonValue")]
        params: Option<serde_json::Value>,
    },
    RemoveNode {
        session_id: String,
        node_id: String,
    },
    Connect {
        session_id: String,
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
        #[serde(default)]
        mode: ConnectionMode,
    },
    Disconnect {
        session_id: String,
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
    },
    TuneNode {
        session_id: String,
        node_id: String,
        message: NodeControlMessage,
    },
    /// Fire-and-forget; no response sent.
    TuneNodeAsync {
        session_id: String,
        node_id: String,
        message: NodeControlMessage,
    },
    GetPipeline {
        session_id: String,
    },
    ValidateBatch {
        session_id: String,
        operations: Vec<BatchOperation>,
    },
    /// All operations succeed or all fail together.
    ApplyBatch {
        session_id: String,
        operations: Vec<BatchOperation>,
    },
    GetPermissions,
}

#[derive(Serialize, Deserialize, Debug, Clone, TS, schemars::JsonSchema)]
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

#[allow(clippy::struct_excessive_bools)] // API contract: explicit bool fields for TS consumers
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
        pipeline: Box<ApiPipeline>,
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
    pub created_at: String,
}

pub type Response = Message<ResponsePayload>;

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
#[serde(tag = "event")]
#[serde(rename_all = "lowercase")]
pub enum EventPayload {
    NodeStateChanged {
        session_id: String,
        node_id: String,
        state: NodeState,
        timestamp: String,
    },
    NodeStatsUpdated {
        session_id: String,
        node_id: String,
        stats: NodeStats,
        timestamp: String,
    },
    NodeParamsChanged {
        session_id: String,
        node_id: String,
        #[ts(type = "JsonValue")]
        params: serde_json::Value,
    },
    SessionCreated {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        created_at: String,
    },
    SessionDestroyed {
        session_id: String,
    },
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
    NodeViewDataUpdated {
        session_id: String,
        node_id: String,
        #[ts(type = "JsonValue")]
        data: serde_json::Value,
        timestamp: String,
    },
    /// Best-effort; may be dropped under load.
    NodeTelemetry {
        session_id: String,
        node_id: String,
        type_id: String,
        #[ts(type = "JsonValue")]
        data: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        timestamp_us: Option<u64>,
        timestamp: String,
    },
    /// UI should merge with static per-kind schema for dynamically discovered params.
    RuntimeSchemasUpdated {
        session_id: String,
        node_id: String,
        #[ts(type = "JsonValue")]
        schema: serde_json::Value,
    },
}

pub type Event = Message<EventPayload>;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum EngineMode {
    #[serde(rename = "oneshot")]
    OneShot,
    #[default]
    Dynamic,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, TS)]
#[ts(export)]
pub struct Connection {
    pub from_node: String,
    pub from_pin: String,
    pub to_node: String,
    pub to_pin: String,
    #[serde(default, skip_serializing_if = "is_default_mode")]
    pub mode: ConnectionMode,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde skip_serializing_if requires reference
fn is_default_mode(mode: &ConnectionMode) -> bool {
    *mode == ConnectionMode::Reliable
}

#[derive(Debug, Deserialize, Serialize, Clone, TS)]
#[ts(export)]
pub struct Node {
    pub kind: String,
    #[ts(type = "JsonValue")]
    pub params: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<NodeState>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, TS)]
#[ts(export)]
pub struct Pipeline {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub mode: EngineMode,
    /// Declarative key/value attributes describing the pipeline as a whole
    /// (e.g. `service: tts`). Telemetry-neutral; the server uses them, bounded
    /// by operator policy, to label pipeline and node metrics.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    #[ts(type = "Record<string, string> | null")]
    pub attributes: Option<std::collections::BTreeMap<String, String>>,
    /// Declarative UI metadata; ignored by the engine.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub client: Option<yaml::ClientSection>,
    #[ts(type = "Record<string, Node>")]
    pub nodes: indexmap::IndexMap<String, Node>,
    pub connections: Vec<Connection>,
    /// Resolved per-node view data (e.g., compositor layout).
    /// Only populated in API responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    #[ts(type = "Record<string, JsonValue> | null")]
    pub view_data: Option<HashMap<String, serde_json::Value>>,
    /// Only populated in API responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    #[ts(type = "Record<string, JsonValue> | null")]
    pub runtime_schemas: Option<HashMap<String, serde_json::Value>>,
}

pub type ApiConnection = Connection;
pub type ApiNode = Node;
pub type ApiPipeline = Pipeline;

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
pub struct SamplePipeline {
    pub id: String,
    pub name: String,
    pub description: String,
    pub yaml: String,
    pub is_system: bool,
    pub mode: String,
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
    #[serde(default)]
    pub is_fragment: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
pub struct AudioAsset {
    pub id: String,
    pub name: String,
    pub path: String,
    pub format: String,
    #[ts(type = "number")]
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    pub is_system: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
pub struct FontAsset {
    pub id: String,
    pub name: String,
    pub path: String,
    pub format: String,
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub is_system: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
pub struct PluginAsset {
    pub id: String,
    pub name: String,
    pub path: String,
    pub format: String,
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub is_system: bool,
    pub type_id: String,
    pub plugin_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum AssetTypeSource {
    Core,
    Plugin,
}

/// Returned by `GET /api/v1/asset-types`.
#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
pub struct AssetTypeInfo {
    pub type_id: String,
    pub label: String,
    pub source: AssetTypeSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_param: Option<String>,
    pub extensions: Vec<String>,
    pub icon_hint: String,
    pub editable: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
pub struct ImageAsset {
    pub id: String,
    pub name: String,
    pub path: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub is_system: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_json<T: for<'de> Deserialize<'de>>(s: &str) -> T {
        serde_json::from_str(s).expect("valid json fixture")
    }

    fn to_value<T: Serialize>(value: &T) -> serde_json::Value {
        serde_json::to_value(value).expect("serializable")
    }

    #[test]
    fn message_type_lowercase_roundtrip() {
        for (variant, raw) in [
            (MessageType::Request, "\"request\""),
            (MessageType::Response, "\"response\""),
            (MessageType::Event, "\"event\""),
        ] {
            let serialized = serde_json::to_string(&variant).expect("serialize");
            assert_eq!(serialized, raw, "{variant:?} should serialize to {raw}");
            let parsed: MessageType = from_json(raw);
            assert_eq!(parsed, variant, "{raw} should deserialize to {variant:?}");
        }
        assert!(
            serde_json::from_str::<MessageType>("\"Request\"").is_err(),
            "capitalized variants must be rejected"
        );
    }

    #[test]
    fn request_envelope_roundtrips_with_correlation_id() {
        let req = Request {
            message_type: MessageType::Request,
            correlation_id: Some("abc123".into()),
            payload: RequestPayload::CreateSession { name: Some("demo".into()) },
        };
        let json = to_value(&req);
        assert_eq!(json["type"], "request");
        assert_eq!(json["correlation_id"], "abc123");
        assert_eq!(json["payload"]["action"], "createsession");
        assert_eq!(json["payload"]["name"], "demo");

        let parsed: Request = serde_json::from_value(json).expect("deserialize");
        assert_eq!(parsed.correlation_id.as_deref(), Some("abc123"));
        assert!(matches!(parsed.message_type, MessageType::Request));
        match parsed.payload {
            RequestPayload::CreateSession { name } => assert_eq!(name.as_deref(), Some("demo")),
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn response_envelope_roundtrips_with_correlation_id() {
        let resp = Response {
            message_type: MessageType::Response,
            correlation_id: Some("abc123".into()),
            payload: ResponsePayload::SessionCreated {
                session_id: "sess".into(),
                name: Some("demo".into()),
                created_at: "2026-01-01T00:00:00Z".into(),
            },
        };
        let json = to_value(&resp);
        assert_eq!(json["type"], "response");
        assert_eq!(json["correlation_id"], "abc123");
        assert_eq!(json["payload"]["action"], "sessioncreated");
        assert_eq!(json["payload"]["session_id"], "sess");

        let parsed: Response = serde_json::from_value(json).expect("deserialize");
        match parsed.payload {
            ResponsePayload::SessionCreated { session_id, name, .. } => {
                assert_eq!(session_id, "sess");
                assert_eq!(name.as_deref(), Some("demo"));
            },
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn event_envelope_omits_correlation_id_when_none() {
        let event = Event {
            message_type: MessageType::Event,
            correlation_id: None,
            payload: EventPayload::SessionDestroyed { session_id: "sess".into() },
        };
        let json = to_value(&event);
        assert_eq!(json["type"], "event");
        assert!(
            json.get("correlation_id").is_none(),
            "correlation_id must be omitted for events, got: {json}"
        );
        assert_eq!(json["payload"]["event"], "sessiondestroyed");
        assert_eq!(json["payload"]["session_id"], "sess");

        let parsed: Event = serde_json::from_value(json).expect("deserialize");
        assert!(parsed.correlation_id.is_none());
    }

    #[test]
    fn request_payload_action_tag_is_lowercase() {
        let req = RequestPayload::AddNode {
            session_id: "sess".into(),
            node_id: "n1".into(),
            kind: "audio::gain".into(),
            params: Some(serde_json::json!({"gain_db": 0.5})),
        };
        let json = to_value(&req);
        assert_eq!(json["action"], "addnode");
        assert_eq!(json["params"]["gain_db"], 0.5);
    }

    #[test]
    fn response_payload_error_tag_is_lowercase() {
        let resp = ResponsePayload::Error { message: "boom".into() };
        let json = to_value(&resp);
        assert_eq!(json["action"], "error");
        assert_eq!(json["message"], "boom");
    }

    #[test]
    fn event_payload_tag_is_lowercase() {
        let event = EventPayload::NodeAdded {
            session_id: "sess".into(),
            node_id: "n1".into(),
            kind: "audio::gain".into(),
            params: None,
        };
        let json = to_value(&event);
        assert_eq!(json["event"], "nodeadded");
    }

    #[test]
    fn batch_operation_action_tag_roundtrips() {
        let ops = vec![
            BatchOperation::AddNode {
                node_id: "n1".into(),
                kind: "audio::gain".into(),
                params: None,
            },
            BatchOperation::Connect {
                from_node: "n1".into(),
                from_pin: "out".into(),
                to_node: "n2".into(),
                to_pin: "in".into(),
                mode: ConnectionMode::default(),
            },
        ];
        let json = to_value(&ops);
        assert_eq!(json[0]["action"], "addnode");
        assert_eq!(json[1]["action"], "connect");

        let parsed: Vec<BatchOperation> = serde_json::from_value(json).expect("deserialize");
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn validation_error_type_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&ValidationErrorType::Error).expect("serialize"),
            "\"error\""
        );
        assert_eq!(
            serde_json::to_string(&ValidationErrorType::Warning).expect("serialize"),
            "\"warning\""
        );
    }

    #[test]
    fn engine_mode_serializes_with_lowercase_and_oneshot_alias() {
        assert_eq!(serde_json::to_string(&EngineMode::Dynamic).expect("serialize"), "\"dynamic\"");
        assert_eq!(serde_json::to_string(&EngineMode::OneShot).expect("serialize"), "\"oneshot\"");
        let parsed: EngineMode = from_json("\"oneshot\"");
        assert!(matches!(parsed, EngineMode::OneShot));
    }

    #[test]
    fn engine_mode_default_is_dynamic() {
        assert!(matches!(EngineMode::default(), EngineMode::Dynamic));
    }

    #[test]
    fn connection_skips_default_mode_on_serialize() {
        let conn = Connection {
            from_node: "a".into(),
            from_pin: "out".into(),
            to_node: "b".into(),
            to_pin: "in".into(),
            mode: ConnectionMode::Reliable,
        };
        let json = to_value(&conn);
        assert!(json.get("mode").is_none(), "default mode should be skipped: {json}");
    }

    #[test]
    fn connection_emits_non_default_mode() {
        let conn = Connection {
            from_node: "a".into(),
            from_pin: "out".into(),
            to_node: "b".into(),
            to_pin: "in".into(),
            mode: ConnectionMode::BestEffort,
        };
        let json = to_value(&conn);
        assert_eq!(json["mode"], "best_effort");
    }

    #[test]
    fn pipeline_default_roundtrips() {
        let pipeline = Pipeline::default();
        let json = to_value(&pipeline);
        assert_eq!(json["mode"], "dynamic");
        assert!(json["nodes"].is_object());
        assert!(json["connections"].is_array());

        let parsed: Pipeline = serde_json::from_value(json).expect("deserialize");
        assert!(matches!(parsed.mode, EngineMode::Dynamic));
        assert!(parsed.nodes.is_empty());
        assert!(parsed.connections.is_empty());
    }

    // Confirms the `pub use` re-export from streamkit-core works and that
    // the type round-trips through serde at this crate boundary — clients
    // consume `streamkit_api::NodeDefinition` directly.
    #[test]
    fn node_definition_reexport_roundtrips_via_serde() {
        let raw = serde_json::json!({
            "kind": "audio::gain",
            "description": "Adjust gain",
            "param_schema": {"type": "object"},
            "inputs": [],
            "outputs": [],
            "categories": ["audio"],
            "bidirectional": false,
        });
        let parsed: NodeDefinition = serde_json::from_value(raw.clone()).expect("deserialize");
        assert_eq!(parsed.kind, "audio::gain");
        assert_eq!(parsed.description.as_deref(), Some("Adjust gain"));
        assert_eq!(parsed.categories, vec!["audio"]);
        assert!(!parsed.bidirectional);

        let reserialized = to_value(&parsed);
        assert_eq!(reserialized["kind"], "audio::gain");
        assert_eq!(reserialized["bidirectional"], false);
    }

    #[test]
    fn pipeline_roundtrips_with_nodes_and_connections() {
        use indexmap::IndexMap;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "src".to_string(),
            Node {
                kind: "core::file_reader".into(),
                params: Some(serde_json::json!({"path": "x"})),
                state: None,
            },
        );
        nodes.insert(
            "sink".to_string(),
            Node { kind: "core::sink".into(), params: None, state: None },
        );
        let pipeline = Pipeline {
            name: Some("demo".into()),
            description: None,
            mode: EngineMode::OneShot,
            attributes: None,
            client: None,
            nodes,
            connections: vec![Connection {
                from_node: "src".into(),
                from_pin: "out".into(),
                to_node: "sink".into(),
                to_pin: "in".into(),
                mode: ConnectionMode::Reliable,
            }],
            view_data: None,
            runtime_schemas: None,
        };

        let json = to_value(&pipeline);
        assert_eq!(json["mode"], "oneshot");
        assert_eq!(json["nodes"]["src"]["kind"], "core::file_reader");
        assert!(json["nodes"]["src"].get("state").is_none(), "absent state must be omitted");

        let parsed: Pipeline = serde_json::from_value(json).expect("deserialize");
        assert_eq!(parsed.nodes.len(), 2);
        assert_eq!(parsed.connections.len(), 1);
        assert!(matches!(parsed.mode, EngineMode::OneShot));
    }

    #[test]
    fn message_with_unknown_correlation_id_is_optional() {
        let raw = serde_json::json!({
            "type": "event",
            "payload": { "event": "sessiondestroyed", "session_id": "s" }
        });
        let parsed: Event = serde_json::from_value(raw).expect("deserialize");
        assert!(parsed.correlation_id.is_none());
    }

    #[test]
    fn permissions_info_roundtrips() {
        let perms = PermissionsInfo {
            create_sessions: true,
            destroy_sessions: false,
            list_sessions: true,
            modify_sessions: true,
            tune_nodes: false,
            load_plugins: false,
            delete_plugins: false,
            list_nodes: true,
            list_samples: true,
            read_samples: true,
            write_samples: false,
            delete_samples: false,
            access_all_sessions: false,
            upload_assets: true,
            delete_assets: false,
        };
        let json = to_value(&perms);
        assert_eq!(json["create_sessions"], true);
        assert_eq!(json["tune_nodes"], false);

        let parsed: PermissionsInfo = serde_json::from_value(json).expect("deserialize");
        assert!(parsed.create_sessions);
        assert!(!parsed.tune_nodes);
    }
}
