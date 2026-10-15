//! FastTick core library.
//!
//! This crate holds the data model for BehaviorTree.CPP v4 XML files,
//! the node models, validation, layout and the FastTick project file.
//! It has no GUI dependencies.
//!
//! The main modules:
//!
//! - [`xml`]: the lossless XML document.
//! - [`bt`]: the BT.CPP view of a document ([`bt::BtFile`]).
//! - [`models`]: node models, model files and the model registry.
//! - [`builtin`]: the built-in models of BT.CPP 4.9.0 and the child rules.
//! - [`project`]: the FastTick project and the project file.
//! - [`runtime`]: the tree of a running executor.

pub mod bt;
pub mod builtin;
pub mod edit;
mod fsio;
pub mod layout;
pub mod models;
pub mod project;
pub mod runtime;
pub mod search;
pub mod status;
pub mod validate;
pub mod xml;
