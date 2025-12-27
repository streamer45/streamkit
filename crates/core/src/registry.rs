// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Node factory registry and discovery.
//!
//! This module provides the factory pattern for creating processing nodes:
//! - [`NodeRegistry`]: Central registry of all available node types
//! - [`NodeDefinition`]: Serializable node metadata for API exposure
//! - Factory types for node and resource creation

use crate::error::StreamKitError;
use crate::node::{NodeFactory, ProcessorNode, ResourceKeyHasher};
use crate::pins::{InputPin, OutputPin};
use crate::resource_manager::{Resource, ResourceError, ResourceKey, ResourceManager};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use ts_rs::TS;

/// Type alias for async resource factories used by the NodeRegistry.
/// Returns a Future that resolves to a Resource that will be shared across node instances.
pub type AsyncResourceFactory = Arc<
    dyn Fn(
            Option<serde_json::Value>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Arc<dyn Resource>, ResourceError>> + Send>,
        > + Send
        + Sync,
>;

/// A serializable representation of a node's definition for API exposure.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct NodeDefinition {
    pub kind: String,
    /// Human-readable description of what this node does.
    /// This is separate from the param_schema description which describes the config struct.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub param_schema: serde_json::Value,
    pub inputs: Vec<InputPin>,
    pub outputs: Vec<OutputPin>,
    /// Hierarchical categories for UI grouping (e.g., `["audio", "filters"]`)
    pub categories: Vec<String>,
    /// Whether this node is bidirectional (has both input and output for the same data flow)
    #[serde(default)]
    pub bidirectional: bool,
}

/// Static pin configuration for nodes with fixed pins.
#[derive(Clone)]
pub struct StaticPins {
    pub inputs: Vec<InputPin>,
    pub outputs: Vec<OutputPin>,
}

/// Internal node registration information.
#[derive(Clone)]
pub(crate) struct NodeInfo {
    pub factory: NodeFactory,
    pub param_schema: serde_json::Value,
    pub static_pins: Option<StaticPins>,
    pub categories: Vec<String>,
    pub bidirectional: bool,
    /// Human-readable description of what this node does
    pub description: Option<String>,
    /// Optional resource factory for nodes that need shared resources (e.g., ML models)
    pub resource_factory: Option<AsyncResourceFactory>,
    /// Optional key hasher for computing resource cache keys from parameters
    pub resource_key_hasher: Option<ResourceKeyHasher>,
}

/// The NodeRegistry holds all available node types that the engine can construct.
#[derive(Clone, Default)]
pub struct NodeRegistry {
    info: HashMap<String, NodeInfo>,
    /// Optional resource manager for shared resources (e.g., ML models)
    #[allow(clippy::type_complexity)]
    resource_manager: Option<Arc<ResourceManager>>,
}

impl NodeRegistry {
    /// Creates a new, empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new registry with resource management support.
    pub fn with_resource_manager(resource_manager: Arc<ResourceManager>) -> Self {
        Self { info: HashMap::new(), resource_manager: Some(resource_manager) }
    }

    /// Sets or updates the resource manager for this registry.
    pub fn set_resource_manager(&mut self, resource_manager: Arc<ResourceManager>) {
        self.resource_manager = Some(resource_manager);
    }

    /// Registers a node with statically defined pins.
    /// This is the preferred method for nodes whose input/output pins do not change based on configuration.
    pub fn register_static<F>(
        &mut self,
        name: &str,
        factory: F,
        param_schema: serde_json::Value,
        pins: StaticPins,
        categories: Vec<String>,
        bidirectional: bool,
    ) where
        F: Fn(Option<&serde_json::Value>) -> Result<Box<dyn ProcessorNode>, StreamKitError>
            + Send
            + Sync
            + 'static,
    {
        self.info.insert(
            name.to_string(),
            NodeInfo {
                factory: Arc::new(factory),
                param_schema,
                static_pins: Some(pins),
                categories,
                bidirectional,
                description: None,
                resource_factory: None,
                resource_key_hasher: None,
            },
        );
    }

