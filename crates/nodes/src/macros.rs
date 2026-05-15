// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/// Registers a node with dynamic pins using its `factory()` method.
macro_rules! register_dynamic_node {
    ($registry:expr, $name:expr, $node_type:ty, $config_type:ty,
     [$($cat:expr),* $(,)?], $desc:expr $(,)?) => {
        #[allow(clippy::expect_used)] // JsonSchema-derived configs are infallible to serialize
        {
            let factory = <$node_type>::factory();
            $registry.register_dynamic_with_description(
                $name,
                move |params| (factory)(params),
                serde_json::to_value(schemars::schema_for!($config_type))
                    .expect(concat!(stringify!($config_type), " schema should serialize to JSON")),
                vec![$($cat.to_string()),*],
                false,
                $desc,
            );
        }
    };
}

/// Registers a node with static pins.
macro_rules! register_static_node {
    ($registry:expr, $name:expr, $factory:expr, $config_type:ty, $pins:expr,
     [$($cat:expr),* $(,)?], $desc:expr $(,)?) => {
        #[allow(clippy::expect_used)] // JsonSchema-derived configs are infallible to serialize
        {
            $registry.register_static_with_description(
                $name,
                $factory,
                serde_json::to_value(schemars::schema_for!($config_type))
                    .expect(concat!(stringify!($config_type), " schema should serialize to JSON")),
                $pins,
                vec![$($cat.to_string()),*],
                false,
                $desc,
            );
        }
    };
    ($registry:expr, $name:expr, $factory:expr, $config_type:ty, $pins:expr,
     [$($cat:expr),* $(,)?], bidirectional, $desc:expr $(,)?) => {
        #[allow(clippy::expect_used)] // JsonSchema-derived configs are infallible to serialize
        {
            $registry.register_static_with_description(
                $name,
                $factory,
                serde_json::to_value(schemars::schema_for!($config_type))
                    .expect(concat!(stringify!($config_type), " schema should serialize to JSON")),
                $pins,
                vec![$($cat.to_string()),*],
                true,
                $desc,
            );
        }
    };
}
