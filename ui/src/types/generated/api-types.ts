// This file is auto-generated. Do not edit it manually.

// Keep loose to allow schema usage in UI
export type JsonValue = unknown;

// streamkit-core
export type SampleFormat = "F32" | "S16Le";

export type AudioFormat = { sample_rate: number, channels: number, sample_format: SampleFormat, };

export type PacketMetadata = { 
/**
 * Absolute timestamp in microseconds (presentation time)
 */
timestamp_us: bigint | null, 
/**
 * Duration of this packet/frame in microseconds
 */
duration_us: bigint | null, 
/**
 * Sequence number for ordering and detecting loss
 */
sequence: bigint | null, };

export type TranscriptionSegment = { 
/**
 * The transcribed text for this segment
 */
text: string, 
/**
 * Start time in milliseconds
 */
start_time_ms: bigint, 
/**
 * End time in milliseconds
 */
end_time_ms: bigint, 
/**
 * Confidence score (0.0 - 1.0), if available
 */
confidence: number | null, };

export type TranscriptionData = { 
/**
 * The full transcribed text (concatenation of all segments)
 */
text: string, 
/**
 * Individual segments with timing information
 */
segments: Array<TranscriptionSegment>, 
/**
 * Detected or specified language code (e.g., "en", "es", "fr")
 */
language: string | null, 
/**
 * Optional timing metadata for the entire transcription
 */
metadata: PacketMetadata | null, };

export type PacketType = { "RawAudio": AudioFormat } | "OpusAudio" | "Text" | "Transcription" | { "Custom": { type_id: string, } } | "Binary" | "Any" | "Passthrough";

export type PinCardinality = "One" | "Broadcast" | { "Dynamic": { prefix: string, } };

export type InputPin = { name: string, accepts_types: Array<PacketType>, cardinality: PinCardinality, };

export type OutputPin = { name: string, produces_type: PacketType, cardinality: PinCardinality, };

export type NodeDefinition = { kind: string, 
/**
 * Human-readable description of what this node does.
 * This is separate from the param_schema description which describes the config struct.
 */
description?: string | null, param_schema: JsonValue, inputs: Array<InputPin>, outputs: Array<OutputPin>, 
/**
 * Hierarchical categories for UI grouping (e.g., `["audio", "filters"]`)
 */
categories: Array<string>, 
/**
 * Whether this node is bidirectional (has both input and output for the same data flow)
 */
bidirectional: boolean, };

export type StopReason = "completed" | "input_closed" | "output_closed" | "shutdown" | "no_inputs" | "unknown";

export type NodeState = "Initializing" | "Ready" | "Running" | { "Recovering": { reason: string, details: JsonValue, } } | { "Degraded": { reason: string, } } | { "Failed": { reason: string, } } | { "Stopped": { reason: StopReason, } };

export type NodeStats = { 
/**
 * Total packets received on all input pins
 */
received: bigint, 
/**
 * Total packets successfully sent on all output pins
 */
sent: bigint, 
/**
 * Total packets discarded (e.g., due to backpressure, invalid data)
 */
discarded: bigint, 
/**
 * Total processing errors that didn't crash the node
 */
errored: bigint, 
/**
 * Duration in seconds since the node started processing (for rate calculation)
 */
duration_secs: number, };

export type NodeControlMessage = { "UpdateParams": JsonValue } | "Start" | "Shutdown";

export type FieldRule = { name: string, wildcard_value: JsonValue | null, };

export type Compatibility = { "kind": "any" } | { "kind": "exact" } | { "kind": "structfieldwildcard", fields: Array<FieldRule>, };

export type PacketTypeMeta = { 
/**
 * Variant identifier (e.g., "RawAudio", "OpusAudio", "Binary", "Any").
 */
id: string, 
/**
 * Human-friendly default label.
 */
label: string, 
/**
 * Hex color to use in UIs.
 */
color: string, 
/**
 * Optional display template for struct payloads. Placeholders are field names,
 * optionally with "|*" to indicate wildcard-display (handled on the client).
 * Example: "Raw Audio ({sample_rate|*}Hz, {channels|*}ch, {sample_format})"
 */
display_template: string | null, 
/**
 * Compatibility strategy for this type.
 */
compatibility: Compatibility, };


