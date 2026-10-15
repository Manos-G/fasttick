//! Commands for trees: add, delete, rename, main tree, layout.

use crate::bt::names::validate_model_name;
use crate::models::is_model_section;
use crate::project::{FileId, LayoutState, Project, TreeRef};
use crate::xml::NodeId;

use super::editor::Editor;
use super::error::{CommandError, NameKind};
use super::transaction::Edit;

/// Checks a tree ID (`validateModelName` in `src/xml_parsing.cpp`).
pub(crate) fn check_tree_id(id: &str) -> Result<(), CommandError> {
    validate_model_name(id).map_err(|e| CommandError::name(NameKind::TreeId, id, e))
}

/// The `<SubTree ID="id">` elements of a file: SubTree nodes in the trees
/// and in the unconnected nodes, and SubTree models in the model sections.
fn subtree_references(project: &Project, file: FileId, id: &str) -> Vec<NodeId> {
    let Some(bt) = project.file(file) else {
        return Vec::new();
    };
    let doc = bt.doc();
    let is_ref = |n: NodeId| doc.name(n) == Some("SubTree") && doc.attribute(n, "ID") == Some(id);
    let mut out = Vec::new();
    let mut tops: Vec<NodeId> = bt.trees().iter().map(|t| t.element).collect();
    tops.extend(
        project
            .all_unconnected()
            .filter(|(f, _, _)| *f == file)
            .map(|(_, _, n)| n.node),
    );
    for top in tops {
        out.extend(doc.descendants(top).filter(|&n| is_ref(n)));
    }
    for &section in &bt.index().model_sections {
        out.extend(doc.element_children(section).filter(|&n| is_ref(n)));
    }
    out
}

