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
    RevealXml {
        /// The view.
        view: ViewId,
        /// The file of the element.
        file: fasttick_core::project::FileId,
        /// The element.
        node: fasttick_core::xml::NodeId,
    },
}
