//! Edit operations on BT.CPP documents, with undo and redo.
//!
//! All changes to a project go through this module (see `docs/DESIGN.md`
//! section 2.5). The GUI does not change a document directly.
//!
//! # Layers
//!
//! - [`Edit`]: one invertible operation. Most edits are primitive XML
//!   changes ([`crate::xml::Change`]) of one file. Other edits change the
//!   layout state of a tree or the unconnected nodes of a project.
//! - [`Transaction`]: a list of edits with a label. It can change several
//!   files. [`Transaction::apply`] applies all edits or none, and returns
//!   the transaction that undoes them. After the edits, it builds the index
//!   of each changed file again and gives the file a new generation (for the
//!   dirty state).
//! - [`Editor`]: an editor session. Its commands (for example
//!   [`Editor::add_node`], [`Editor::move_node`],
//!   [`Editor::create_subtree_from_selection`]) check their input first and
//!   return a [`CommandError`] instead of producing broken XML. Each command
//!   is atomic: if it fails, it changes nothing. One session can run several
//!   commands; it becomes one undo step.
//! - [`History`]: the undo and redo stacks, with a maximum depth and the
//!   merge of quick, repeated edits ([`History::run_coalesced`]).
//!
//! Every command keeps the bytes of the rest of the document: new elements
//! get the indentation of their siblings
//! ([`crate::xml::Document::insert_element_pretty`]), removed elements take
//! their white space with them, and an undo gives the exact original
//! bytes. Node IDs stay valid: a removed node stays in the document arena as
//! a detached node, so a redo inserts the same node again.
//!
//! # Example
//!
//! ```
//! use fasttick_core::bt::BtFile;
//! use fasttick_core::edit::History;
//! use fasttick_core::project::Project;
//!
//! let text = "<root BTCPP_format=\"4\">\n    <BehaviorTree ID=\"Main\">\n        <Sequence>\n            <AlwaysSuccess/>\n        </Sequence>\n    </BehaviorTree>\n</root>\n";
//! let (mut project, file) = Project::with_file("demo", BtFile::parse(text, None).unwrap());
//! let sequence = project.file(file).unwrap().tree("Main").unwrap().root.unwrap();
//!
//! let mut history = History::new();
//! let sleep = history
//!     .run(&mut project, |ed| {
//!         let sleep = ed.add_node(file, sequence, 1, "Sleep", None)?;
//!         ed.set_port_value(file, sleep, "msec", Some("100"))?;
//!         Ok(sleep)
//!     })
//!     .unwrap();
//! assert_eq!(
//!     project.file(file).unwrap().to_xml_string(),
//!     "<root BTCPP_format=\"4\">\n    <BehaviorTree ID=\"Main\">\n        <Sequence>\n            <AlwaysSuccess/>\n            <Sleep msec=\"100\"/>\n        </Sequence>\n    </BehaviorTree>\n</root>\n"
//! );
//! assert!(project.file(file).unwrap().is_dirty());
//!
//! history.undo(&mut project).unwrap();
//! assert_eq!(project.file(file).unwrap().to_xml_string(), text);
//! assert!(!project.file(file).unwrap().is_dirty());
//!
//! // Redo inserts the same node again.
//! history.redo(&mut project).unwrap();
//! let doc = project.file(file).unwrap().doc();
//! assert_eq!(doc.attribute(sleep, "msec"), Some("100"));
//! ```

mod annotations;
mod clipboard;
mod editor;
mod error;
mod history;
mod model_edits;
mod nodes;
mod subtree;
mod transaction;
mod trees;
mod unconnected;

pub use annotations::{group_members, tree_annotations, tree_nodes};
pub use clipboard::{DUPLICATE_OFFSET, copy};
pub use editor::{
    Applied, Editor, NodePlace, StructurePolicy, node_place, parent_rule, run, top_most,
};
pub use error::{CommandError, NameKind};
pub use history::{Coalesce, CoalesceKey, DEFAULT_MAX_DEPTH, History, HistoryError};
pub use nodes::Condition;
pub use subtree::NewSubTree;
pub use transaction::{Edit, FileGeneration, Transaction};
