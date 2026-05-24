// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Global node constraints.
//!
//! Server-level configuration (resource limits, security policies, etc.) that
//! node factories can query at registration time.  Each node module opts in by
//! implementing [`NodeConstraint`] on its own config struct — no central
//! registry of types is required.
//!
//! # Example
//!
//! ```ignore
//! use streamkit_core::constraints::{GlobalNodeConstraints, NodeConstraint};
//!
//! #[derive(Debug, Clone)]
//! struct MyNodeLimits { max_size: u32 }
//!
//! impl NodeConstraint for MyNodeLimits {
//!     fn constraint_name() -> &'static str { "my_module::my_node" }
//! }
//!
//! let mut constraints = GlobalNodeConstraints::new();
//! constraints.insert(MyNodeLimits { max_size: 1024 });
//!
//! let limits = constraints.get::<MyNodeLimits>();
//! assert_eq!(limits.unwrap().max_size, 1024);
//! ```

use std::any::{Any, TypeId};
use std::collections::HashMap;

/// Marker trait for server-level node constraints.
///
/// Implement this on any configuration struct that should be available to node
/// factories at registration time.  The trait bound ensures that only
/// intentionally marked types can be stored in [`GlobalNodeConstraints`].
pub trait NodeConstraint: Any + Send + Sync {
    /// Human-readable name used in log messages when a constraint is
    /// inserted or queried.  Use a namespaced format matching the node
    /// module path (e.g. `"core::script"`, `"video::compositor"`,
    /// `"plugin::native::kokoro"`).
    fn constraint_name() -> &'static str;
}

/// Type-safe container for server-level node constraints.
///
/// Internally keyed by [`TypeId`] so each type implementing
/// [`NodeConstraint`] can have at most one entry.  Node registration
/// functions retrieve their config via the typed [`get`](Self::get) accessor.
#[derive(Default)]
pub struct GlobalNodeConstraints {
    inner: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl GlobalNodeConstraints {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces any existing value of the same type, returning the old one.
    pub fn insert<T: NodeConstraint>(&mut self, value: T) -> Option<T> {
        tracing::debug!(constraint = T::constraint_name(), "inserting node constraint");
        self.inner
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|prev| prev.downcast::<T>().ok())
            .map(|boxed| *boxed)
    }

    #[must_use]
    pub fn get<T: NodeConstraint>(&self) -> Option<&T> {
        self.inner.get(&TypeId::of::<T>()).and_then(|boxed| boxed.downcast_ref::<T>())
    }
}

impl std::fmt::Debug for GlobalNodeConstraints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlobalNodeConstraints").field("count", &self.inner.len()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct TestConstraintA {
        limit: u32,
    }

    impl NodeConstraint for TestConstraintA {
        fn constraint_name() -> &'static str {
            "test::constraint_a"
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    struct TestConstraintB {
        name: String,
    }

    impl NodeConstraint for TestConstraintB {
        fn constraint_name() -> &'static str {
            "test::constraint_b"
        }
    }

    #[test]
    fn insert_and_retrieve() {
        let mut c = GlobalNodeConstraints::new();
        c.insert(TestConstraintA { limit: 42 });
        assert_eq!(c.get::<TestConstraintA>().unwrap().limit, 42);
    }

    #[test]
    fn missing_returns_none() {
        let c = GlobalNodeConstraints::new();
        assert!(c.get::<TestConstraintA>().is_none());
    }

    #[test]
    fn distinct_types_are_independent() {
        let mut c = GlobalNodeConstraints::new();
        c.insert(TestConstraintA { limit: 1 });
        c.insert(TestConstraintB { name: "hello".into() });
        assert_eq!(c.get::<TestConstraintA>().unwrap().limit, 1);
        assert_eq!(c.get::<TestConstraintB>().unwrap().name, "hello");
    }

    #[test]
    fn insert_replaces_and_returns_old() {
        let mut c = GlobalNodeConstraints::new();
        let old = c.insert(TestConstraintA { limit: 1 });
        assert!(old.is_none());

        let old = c.insert(TestConstraintA { limit: 2 });
        assert_eq!(old.unwrap().limit, 1);
        assert_eq!(c.get::<TestConstraintA>().unwrap().limit, 2);
    }

    #[test]
    fn default_is_empty() {
        let c = GlobalNodeConstraints::default();
        assert!(c.get::<TestConstraintA>().is_none());
    }
}
