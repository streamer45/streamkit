// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Re-export all generated types; import only what's needed for composed types below.
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
  /** Plugin version from the marketplace record or local manifest, if available */
  version?: string | null;
  /** Accelerator variant of the installed bundle (e.g. "cpu", "cuda"), if known */
  accelerator?: string | null;
  /** Human-readable plugin description from metadata or manifest, if available */
  description?: string | null;
}