    /// Registers a node with statically defined pins and a description.
    #[allow(clippy::too_many_arguments)]
    pub fn register_static_with_description<F>(
        &mut self,
        name: &str,
        factory: F,
        param_schema: serde_json::Value,
        pins: StaticPins,
        categories: Vec<String>,
        bidirectional: bool,
        description: impl Into<String>,
    ) where
        F: Fn(Option<&serde_json::Value>) -> Result<Box<dyn ProcessorNode>, StreamKitError>
            + Send
            + Sync
            + 'static,
    {
        self.info.insert(
            name.to_string(),
            NodeInfo {
                factory: Arc::new(factory),
                param_schema,
                static_pins: Some(pins),
                categories,
                bidirectional,
                description: Some(description.into()),
                resource_factory: None,
                resource_key_hasher: None,
            },
        );
    }

    /// Registers a node with dynamically defined pins.
    /// The pin layout for these nodes is determined at instantiation time from their configuration.
    /// The factory for such a node MUST be able to produce a default instance when `params` is `None`.
    pub fn register_dynamic<F>(
        &mut self,
        name: &str,
        factory: F,
        param_schema: serde_json::Value,
        categories: Vec<String>,
        bidirectional: bool,
    ) where
        F: Fn(Option<&serde_json::Value>) -> Result<Box<dyn ProcessorNode>, StreamKitError>
            + Send
            + Sync
            + 'static,
    {
        self.info.insert(
            name.to_string(),
            NodeInfo {
                factory: Arc::new(factory),
                param_schema,
                static_pins: None,
                categories,
                bidirectional,
                description: None,
                resource_factory: None,
                resource_key_hasher: None,
            },
        );
    }

    /// Registers a node with dynamically defined pins and a description.
    pub fn register_dynamic_with_description<F>(
        &mut self,
        name: &str,
        factory: F,
        param_schema: serde_json::Value,
        categories: Vec<String>,
        bidirectional: bool,
        description: impl Into<String>,
    ) where
        F: Fn(Option<&serde_json::Value>) -> Result<Box<dyn ProcessorNode>, StreamKitError>
            + Send
            + Sync
            + 'static,
    {
        self.info.insert(
            name.to_string(),
            NodeInfo {
                factory: Arc::new(factory),
                param_schema,
                static_pins: None,
                categories,
                bidirectional,
                description: Some(description.into()),
                resource_factory: None,
                resource_key_hasher: None,
            },
        );
    }

    /// Registers a node with resource management support.
    /// This is for nodes that need shared resources like ML models.
    ///
    /// # Arguments
    ///
    /// * `name` - The unique name for this node type
    /// * `factory` - Factory function that creates node instances (receives params)
    /// * `resource_factory` - Async factory that creates/loads the shared resource
    /// * `resource_key_hasher` - Function that hashes params into a cache key
    /// * `param_schema` - JSON schema for parameter validation
    /// * `pins` - Static pin configuration
    /// * `categories` - UI categories for this node
    /// * `bidirectional` - Whether this node is bidirectional
    #[allow(clippy::too_many_arguments)]
    pub fn register_static_with_resource<F>(
        &mut self,
        name: &str,
        factory: F,
        resource_factory: AsyncResourceFactory,
        resource_key_hasher: ResourceKeyHasher,
        param_schema: serde_json::Value,
        pins: StaticPins,
        categories: Vec<String>,
        bidirectional: bool,
    ) where
        F: Fn(Option<&serde_json::Value>) -> Result<Box<dyn ProcessorNode>, StreamKitError>
            + Send
            + Sync
            + 'static,
    {
        self.info.insert(
            name.to_string(),
            NodeInfo {
                factory: Arc::new(factory),
                param_schema,
                static_pins: Some(pins),
                categories,
                bidirectional,
                description: None,
                resource_factory: Some(resource_factory),
                resource_key_hasher: Some(resource_key_hasher),
            },
        );
    }

