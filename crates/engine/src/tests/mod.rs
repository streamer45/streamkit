// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Unit tests for the engine crate.

#[cfg(feature = "dynamic")]
mod async_node_creation;
#[cfg(feature = "dynamic")]
mod connection_types;
#[cfg(feature = "dynamic")]
mod dynamic_initialize;
mod graph_builder;
mod oneshot;
mod oneshot_linear;
#[cfg(feature = "dynamic")]
mod pin_distributor;
#[cfg(feature = "dynamic")]
mod pipeline_activation;
#[cfg(feature = "dynamic")]
mod upstream_hints;