// streamkit-api
export type MessageType = "request" | "response" | "event";

export type RequestPayload = { "action": "createsession", 
/**
 * Optional session name for identification
 */
name: string | null, } | { "action": "destroysession", 
/**
 * The session ID to destroy
 */
session_id: string, } | { "action": "listsessions" } | { "action": "listnodes" } | { "action": "addnode", 
/**
 * The session ID to add the node to
 */
session_id: string, 
/**
 * Unique identifier for this node instance
 */
node_id: string, 
/**
 * Node type (e.g., "audio::gain", "plugin::native::whisper")
 */
kind: string, 
/**
 * Optional JSON configuration parameters for the node
 */
params: JsonValue, } | { "action": "removenode", 
/**
 * The session ID containing the node
 */
session_id: string, 
/**
 * The node ID to remove
 */
node_id: string, } | { "action": "connect", 
/**
 * The session ID containing the nodes
 */
session_id: string, 
/**
 * Source node ID
 */
from_node: string, 
/**
 * Source output pin name
 */
from_pin: string, 
/**
 * Destination node ID
 */
to_node: string, 
/**
 * Destination input pin name
 */
to_pin: string, 
/**
 * Connection mode (reliable or best-effort). Defaults to Reliable.
 */
mode: ConnectionMode, } | { "action": "disconnect", 
/**
 * The session ID containing the nodes
 */
session_id: string, 
/**
 * Source node ID
 */
from_node: string, 
/**
 * Source output pin name
 */
from_pin: string, 
/**
 * Destination node ID
 */
to_node: string, 
/**
 * Destination input pin name
 */
to_pin: string, } | { "action": "tunenode", 
/**
 * The session ID containing the node
 */
session_id: string, 
/**
 * The node ID to send the message to
 */
node_id: string, 
/**
 * The control message (UpdateParams, Start, or Shutdown)
 */
message: NodeControlMessage, } | { "action": "tunenodeasync", 
/**
 * The session ID containing the node
 */
session_id: string, 
/**
 * The node ID to send the message to
 */
node_id: string, 
/**
 * The control message (typically UpdateParams)
 */
message: NodeControlMessage, } | { "action": "getpipeline", 
/**
 * The session ID to query
 */
session_id: string, } | { "action": "validatebatch", 
/**
 * The session ID to validate operations against
 */
session_id: string, 
/**
 * List of operations to validate
 */
operations: Array<BatchOperation>, } | { "action": "applybatch", 
/**
 * The session ID to apply operations to
 */
session_id: string, 
/**
 * List of operations to apply atomically
 */
operations: Array<BatchOperation>, } | { "action": "getpermissions" };

export type ResponsePayload = { "action": "sessioncreated", session_id: string, name: string | null, 
/**
 * ISO 8601 formatted timestamp when the session was created
 */
created_at: string, } | { "action": "sessiondestroyed", session_id: string, } | { "action": "sessionslisted", sessions: Array<SessionInfo>, } | { "action": "nodeslisted", nodes: Array<NodeDefinition>, } | { "action": "pipeline", pipeline: Pipeline, } | { "action": "validationresult", errors: Array<ValidationError>, } | { "action": "batchapplied", success: boolean, errors: Array<string>, } | { "action": "permissions", role: string, permissions: PermissionsInfo, } | { "action": "success" } | { "action": "error", message: string, };

