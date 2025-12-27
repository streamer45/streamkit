// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Re-export all generated types to provide a single entry point for the UI.
// Import the specific generated types needed for composing new types in this file.
import type {
  MessageType,
  RequestPayload,
  ResponsePayload,
  EventPayload,
} from './generated/api-types';

export * from './generated/api-types';

// --- Composed API message types ---
// These are based on the Rust `Message<T>` struct with `#[serde(flatten)]`.

/**
 * Generic WebSocket message wrapper.
 * All messages include a type discriminator and optional correlation ID for request/response matching.
 *
 * @template T - The payload type (RequestPayload, ResponsePayload, or EventPayload)
 */
export type Message<T> = {
  /** Message type discriminator: "request", "response", or "event" */
  type: MessageType;
  /** Correlation ID for matching requests with responses (absent for events) */
  correlation_id?: string;
  /** The actual message payload */
  payload: T;
};

/**
 * WebSocket request message sent from client to server.
 * Includes a correlation_id to match the response.
 */
export type Request = Message<RequestPayload>;

/**
 * WebSocket response message sent from server to client.
 * Includes the correlation_id from the corresponding request.
 */
export type Response = Message<ResponsePayload>;

/**
 * WebSocket event message broadcast from server to all connected clients.
 * No correlation_id since events are not request-driven.
 */
export type Event = Message<EventPayload>;

/**
 * Represents a node instance in the pipeline graph.
 * Used for both the staging area (design mode) and the live session graph.
 * This is the UI's representation - the backend uses Node which includes runtime state.
 */
export interface NodeInstance {
  /** Unique identifier for this node instance within the session */
  id: string;
  /** Node type identifier (e.g., "audio::gain", "plugin::native::whisper", "core::script") */
  kind: string;
  /** Position on the React Flow canvas (x, y coordinates in pixels) */
  position: { x: number; y: number };
  /** Optional JSON configuration parameters specific to this node type */
  params?: Record<string, unknown>;
}

/**
 * Data structure passed to React Flow custom node components.
 * Contains all the information needed to render a node and handle parameter updates.
 *
 * @see ConfigurableNode - The main React Flow node component that uses this data
 */
export interface CustomNodeData {
  /** Display label for the node (shown in the node's header) */
  label: string;
  /** Node type identifier (same as NodeInstance.kind) */
  kind: string;
  /** Current parameter values for this node instance */
  params: Record<string, unknown>;
  /** JSON Schema for parameter validation and UI generation (from NodeDefinition) */
  paramSchema?: unknown;
  /** Callback invoked when a parameter changes in the node's UI */
  onParamChange: (nodeId: string, paramName: string, value: unknown) => void;
}

/**
 * Plugin type discriminator.
 * - "wasm": WebAssembly Component Model plugin (sandboxed, ~50-200% overhead)
 * - "native": Native plugin via C ABI (trusted, ~0-5% overhead)
 */
export type PluginType = 'wasm' | 'native';

/**
 * Summary information for a loaded plugin.
 * Returned by the plugin management REST API endpoints.
 *
 * Plugins are automatically namespaced:
 * - Native: "plugin::native::name"
 * - WASM: "plugin::wasm::name"
 */
export interface PluginSummary {
  /** Fully qualified kind with namespace (e.g., "plugin::native::whisper") */
  kind: string;
  /** Original kind without the "plugin::<type>::" prefix (e.g., "whisper") */
  original_kind: string;
  /** Filename of the plugin binary (.so, .dylib, .dll, or .wasm) */
  file_name: string;
  /** Hierarchical categories for UI grouping (e.g., ["audio", "speech-to-text"]) */
  categories: string[];
  /** Unix timestamp in milliseconds when the plugin was loaded */
  loaded_at_ms: number;
  /** Plugin type (wasm or native) */
  plugin_type: PluginType;
}
