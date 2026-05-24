// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Unit tests for the engine crate.

#[cfg(feature = "dynamic")]
mod async_node_creation;
#[cfg(feature = "dynamic")]
mod connection_types;
#[cfg(feature = "dynamic")]
mod dynamic_actor_edges;
#[cfg(feature = "dynamic")]
mod dynamic_actor_handlers;
#[cfg(feature = "dynamic")]
mod dynamic_actor_runtime_schemas;
#[cfg(feature = "dynamic")]
mod dynamic_actor_update_filters;
#[cfg(feature = "dynamic")]
mod dynamic_handle;
#[cfg(feature = "dynamic")]
mod dynamic_initialize;
mod engine_construction;
mod graph_builder;
mod oneshot;
mod oneshot_linear;
#[cfg(feature = "dynamic")]
mod pin_distributor;
#[cfg(feature = "dynamic")]
mod pipeline_activation;
#[cfg(feature = "dynamic")]
mod upstream_hints;
