//! BT.CPP v4 view on top of [`crate::xml::Document`].
//!
//! This module reads the BT.CPP concepts of an XML file. It does not copy
//! the data: [`BtFile`] holds the lossless document and a small
//! [`BtIndex`] (trees, includes, model sections). [`NodeView`] reads one
//! node element.
//!
//! The rules follow the BT.CPP 4.9.0 source:
//!
//! - `src/xml_parsing.cpp`: `loadDocImpl` (root element, `<include>`,
//!   `<BehaviorTree>` IDs), `VerifyXML` (structure rules, nesting depth),
//!   `createNodeFromXML` (type ID, instance name, attribute classes, port
//!   remapping) and `recursivelyCreateSubtree` (SubTree remapping,
//!   `_autoremap`, `{=}`).
//! - `src/basic_types.cpp`: `IsAllowedPortName`, `IsReservedAttribute`,
//!   `findForbiddenChar`, `convertFromString<bool>` ([`names`]).
//! - `src/tree_node.cpp`: `isBlackboardPointer`, `getRemappedKey`
//!   ([`PortValue`]).
//! - `src/blackboard.cpp`: the `@` prefix for the root blackboard.
//!
//! # Example
//!
//! ```
//! use fasttick_core::bt::{BtFile, PortValue, PreCondition};
//!
//! let file = BtFile::parse(
//!     r#"<root BTCPP_format="4" main_tree_to_execute="Main">
//!   <BehaviorTree ID="Main">
//!     <Sequence>
//!       <SaySomething message="{greeting}" _skipIf="done"/>
//!       <SubTree ID="Sub" target="{@goal}" _autoremap="true"/>
//!     </Sequence>
//!   </BehaviorTree>
//! </root>"#,
//!     None,
//! )
//! .unwrap();
//! let main = file.index().main_tree().unwrap();
//! let sequence = file.node(main.root.unwrap()).unwrap();
//! let say = sequence.children().next().unwrap();
//! assert_eq!(say.type_id(), Some("SaySomething"));
//! assert_eq!(say.instance_name(), Some("SaySomething"));
//! assert_eq!(
//!     say.port_value("message"),
//!     Some(PortValue::BlackboardRef { key: "greeting", root_scope: false })
//! );
//! assert_eq!(say.pre_condition(PreCondition::SkipIf), Some("done"));
//!
//! let sub = sequence.children().nth(1).unwrap();
//! assert_eq!(sub.subtree_id(), Some("Sub"));
//! assert_eq!(sub.autoremap(), Some(Ok(true)));
//! assert_eq!(file.node_index_path(sub.id()), Some((0, vec![0, 1])));
//! ```

mod attr;
mod file;
mod index;
pub mod names;
mod node;
mod path;
mod port_value;

pub use attr::{AttributeClass, PostCondition, PreCondition, RuntimeMeta};
pub use file::{BtError, BtFile, NEW_FILE_TEXT};
pub use index::{BtIndex, IncludeEntry, TreeEntry};
pub use node::{NodeAttribute, NodeView};
pub use path::{
    MAX_NESTING_DEPTH, StructureHash, find_too_deep, format_index_path, node_at_index_path,
    node_index_path, parse_index_path, structure_hash,
};
pub use port_value::{PortValue, blackboard_pointer};