    /// Registers a dynamic node with resource management support.
    #[allow(clippy::too_many_arguments)]
    pub fn register_dynamic_with_resource<F>(
        &mut self,
        name: &str,
        factory: F,
        resource_factory: AsyncResourceFactory,
        resource_key_hasher: ResourceKeyHasher,
        param_schema: serde_json::Value,
        categories: Vec<String>,
        bidirectional: bool,
    ) where
        F: Fn(Option<&serde_json::Value>) -> Result<Box<dyn ProcessorNode>, StreamKitError>
            + Send
            + Sync
            + 'static,
    {
        self.info.insert(
            name.to_string(),
            NodeInfo {
                factory: Arc::new(factory),
                param_schema,
                static_pins: None,
                categories,
                bidirectional,
                description: None,
                resource_factory: Some(resource_factory),
                resource_key_hasher: Some(resource_key_hasher),
            },
        );
    }

    /// Creates an instance of a node by its registered name, passing in its configuration.
    ///
    /// # Errors
    ///
    /// Returns `StreamKitError::Runtime` if the node type is not found in the registry,
    /// or if the node's factory function returns an error during construction.
    ///
    /// # Note
    ///
    /// This method does not support resource management. For nodes with resources,
    /// use `create_node_async` instead.
    pub fn create_node(
        &self,
        name: &str,
        params: Option<&serde_json::Value>,
    ) -> Result<Box<dyn ProcessorNode>, StreamKitError> {
        self.info.get(name).map_or_else(
            || Err(StreamKitError::Runtime(format!("Node type '{name}' not found in registry"))),
            |info| (info.factory)(params),
        )
    }

    /// Creates an instance of a node asynchronously, with resource management support.
    ///
    /// This method should be used for nodes that have resource factories registered.
    /// It will load or retrieve shared resources (like ML models) before creating the node instance.
    ///
    /// # Errors
    ///
    /// Returns `StreamKitError::Runtime` if the node type is not found in the registry,
    /// if resource initialization fails, or if the node's factory function returns an error.
    pub async fn create_node_async(
        &self,
        name: &str,
        params: Option<&serde_json::Value>,
    ) -> Result<Box<dyn ProcessorNode>, StreamKitError> {
        let info = self.info.get(name).ok_or_else(|| {
            StreamKitError::Runtime(format!("Node type '{name}' not found in registry"))
        })?;

        // If the node has a resource factory and we have a resource manager, use it
        if let (Some(resource_factory), Some(resource_key_hasher), Some(resource_manager)) =
            (&info.resource_factory, &info.resource_key_hasher, &self.resource_manager)
        {
            // Compute resource key hash from parameters
            let params_hash = resource_key_hasher(params);
            let resource_key = ResourceKey::new(name, params_hash);

            // Get or create the resource
            let params_owned = params.cloned();
            let rf = resource_factory.clone();
            let _resource = resource_manager
                .get_or_create(resource_key, || (rf)(params_owned))
                .await
                .map_err(|e| {
                    StreamKitError::Runtime(format!(
                        "Resource initialization failed for '{name}': {e}"
                    ))
                })?;

            tracing::debug!("Resource loaded for node '{}', calling factory", name);
        }

        // Call the node factory
        (info.factory)(params)
    }

    /// Returns a list of definitions for all registered nodes.
    pub fn definitions(&self) -> Vec<NodeDefinition> {
        let mut defs = Vec::new();
        for (kind, info) in &self.info {
            let (inputs, outputs) = match &info.static_pins {
                Some(pins) => (pins.inputs.clone(), pins.outputs.clone()),
                None => {
                    // For dynamic nodes, we must create a temporary instance to get pin info.
                    match (info.factory)(None) {
                        Ok(node_instance) => {
                            (node_instance.input_pins(), node_instance.output_pins())
                        },
                        Err(e) => {
                            tracing::error!(kind=%kind, error=%e, "Failed to create temporary node instance for dynamic node definition");
                            continue;
                        },
                    }
                },
            };

            defs.push(NodeDefinition {
                kind: kind.clone(),
                description: info.description.clone(),
                param_schema: info.param_schema.clone(),
                inputs,
                outputs,
                categories: info.categories.clone(),
                bidirectional: info.bidirectional,
            });
        }
        defs
    }

    /// Removes a node definition from the registry.
    /// Returns true if a definition with the provided name was present.
    pub fn unregister(&mut self, name: &str) -> bool {
        self.info.remove(name).is_some()
    }

    /// Checks whether a node definition exists in the registry.
    pub fn contains(&self, name: &str) -> bool {
        self.info.contains_key(name)
    }
}
