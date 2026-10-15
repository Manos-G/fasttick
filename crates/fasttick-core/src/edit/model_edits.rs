//! Commands for the node models in the `<TreeNodesModel>` of a file.

use crate::bt::names::validate_port_name;
use crate::models::{self, NodeModel};
use crate::project::FileId;

use super::editor::Editor;
use super::error::{CommandError, NameKind};
use super::nodes::check_type_id;

/// Checks the names of a model: the ID (`validateModelName`) and the port
/// names (`validatePortName`, `src/xml_parsing.cpp`), and that no two
/// ports have the same name.
fn check_model(model: &NodeModel) -> Result<(), CommandError> {
    check_type_id(&model.id)?;
    for (i, port) in model.ports.iter().enumerate() {
        validate_port_name(&port.name)
            .map_err(|e| CommandError::name(NameKind::PortName, &port.name, e))?;
        if model.ports[..i].iter().any(|p| p.name == port.name) {
            return Err(CommandError::DuplicatePort(port.name.clone()));
        }
    }
    Ok(())
}

impl Editor<'_> {
    /// Adds a model to the `<TreeNodesModel>` of a file (see
    /// [`models::add_model`]). The file gets a `<TreeNodesModel>` if it has
    /// none.
    pub fn add_model(&mut self, file: FileId, model: &NodeModel) -> Result<(), CommandError> {
        self.atomic("Add node model", |ed| {
            check_model(model)?;
            ed.xml(file, |doc| Ok(((), models::add_model(doc, model)?)))
        })
    }

    /// Replaces the model with the ID `id` in a file (see
    /// [`models::replace_model`]). The ID and the kind can change. The nodes
    /// that use the model do not change.
    pub fn replace_model(
        &mut self,
        file: FileId,
        id: &str,
        model: &NodeModel,
    ) -> Result<(), CommandError> {
        self.atomic("Change node model", |ed| {
            check_model(model)?;
            ed.xml(file, |doc| Ok(((), models::replace_model(doc, id, model)?)))
        })
    }

    /// Removes the model with this ID from a file (see
    /// [`models::remove_model`]). The nodes that use the model do not
    /// change.
    pub fn remove_model(&mut self, file: FileId, id: &str) -> Result<(), CommandError> {
        self.atomic("Remove node model", |ed| {
            ed.xml(file, |doc| Ok(((), models::remove_model(doc, id)?)))
        })
    }
}
