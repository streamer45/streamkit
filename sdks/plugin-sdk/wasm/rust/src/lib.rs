// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! # StreamKit Plugin SDK
//!
//! This crate provides pre-generated type bindings for writing StreamKit plugins in Rust.
//!
//! ## Quick Start
//!
//! 1. Add dependencies to your `Cargo.toml`:
//! ```toml
//! [dependencies]
//! streamkit-plugin-sdk-wasm = "0.1.0"
//! wit-bindgen = "0.44"
//! serde_json = "1"
//!
//! [lib]
//! crate-type = ["cdylib"]
//! ```
//!
//! 2. Use `wit_bindgen::generate!` with the SDK's pre-generated types:
//! ```rust,ignore
//! use streamkit_plugin_sdk_wasm as sdk;
//!
//! // Generate bindings, reusing SDK types for faster compilation
//! wit_bindgen::generate!({
//!     world: "plugin",
//!     path: "wit",
//!     generate_all,
//!     with: {
//!         "streamkit:plugin/types@0.1.0": sdk::types,
//!         "streamkit:plugin/host@0.1.0": sdk::host,
//!     },
//! });
//!
//! // Now use the generated traits with SDK types
//! use exports::streamkit::plugin::node::{Guest, GuestNodeInstance};
//! use sdk::{NodeMetadata, InputPin, OutputPin, Packet, PacketType, AudioFormat, SampleFormat};
//!
//! struct MyPlugin;
//! struct MyNodeInstance { /* your state here */ }
//!
//! impl Guest for MyPlugin {
//!     type NodeInstance = MyNodeInstance;
//!
//!     fn metadata() -> NodeMetadata {
//!         NodeMetadata {
//!             kind: "my_plugin".to_string(),
//!             inputs: vec![InputPin { /* ... */ }],
//!             outputs: vec![/* ... */],
//!             param_schema: "{}".to_string(),
//!             categories: vec!["audio".to_string()],
//!         }
//!     }
//! }
//!
//! impl GuestNodeInstance for MyNodeInstance {
//!     fn new(params: Option<String>) -> Self {
//!         Self { /* ... */ }
//!     }
//!
//!     fn process(&self, input_pin: String, packet: Packet) -> Result<(), String> {
//!         Ok(())
//!     }
//!
//!     fn update_params(&self, params: Option<String>) -> Result<(), String> {
//!         Ok(())
//!     }
//!
//!     fn cleanup(&self) {}
//! }
//!
//! // Export using the generated macro
//! export!(MyPlugin);
//! ```
//!
//! ## Why This Approach?
//!
//! This hybrid approach gives you the best of both worlds:
//! - ✅ **Fast compilation**: Types are pre-generated in the SDK
//! - ✅ **Correct WASM exports**: The `export!()` macro generates fresh exports in your plugin
//! - ✅ **Simple dependency**: Just add `streamkit-plugin-sdk` + `wit-bindgen`
//! - ✅ **Versioned API**: SDK types are stable and versioned
//!
//! The `with` parameter tells wit-bindgen to reuse our pre-compiled types instead of
//! regenerating them, saving compile time while still generating the necessary export
//! glue code in your plugin's WASM binary.

pub mod generated;

// Re-export types module for use with `with` parameter
pub use generated::streamkit::plugin::host;
pub use generated::streamkit::plugin::types;

// Also re-export individual types for convenience
pub use types::*;
