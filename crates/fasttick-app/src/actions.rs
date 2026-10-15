//! Requests from panels and tabs to the app.
//!
//! Panels draw while the app is borrowed in parts. A panel that wants a
//! change outside its own data pushes an [`AppAction`]; the app runs the
//! actions after the frame's dock pass.

use std::path::PathBuf;

use fasttick_core::runtime::InstanceIndex;

use crate::documents::{DocId, ViewId};
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
    /// Connect to an executor.
    Connect {
        /// Host name or address.
        host: String,
        /// Server port (the notification port is the next one).
        port: u16,
    },
    /// Set the replay position of a log document.
    SetReplayPosition(DocId, usize),
    /// Remove the recent-files list.
    ClearRecent,
}