impl Editor<'_> {
    /// The root element of a tree file.
    fn tree_file_root(&self, file: FileId) -> Result<NodeId, CommandError> {
        let bt = self
            .project()
            .file(file)
            .ok_or(CommandError::UnknownFile(file))?;
        let root = bt.index().root.ok_or(CommandError::NoRootElement)?;
        if is_model_section(bt.doc(), root) {
            return Err(CommandError::NotATreeFile);
        }
        Ok(root)
    }

    /// Checks that no tree in the project has this ID.
    pub(crate) fn check_new_tree_id(&self, id: &str) -> Result<(), CommandError> {
        check_tree_id(id)?;
        if self.project().find_trees(id).is_empty() {
            Ok(())
        } else {
            Err(CommandError::TreeExists(id.to_owned()))
        }
    }

    /// Adds an empty tree `<BehaviorTree ID="id"/>` to a file and returns
    /// the `<BehaviorTree>` element.
    ///
    /// The tree goes before the first `<TreeNodesModel>` of the file, or
    /// after the last element of the root element. The ID must be a valid
    /// tree ID, and no tree in the project can have it.
    pub fn add_tree(&mut self, file: FileId, id: &str) -> Result<NodeId, CommandError> {
        self.atomic("Add tree", |ed| {
            let root = ed.tree_file_root(file)?;
            ed.check_new_tree_id(id)?;
            let doc = ed.doc(file)?;
            let index = doc
                .element_children(root)
                .position(|e| is_model_section(doc, e))
                .unwrap_or_else(|| doc.element_children(root).count());
            let element = ed.alloc(file, |doc| {
                let e = doc.create_element("BehaviorTree")?;
                doc.set_attribute(e, "ID", id)?;
                Ok(e)
            })?;
            ed.xml(file, |doc| {
                Ok(((), doc.insert_element_pretty(root, index, element)?))
            })?;
            Ok(element)
        })
    }

    /// Deletes a tree of a file (the last tree with this ID, as BT.CPP
    /// uses it). The layout state and the unconnected nodes of the tree go
    /// too.
    ///
    /// The command does not change the SubTree nodes that refer to the tree
    /// or `main_tree_to_execute`; the validator reports them.
    pub fn delete_tree(&mut self, file: FileId, id: &str) -> Result<(), CommandError> {
        self.atomic("Delete tree", |ed| {
            let bt = ed
                .project()
                .file(file)
                .ok_or(CommandError::UnknownFile(file))?;
            let element = bt
                .tree(id)
                .map(|t| t.element)
                .ok_or_else(|| CommandError::TreeNotFound(id.to_owned()))?;
            ed.xml(file, |doc| Ok(((), doc.remove_element_pretty(element)?)))?;
            let still_there = ed
                .project()
                .file(file)
                .is_some_and(|bt| bt.tree(id).is_some());
            if !still_there {
                ed.forget_tree_state(file, id)?;
            }
            Ok(())
        })
    }

    /// Removes the layout state and the unconnected nodes of a tree.
    fn forget_tree_state(&mut self, file: FileId, id: &str) -> Result<(), CommandError> {
        if self.project().layout(file, id).is_some() {
            self.apply(Edit::Layout {
                file,
                tree: id.to_owned(),
                state: None,
            })?;
        }
        self.forget_unconnected_list(file, id)
    }

    /// Renames a tree of a file (the last tree with the ID `old`) and
    /// returns the number of references that changed.
    ///
    /// The command also changes all references that resolve to this tree
    /// (see [`Project::find_tree_from`]), in all files of the project:
    ///
    /// - `<SubTree ID="old">` nodes in the trees and in the unconnected
    ///   nodes;
    /// - `<SubTree ID="old">` models in the `<TreeNodesModel>` sections;
    /// - `main_tree_to_execute="old"`.
    ///
    /// The layout state and the unconnected nodes of the tree move to the
    /// new ID. The new ID must be a valid tree ID, and no tree in the
    /// project can have it.
    pub fn rename_tree(
        &mut self,
        file: FileId,
        old: &str,
        new: &str,
    ) -> Result<usize, CommandError> {
        self.atomic("Rename tree", |ed| {
            if old == new {
                return Ok(0);
            }
            let bt = ed
                .project()
                .file(file)
                .ok_or(CommandError::UnknownFile(file))?;
            let index = bt
                .index()
                .tree_index(old)
                .ok_or_else(|| CommandError::TreeNotFound(old.to_owned()))?;
            let element = bt.trees()[index].element;
            ed.check_new_tree_id(new)?;
            let target = TreeRef { file, index };

            // Find all references first: the look-up uses the old IDs.
            let project = ed.synced();
            let mut references: Vec<(FileId, NodeId)> = Vec::new();
            let mut mains: Vec<(FileId, NodeId)> = Vec::new();
            for f in project.file_ids() {
                if project.find_tree_from(f, old) != Some(target) {
                    continue;
                }
                references.extend(
                    subtree_references(project, f, old)
                        .into_iter()
                        .map(|n| (f, n)),
                );
                if let Some(other) = project.file(f)
                    && other.index().main_tree_to_execute.as_deref() == Some(old)
                    && let Some(root) = other.index().root
                {
                    mains.push((f, root));
                }
            }
            let count = references.len() + mains.len();

            ed.set_attribute_raw(file, element, "ID", new)?;
            for (f, node) in references {
                ed.set_attribute_raw(f, node, "ID", new)?;
            }
            for (f, root) in mains {
                ed.set_attribute_raw(f, root, "main_tree_to_execute", new)?;
            }

            // Move the layout state and the unconnected nodes.
            if let Some(state) = ed.project().layout(file, old).cloned() {
                ed.apply(Edit::Layout {
                    file,
                    tree: old.to_owned(),
                    state: None,
                })?;
                ed.apply(Edit::Layout {
                    file,
                    tree: new.to_owned(),
                    state: Some(Box::new(state)),
                })?;
            }
            let moved = ed.project().unconnected(file, old).to_vec();
            ed.forget_unconnected_list(file, old)?;
            for item in moved {
                ed.push_unconnected(file, new, item.node, item.position)?;
            }
            Ok(count)
        })
    }

    /// Removes all unconnected nodes of a tree from the list (the nodes
    /// stay in the document).
    fn forget_unconnected_list(&mut self, file: FileId, id: &str) -> Result<(), CommandError> {
        for index in (0..self.project().unconnected(file, id).len()).rev() {
            self.apply(Edit::RemoveUnconnected {
                file,
                tree: id.to_owned(),
                index,
            })?;
        }
        Ok(())
    }

    /// Sets an attribute without the checks of [`Editor::set_attribute`].
    pub(crate) fn set_attribute_raw(
        &mut self,
        file: FileId,
        node: NodeId,
        name: &str,
        value: &str,
    ) -> Result<(), CommandError> {
        if self.doc(file)?.attribute(node, name) == Some(value) {
            return Ok(());
        }
        self.xml(file, |doc| {
            let change = doc.set_attribute(node, name, value)?;
            Ok(((), vec![change.inverse()]))
        })
    }

    /// Sets (`Some`) or removes (`None`) the attribute
    /// `main_tree_to_execute` of the root element of a file.
    ///
    /// The tree must be in the file or in a file that it includes: BT.CPP
    /// looks for it only there (`XMLParser::instantiateTree`).
    pub fn set_main_tree(&mut self, file: FileId, id: Option<&str>) -> Result<(), CommandError> {
        self.atomic("Set main tree", |ed| {
            let root = ed.tree_file_root(file)?;
            match id {
                Some(id) => {
                    let project = ed.synced();
                    let found = project
                        .find_tree_from(file, id)
                        .is_some_and(|t| project.include_closure(file).contains(&t.file));
                    if !found {
                        return Err(CommandError::TreeNotFound(id.to_owned()));
                    }
                    ed.set_attribute_raw(file, root, "main_tree_to_execute", id)
                }
                None => ed.remove_attribute(file, root, "main_tree_to_execute"),
            }
        })
    }

    /// Sets (`Some`) or removes (`None`) the layout state of a tree (for
    /// example after the user moves nodes on the canvas).
    pub fn set_layout(
        &mut self,
        file: FileId,
        tree: &str,
        state: Option<LayoutState>,
    ) -> Result<(), CommandError> {
        self.atomic("Change layout", |ed| {
            if ed.project().file(file).is_none() {
                return Err(CommandError::UnknownFile(file));
            }
            if ed.project().layout(file, tree) == state.as_ref() {
                return Ok(());
            }
            ed.apply(Edit::Layout {
                file,
                tree: tree.to_owned(),
                state: state.map(Box::new),
            })
        })
    }
}
