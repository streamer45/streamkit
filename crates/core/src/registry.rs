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

    /// Returns the definition for a single registered node kind, if it exists.
    ///
    /// More efficient than [`definitions()`](Self::definitions) when only one
    /// node type is needed, since it avoids iterating (and potentially
    /// instantiating) every registered node.
    pub fn get_definition(&self, kind: &str) -> Option<NodeDefinition> {
        let info = self.info.get(kind)?;

        let (inputs, outputs) = match &info.static_pins {
            Some(pins) => (pins.inputs.clone(), pins.outputs.clone()),
            None => match (info.factory)(None) {
                Ok(node_instance) => (node_instance.input_pins(), node_instance.output_pins()),
                Err(e) => {
                    tracing::error!(
                        kind = %kind,
                        error = %e,
                        "Failed to create temporary node instance for definition lookup"
                    );
                    return None;
                },
            },
        };

        Some(NodeDefinition {
            kind: kind.to_string(),
            description: info.description.clone(),
            param_schema: info.param_schema.clone(),
            inputs,
            outputs,
            categories: info.categories.clone(),
            bidirectional: info.bidirectional,
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::NodeContext;
    use crate::pins::{InputPin, OutputPin, PinCardinality};
    use crate::resource_manager::ResourcePolicy;
    use crate::types::PacketType;

    struct StubNode;

    #[crate::async_trait]
    impl ProcessorNode for StubNode {
        fn input_pins(&self) -> Vec<InputPin> {
            vec![InputPin {
                name: "in".into(),
                accepts_types: vec![PacketType::Any],
                cardinality: PinCardinality::One,
            }]
        }
        fn output_pins(&self) -> Vec<OutputPin> {
            vec![OutputPin {
                name: "out".into(),
                produces_type: PacketType::Text,
                cardinality: PinCardinality::One,
            }]
        }
        async fn run(self: Box<Self>, _ctx: NodeContext) -> Result<(), StreamKitError> {
            Ok(())
        }
    }

    fn stub_factory(
        _params: Option<&serde_json::Value>,
    ) -> Result<Box<dyn ProcessorNode>, StreamKitError> {
        Ok(Box::new(StubNode))
    }

    fn stub_pins() -> StaticPins {
        StaticPins {
            inputs: vec![InputPin {
                name: "in".into(),
                accepts_types: vec![PacketType::Any],
                cardinality: PinCardinality::One,
            }],
            outputs: vec![OutputPin {
                name: "out".into(),
                produces_type: PacketType::Text,
                cardinality: PinCardinality::One,
            }],
        }
    }

    #[test]
    fn new_registry_is_empty() {
        let reg = NodeRegistry::new();
        assert!(reg.definitions().is_empty());
    }

    #[test]
    fn register_static_and_list_definitions() {
        let mut reg = NodeRegistry::new();
        reg.register_static(
            "stub",
            stub_factory,
            serde_json::json!({}),
            stub_pins(),
            vec!["test".into()],
            false,
        );
        let defs = reg.definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].kind, "stub");
        assert!(!defs[0].bidirectional);
        assert_eq!(defs[0].categories, vec!["test"]);
        assert!(defs[0].description.is_none());
    }

    #[test]
    fn register_static_with_description() {
        let mut reg = NodeRegistry::new();
        reg.register_static_with_description(
            "described",
            stub_factory,
            serde_json::json!({}),
            stub_pins(),
            vec![],
            true,
            "A test node",
        );
        let def = reg.get_definition("described").unwrap();
        assert_eq!(def.description.as_deref(), Some("A test node"));
        assert!(def.bidirectional);
    }

    #[test]
    fn register_dynamic_and_list_definitions() {
        let mut reg = NodeRegistry::new();
        reg.register_dynamic(
            "dyn_stub",
            stub_factory,
            serde_json::json!({}),
            vec!["dynamic".into()],
            false,
        );
        let defs = reg.definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].kind, "dyn_stub");
        assert_eq!(defs[0].inputs.len(), 1);
        assert_eq!(defs[0].outputs.len(), 1);
    }

    #[test]
    fn register_dynamic_with_description() {
        let mut reg = NodeRegistry::new();
        reg.register_dynamic_with_description(
            "dyn_desc",
            stub_factory,
            serde_json::json!({}),
            vec![],
            false,
            "Dynamic described",
        );
        let def = reg.get_definition("dyn_desc").unwrap();
        assert_eq!(def.description.as_deref(), Some("Dynamic described"));
    }

    #[test]
    fn create_node_success() {
        let mut reg = NodeRegistry::new();
        reg.register_static(
            "stub",
            stub_factory,
            serde_json::json!({}),
            stub_pins(),
            vec![],
            false,
        );
        let node = reg.create_node("stub", None).unwrap();
        assert_eq!(node.input_pins().len(), 1);
        assert_eq!(node.output_pins().len(), 1);
    }

    #[test]
    fn create_node_unknown_kind() {
        let reg = NodeRegistry::new();
        let result = reg.create_node("nonexistent", None);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn create_node_factory_error() {
        let mut reg = NodeRegistry::new();
        reg.register_static(
            "fail",
            |_| Err(StreamKitError::Configuration("bad params".into())),
            serde_json::json!({}),
            stub_pins(),
            vec![],
            false,
        );
        let result = reg.create_node("fail", None);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("bad params"));
    }

    #[test]
    fn get_definition_existing() {
        let mut reg = NodeRegistry::new();
        reg.register_static(
            "stub",
            stub_factory,
            serde_json::json!({}),
            stub_pins(),
            vec!["cat".into()],
            false,
        );
        let def = reg.get_definition("stub").unwrap();
        assert_eq!(def.kind, "stub");
        assert_eq!(def.categories, vec!["cat"]);
    }

    #[test]
    fn get_definition_missing() {
        let reg = NodeRegistry::new();
        assert!(reg.get_definition("nope").is_none());
    }

    #[test]
    fn contains_and_unregister() {
        let mut reg = NodeRegistry::new();
        reg.register_static(
            "stub",
            stub_factory,
            serde_json::json!({}),
            stub_pins(),
            vec![],
            false,
        );
        assert!(reg.contains("stub"));
        assert!(!reg.contains("other"));

        assert!(reg.unregister("stub"));
        assert!(!reg.contains("stub"));
        assert!(!reg.unregister("stub"));
    }

    #[test]
    fn duplicate_registration_overwrites() {
        let mut reg = NodeRegistry::new();
        reg.register_static(
            "stub",
            stub_factory,
            serde_json::json!({}),
            stub_pins(),
            vec!["first".into()],
            false,
        );
        reg.register_static(
            "stub",
            stub_factory,
            serde_json::json!({}),
            stub_pins(),
            vec!["second".into()],
            true,
        );
        let defs = reg.definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].categories, vec!["second"]);
        assert!(defs[0].bidirectional);
    }

    #[test]
    fn node_definition_serialization_roundtrip() {
        let def = NodeDefinition {
            kind: "test".into(),
            description: Some("desc".into()),
            param_schema: serde_json::json!({"type": "object"}),
            inputs: vec![InputPin {
                name: "in".into(),
                accepts_types: vec![PacketType::Text],
                cardinality: PinCardinality::One,
            }],
            outputs: vec![],
            categories: vec!["audio".into(), "filters".into()],
            bidirectional: false,
        };
        let json = serde_json::to_string(&def).unwrap();
        let deserialized: NodeDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.kind, "test");
        assert_eq!(deserialized.description.as_deref(), Some("desc"));
        assert_eq!(deserialized.categories.len(), 2);
    }

    #[test]
    fn with_resource_manager() {
        let rm = Arc::new(ResourceManager::new(ResourcePolicy::default()));
        let reg = NodeRegistry::with_resource_manager(rm);
        assert!(reg.definitions().is_empty());
    }

    #[test]
    fn set_resource_manager_on_existing_registry() {
        let mut reg = NodeRegistry::new();
        reg.register_static(
            "plain",
            stub_factory,
            serde_json::json!({}),
            stub_pins(),
            vec![],
            false,
        );
        let rm = Arc::new(ResourceManager::new(ResourcePolicy::default()));
        reg.set_resource_manager(rm);
        reg.register_static_with_resource(
            "res_node",
            stub_factory,
            stub_resource_factory(),
            stub_key_hasher(),
            serde_json::json!({}),
            stub_pins(),
            vec![],
            false,
        );
        assert!(reg.contains("plain"));
        assert!(reg.contains("res_node"));
    }

    struct StubResource;
    impl crate::resource_manager::Resource for StubResource {
        fn size_bytes(&self) -> usize {
            64
        }
        fn resource_type(&self) -> &str {
            "test"
        }
    }

    fn stub_resource_factory() -> AsyncResourceFactory {
        Arc::new(|_params| {
            Box::pin(async {
                Ok(Arc::new(StubResource) as Arc<dyn crate::resource_manager::Resource>)
            })
        })
    }

    fn stub_key_hasher() -> crate::node::ResourceKeyHasher {
        Arc::new(|_params| "test_hash".to_string())
    }

    #[test]
    fn register_static_with_resource() {
        let rm = Arc::new(ResourceManager::new(ResourcePolicy::default()));
        let mut reg = NodeRegistry::with_resource_manager(rm);
        reg.register_static_with_resource(
            "res_node",
            stub_factory,
            stub_resource_factory(),
            stub_key_hasher(),
            serde_json::json!({}),
            stub_pins(),
            vec!["ml".into()],
            false,
        );
        assert!(reg.contains("res_node"));
        let def = reg.get_definition("res_node").unwrap();
        assert_eq!(def.categories, vec!["ml"]);
    }

    #[test]
    fn register_dynamic_with_resource() {
        let rm = Arc::new(ResourceManager::new(ResourcePolicy::default()));
        let mut reg = NodeRegistry::with_resource_manager(rm);
        reg.register_dynamic_with_resource(
            "dyn_res",
            stub_factory,
            stub_resource_factory(),
            stub_key_hasher(),
            serde_json::json!({}),
            vec!["ml".into()],
            false,
        );
        assert!(reg.contains("dyn_res"));
        let defs = reg.definitions();
        assert_eq!(defs.len(), 1);
    }

    #[tokio::test]
    async fn create_node_async_success() {
        let rm = Arc::new(ResourceManager::new(ResourcePolicy::default()));
        let mut reg = NodeRegistry::with_resource_manager(rm);
        reg.register_static_with_resource(
            "res_node",
            stub_factory,
            stub_resource_factory(),
            stub_key_hasher(),
            serde_json::json!({}),
            stub_pins(),
            vec![],
            false,
        );
        let node = reg.create_node_async("res_node", None).await.unwrap();
        assert_eq!(node.input_pins().len(), 1);
    }

    #[tokio::test]
    async fn create_node_async_unknown_kind() {
        let reg = NodeRegistry::new();
        let result = reg.create_node_async("missing", None).await;
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn create_node_async_without_resource_manager() {
        let mut reg = NodeRegistry::new();
        reg.register_static(
            "plain",
            stub_factory,
            serde_json::json!({}),
            stub_pins(),
            vec![],
            false,
        );
        let node = reg.create_node_async("plain", None).await.unwrap();
        assert_eq!(node.output_pins().len(), 1);
    }
}