export type EventPayload = { "event": "nodestatechanged", session_id: string, node_id: string, state: NodeState, 
/**
 * ISO 8601 formatted timestamp
 */
timestamp: string, } | { "event": "nodestatsupdated", session_id: string, node_id: string, stats: NodeStats, 
/**
 * ISO 8601 formatted timestamp
 */
timestamp: string, } | { "event": "nodeparamschanged", session_id: string, node_id: string, params: JsonValue, } | { "event": "sessioncreated", session_id: string, name: string | null, 
/**
 * ISO 8601 formatted timestamp when the session was created
 */
created_at: string, } | { "event": "sessiondestroyed", session_id: string, } | { "event": "nodeadded", session_id: string, node_id: string, kind: string, params: JsonValue, } | { "event": "noderemoved", session_id: string, node_id: string, } | { "event": "connectionadded", session_id: string, from_node: string, from_pin: string, to_node: string, to_pin: string, } | { "event": "connectionremoved", session_id: string, from_node: string, from_pin: string, to_node: string, to_pin: string, } | { "event": "nodetelemetry", 
/**
 * The session this event belongs to
 */
session_id: string, 
/**
 * The node that emitted this event
 */
node_id: string, 
/**
 * Packet type identifier (e.g., "core::telemetry/event@1")
 */
type_id: string, 
/**
 * Event payload containing event_type, correlation_id, turn_id, and event-specific data
 */
data: JsonValue, 
/**
 * Microsecond timestamp from the packet metadata (if available)
 */
timestamp_us: bigint | null, 
/**
 * RFC 3339 formatted timestamp for convenience
 */
timestamp: string, };

export type SessionInfo = { id: string, name: string | null, 
/**
 * ISO 8601 formatted timestamp when the session was created
 */
created_at: string, };

export type EngineMode = "oneshot" | "dynamic";

export type ConnectionMode = "reliable" | "best_effort";

export type Connection = { from_node: string, from_pin: string, to_node: string, to_pin: string, 
/**
 * How this connection handles backpressure. Defaults to `Reliable`.
 */
mode?: ConnectionMode, };

export type Node = { kind: string, params: JsonValue, 
/**
 * Runtime state (only populated in API responses)
 */
state: NodeState | null, };

export type Pipeline = { name: string | null, description: string | null, mode: EngineMode, nodes: Record<string, Node>, connections: Array<Connection>, };

export type SamplePipeline = { id: string, name: string, description: string, yaml: string, is_system: boolean, mode: string, 
/**
 * Whether this is a reusable fragment (partial pipeline) vs a complete pipeline
 */
is_fragment: boolean, };

export type SavePipelineRequest = { name: string, description: string, yaml: string, overwrite: boolean, 
/**
 * Whether this is a fragment (partial pipeline) vs a complete pipeline
 */
is_fragment: boolean, };

export type AudioAsset = { 
/**
 * Unique identifier (filename without extension)
 */
id: string, 
/**
 * Display name
 */
name: string, 
/**
 * Absolute path on the server
 */
path: string, 
/**
 * File extension/format (opus, ogg, flac, mp3, wav)
 */
format: string, 
/**
 * File size in bytes
 */
size_bytes: bigint, 
/**
 * License information from .license file
 */
license: string | null, 
/**
 * Whether this is a system asset (true) or user asset (false)
 */
is_system: boolean, };

export type BatchOperation = { "action": "addnode", node_id: string, kind: string, params: JsonValue, } | { "action": "removenode", node_id: string, } | { "action": "connect", from_node: string, from_pin: string, to_node: string, to_pin: string, mode: ConnectionMode, } | { "action": "disconnect", from_node: string, from_pin: string, to_node: string, to_pin: string, };

export type ValidationError = { error_type: ValidationErrorType, message: string, node_id: string | null, connection_id: string | null, };

export type ValidationErrorType = "error" | "warning";

export type PermissionsInfo = { create_sessions: boolean, destroy_sessions: boolean, list_sessions: boolean, modify_sessions: boolean, tune_nodes: boolean, load_plugins: boolean, delete_plugins: boolean, list_nodes: boolean, list_samples: boolean, read_samples: boolean, write_samples: boolean, delete_samples: boolean, access_all_sessions: boolean, upload_assets: boolean, delete_assets: boolean, };