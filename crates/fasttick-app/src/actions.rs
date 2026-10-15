//! Requests from panels and tabs to the app.
//!
//! Panels draw while the app is borrowed in parts. A panel that wants a
//! change outside its own data pushes an [`AppAction`]; the app runs the
//! actions after the frame's dock pass.

use std::path::PathBuf;

use fasttick_core::runtime::{InstanceIndex, Uid};

use crate::documents::{DocId, ViewId};
use crate::edit_ops::EditOp;
use crate::jobs::DialogPurpose;
use crate::shortcuts::Command;

/// A request to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum AppAction {
    /// Run a command (as from the menu).
    Command(Command),
    /// Open a file (tree, project or log, by extension).
    OpenPath(PathBuf),
    /// Close a document view.
    CloseView(ViewId),
    /// Show another tree of the document in a view.
    SetViewRoot(ViewId, InstanceIndex),
    /// Open a tree of a document in a new tab.
    OpenTreeInNewTab(DocId, InstanceIndex),
    /// Expand the path to a SubTree instance and focus its node.
    RevealInstance(ViewId, InstanceIndex),
    /// Show a node of a document: expand the SubTrees above it, select it
    /// and move the view to it.
    RevealNode(DocId, Uid),
    /// Connect to an executor.
    Connect {
        /// Host name or address.
        host: String,
        /// Server port (the notification port is the next one).
        port: u16,
    },
    /// Open a file dialog for a purpose (save a recording, export, ...).
    PickFile(DialogPurpose),
    /// Remove the recent-files list.
    ClearRecent,
    /// Change the edit document of a view (see [`crate::edit_ops`]).
    Edit(ViewId, EditOp),
    /// Select an element of the edit document of a view and show it.
    /// Asks for a name, then runs the edit that needs it.
    PromptName {
        /// The edit view.
        view: ViewId,
        /// What the name is for.
        purpose: NamePurpose,
    },
    RevealXml {
        /// The view.
        view: ViewId,
        /// The file of the element.
        file: fasttick_core::project::FileId,
        /// The element.
        node: fasttick_core::xml::NodeId,
    },
}

/// What a name prompt is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamePurpose {
    /// The ID of a new tree made from the selected nodes.
    CreateSubTree,
    /// The ID of a new empty tree.
    AddTree,
    /// The new ID of a tree.
    RenameTree {
        /// The old ID.
        old: String,
    },
}

impl NamePurpose {
    /// The title of the prompt.
    pub fn title(&self) -> String {
        match self {
            NamePurpose::CreateSubTree => "Create SubTree from selection".to_owned(),
            NamePurpose::AddTree => "Add tree".to_owned(),
            NamePurpose::RenameTree { old } => format!("Rename tree {old}"),
        }
    }

    /// The text of the OK button.
    pub fn ok_text(&self) -> &'static str {
        match self {
            NamePurpose::CreateSubTree => "Create",
            NamePurpose::AddTree => "Add",
            NamePurpose::RenameTree { .. } => "Rename",
        }
    }

    /// The edit operation for a name.
    pub fn op(&self, name: String) -> EditOp {
        match self {
            NamePurpose::CreateSubTree => EditOp::CreateSubTree { tree_id: name },
            NamePurpose::AddTree => EditOp::AddTree { id: name },
            NamePurpose::RenameTree { old } => EditOp::RenameTree {
                old: old.clone(),
                new: name,
            },
        }
    }
}
