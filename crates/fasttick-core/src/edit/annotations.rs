//! Node notes, groups and canvas notes (see [`crate::bt::annotations`]).
//!
//! All annotations are XML attributes, so the commands are attribute
//! changes: they are part of the undo history, they make the file dirty and
//! they go into the saved file.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::bt::annotations::{
    GROUP_ATTRIBUTE, MIN_NOTE_SIZE, NOTE_ATTRIBUTE, TreeAnnotation, format_box, next_group_id,
    next_note_id, node_group, parse_color, tree_groups, tree_notes,
};
use crate::project::{FileId, Project};
use crate::xml::NodeId;

use super::editor::{Editor, node_place};
use super::error::CommandError;

/// The `<BehaviorTree>` element of a tree.
fn tree_element(project: &Project, file: FileId, tree: &str) -> Result<NodeId, CommandError> {
    let bt = project.file(file).ok_or(CommandError::UnknownFile(file))?;
    bt.tree(tree)
        .map(|entry| entry.element)
        .ok_or_else(|| CommandError::TreeNotFound(tree.to_owned()))
}

/// The nodes of a tree that can be group members, in document order: the
/// nodes below the `<BehaviorTree>` element, then the unconnected nodes of
/// the tree with their descendants.
pub fn tree_nodes(project: &Project, file: FileId, tree: &str) -> Vec<NodeId> {
    let Some(bt) = project.file(file) else {
        return Vec::new();
    };
    let Some(entry) = bt.tree(tree) else {
        return Vec::new();
    };
    let doc = bt.doc();
    let mut nodes: Vec<NodeId> = doc
        .descendants(entry.element)
        .skip(1)
        .filter(|n| doc.is_element(*n))
        .collect();
    for unconnected in project.unconnected(file, tree) {
        nodes.extend(
            doc.descendants(unconnected.node)
                .filter(|n| doc.is_element(*n)),
        );
    }
    nodes
}

/// The members of the groups of a tree, by group ID, in document order.
/// The map also has the IDs that nodes use without a definition.
pub fn group_members(project: &Project, file: FileId, tree: &str) -> BTreeMap<u32, Vec<NodeId>> {
    let mut members: BTreeMap<u32, Vec<NodeId>> = BTreeMap::new();
    let Some(doc) = project.file(file).map(|f| f.doc()) else {
        return members;
    };
    for node in tree_nodes(project, file, tree) {
        if let Some(group) = node_group(doc, node) {
            members.entry(group).or_default().push(node);
        }
    }
    members
}

impl Editor<'_> {
    /// Checks that `node` is a node of the tree `tree` (or of its
    /// unconnected area).
    fn check_in_tree(&self, file: FileId, tree: &str, node: NodeId) -> Result<(), CommandError> {
        let place = node_place(self.project(), file, node)?;
        if place.tree_id() == tree {
            Ok(())
        } else {
            Err(CommandError::NotInTree {
                node,
                tree: tree.to_owned(),
            })
        }
    }

    /// Checks that a tree defines a group.
    fn check_group(&self, file: FileId, tree: &str, id: u32) -> Result<NodeId, CommandError> {
        let element = tree_element(self.project(), file, tree)?;
        let doc = self.doc(file)?;
        if doc
            .attribute(element, &TreeAnnotation::GroupTitle(id).name())
            .is_none()
        {
            return Err(CommandError::UnknownGroup {
                tree: tree.to_owned(),
                id,
            });
        }
        Ok(element)
    }

    /// Checks that a tree defines a canvas note.
    fn check_note(&self, file: FileId, tree: &str, id: u32) -> Result<NodeId, CommandError> {
        let element = tree_element(self.project(), file, tree)?;
        let doc = self.doc(file)?;
        if doc
            .attribute(element, &TreeAnnotation::NoteText(id).name())
            .is_none()
        {
            return Err(CommandError::UnknownNote {
                tree: tree.to_owned(),
                id,
            });
        }
        Ok(element)
    }

    /// Sets (`Some`) or removes (`None`) an attribute. An empty text
    /// removes it too.
    fn set_or_remove(
        &mut self,
        file: FileId,
        node: NodeId,
        name: &str,
        value: Option<&str>,
    ) -> Result<(), CommandError> {
        match value.filter(|v| !v.is_empty()) {
            Some(value) => self.set_attribute(file, node, name, value),
            None => self.remove_attribute(file, node, name),
        }
    }

    /// Sets the note of a tree node (the `_note` attribute). `None` or a
    /// blank text removes the note.
    pub fn set_node_note(
        &mut self,
        file: FileId,
        node: NodeId,
        note: Option<&str>,
    ) -> Result<(), CommandError> {
        self.atomic("Edit note", |ed| {
            node_place(ed.project(), file, node)?;
            let note = note.filter(|n| !n.trim().is_empty());
            ed.set_or_remove(file, node, NOTE_ATTRIBUTE, note)
        })
    }

    /// Makes a new group of nodes of a tree, with a title. Nodes in another
    /// group move to the new group. Returns the group ID.
    pub fn create_group(
        &mut self,
        file: FileId,
        tree: &str,
        members: &[NodeId],
        title: &str,
    ) -> Result<u32, CommandError> {
        self.atomic("New group", |ed| {
            if members.is_empty() {
                return Err(CommandError::EmptyGroup);
            }
            for node in members {
                ed.check_in_tree(file, tree, *node)?;
            }
            let element = tree_element(ed.project(), file, tree)?;
            let used = group_members(ed.project(), file, tree).into_keys();
            let id = next_group_id(ed.doc(file)?, element, used);
            let title = if title.trim().is_empty() {
                "Group"
            } else {
                title
            };
            ed.set_attribute(file, element, &TreeAnnotation::GroupTitle(id).name(), title)?;
            let value = id.to_string();
            for node in members {
                ed.set_attribute(file, *node, GROUP_ATTRIBUTE, &value)?;
            }
            Ok(id)
        })
    }

    /// Sets the members of a group: the nodes in `members` join the group
    /// (also from another group), and the other members leave it. A group
    /// cannot be empty.
    pub fn set_group_members(
        &mut self,
        file: FileId,
        tree: &str,
        id: u32,
        members: &[NodeId],
    ) -> Result<(), CommandError> {
        self.atomic("Change group", |ed| {
            ed.check_group(file, tree, id)?;
            if members.is_empty() {
                return Err(CommandError::EmptyGroup);
            }
            for node in members {
                ed.check_in_tree(file, tree, *node)?;
            }
            let wanted: HashSet<NodeId> = members.iter().copied().collect();
            let old = group_members(ed.project(), file, tree)
                .remove(&id)
                .unwrap_or_default();
            for node in old.iter().filter(|n| !wanted.contains(n)) {
                ed.remove_attribute(file, *node, GROUP_ATTRIBUTE)?;
            }
            let value = id.to_string();
            for node in members {
                ed.set_attribute(file, *node, GROUP_ATTRIBUTE, &value)?;
            }
            Ok(())
        })
    }

    /// Removes nodes from their groups. A group without members goes away.
    pub fn remove_from_group(
        &mut self,
        file: FileId,
        nodes: &[NodeId],
    ) -> Result<(), CommandError> {
        self.atomic("Remove from group", |ed| {
            for node in nodes {
                node_place(ed.project(), file, *node)?;
                ed.remove_attribute(file, *node, GROUP_ATTRIBUTE)?;
            }
            ed.remove_empty_groups(file)
        })
    }

    /// Sets the title of a group. A blank title becomes "Group".
    pub fn set_group_title(
        &mut self,
        file: FileId,
        tree: &str,
        id: u32,
        title: &str,
    ) -> Result<(), CommandError> {
        self.atomic("Edit group", |ed| {
            let element = ed.check_group(file, tree, id)?;
            let title = if title.trim().is_empty() {
                "Group"
            } else {
                title
            };
            ed.set_attribute(file, element, &TreeAnnotation::GroupTitle(id).name(), title)
        })
    }

    /// Sets the description of a group. An empty text removes it.
    pub fn set_group_description(
        &mut self,
        file: FileId,
        tree: &str,
        id: u32,
        description: &str,
    ) -> Result<(), CommandError> {
        self.atomic("Edit group", |ed| {
            let element = ed.check_group(file, tree, id)?;
            let name = TreeAnnotation::GroupDescription(id).name();
            ed.set_or_remove(file, element, &name, Some(description))
        })
    }

    /// Sets the color of a group (a name of
    /// [`ANNOTATION_COLORS`](crate::bt::annotations::ANNOTATION_COLORS)),
    /// or removes it (`None`: the default color).
    pub fn set_group_color(
        &mut self,
        file: FileId,
        tree: &str,
        id: u32,
        color: Option<&str>,
    ) -> Result<(), CommandError> {
        self.atomic("Group color", |ed| {
            let element = ed.check_group(file, tree, id)?;
            let color = color.and_then(parse_color);
            let name = TreeAnnotation::GroupColor(id).name();
            ed.set_or_remove(file, element, &name, color.as_deref())
        })
    }

    /// Deletes a group: its definition and the `_group` attributes of its
    /// members. The nodes stay.
    pub fn delete_group(&mut self, file: FileId, tree: &str, id: u32) -> Result<(), CommandError> {
        self.atomic("Delete group", |ed| {
            let element = ed.check_group(file, tree, id)?;
            let members = group_members(ed.project(), file, tree)
                .remove(&id)
                .unwrap_or_default();
            for node in members {
                ed.remove_attribute(file, node, GROUP_ATTRIBUTE)?;
            }
            ed.remove_group_definition(file, element, id)
        })
    }

    fn remove_group_definition(
        &mut self,
        file: FileId,
        element: NodeId,
        id: u32,
    ) -> Result<(), CommandError> {
        for attribute in [
            TreeAnnotation::GroupTitle(id),
            TreeAnnotation::GroupDescription(id),
            TreeAnnotation::GroupColor(id),
        ] {
            self.remove_attribute(file, element, &attribute.name())?;
        }
        Ok(())
    }

    /// Removes the definitions of the groups without members, in all trees
    /// of a file (for example after a delete).
    pub fn remove_empty_groups(&mut self, file: FileId) -> Result<(), CommandError> {
        let trees: Vec<(String, NodeId)> = self
            .project()
            .file(file)
            .ok_or(CommandError::UnknownFile(file))?
            .index()
            .trees
            .iter()
            .map(|t| (t.id.clone(), t.element))
            .collect();
        for (tree, element) in trees {
            let members = group_members(self.project(), file, &tree);
            let empty: Vec<u32> = tree_groups(self.doc(file)?, element)
                .into_iter()
                .map(|g| g.id)
                .filter(|id| !members.contains_key(id))
                .collect();
            for id in empty {
                self.remove_group_definition(file, element, id)?;
            }
        }
        Ok(())
    }

    /// Keeps the groups tidy after nodes moved to a new place (a move, a
    /// paste, a duplicate). `roots` are the moved subtrees. For each group
    /// of a node in these subtrees:
    ///
    /// - If all members of the group are in the moved subtrees, the group
    ///   moved as a whole and stays.
    /// - Else the nodes of a moved subtree stay in the group only if the
    ///   new parent or a sibling next to the subtree (that did not move) is
    ///   in the group.
    /// - The nodes leave a group that the tree does not define.
    pub fn keep_groups_in_place(
        &mut self,
        file: FileId,
        roots: &[NodeId],
    ) -> Result<(), CommandError> {
        let mut by_tree: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
        for root in roots {
            let place = node_place(self.project(), file, *root)?;
            by_tree
                .entry(place.tree_id().to_owned())
                .or_default()
                .push(*root);
        }
        for (tree, roots) in by_tree {
            let element = tree_element(self.project(), file, &tree)?;
            let doc = self.doc(file)?;
            let defined: HashSet<u32> = tree_groups(doc, element).iter().map(|g| g.id).collect();
            let moved: Vec<Vec<NodeId>> = roots
                .iter()
                .map(|r| doc.descendants(*r).filter(|n| doc.is_element(*n)).collect())
                .collect();
            let moved_set: HashSet<NodeId> = moved.iter().flatten().copied().collect();
            let members = group_members(self.project(), file, &tree);
            // The groups that moved as a whole.
            let whole: BTreeSet<u32> = members
                .iter()
                .filter(|(_, nodes)| nodes.iter().all(|n| moved_set.contains(n)))
                .map(|(id, _)| *id)
                .collect();
            let mut removals: Vec<NodeId> = Vec::new();
            for (root, nodes) in roots.iter().zip(&moved) {
                let doc = self.doc(file)?;
                let tree_node = |n: NodeId| node_place(self.project(), file, n).is_ok();
                let neighbours = [
                    doc.parent(*root),
                    element_before(doc, *root),
                    element_after(doc, *root),
                ];
                let context: HashSet<u32> = neighbours
                    .into_iter()
                    .flatten()
                    .filter(|n| !moved_set.contains(n) && tree_node(*n))
                    .filter_map(|n| node_group(doc, n))
                    .collect();
                for node in nodes {
                    let Some(group) = node_group(doc, *node) else {
                        continue;
                    };
                    let keep = defined.contains(&group)
                        && (whole.contains(&group) || context.contains(&group));
                    if !keep {
                        removals.push(*node);
                    }
                }
            }
            for node in removals {
                self.remove_attribute(file, node, GROUP_ATTRIBUTE)?;
            }
        }
        self.remove_empty_groups(file)
    }

    /// Adds a canvas note to a tree. `rect` is left, top, width, height in
    /// canvas units. Returns the note ID.
    pub fn add_canvas_note(
        &mut self,
        file: FileId,
        tree: &str,
        rect: [f32; 4],
        text: &str,
    ) -> Result<u32, CommandError> {
        self.atomic("New note", |ed| {
            let rect = check_box(rect)?;
            let element = tree_element(ed.project(), file, tree)?;
            let id = next_note_id(ed.doc(file)?, element);
            ed.set_attribute(file, element, &TreeAnnotation::NoteText(id).name(), text)?;
            ed.set_attribute(
                file,
                element,
                &TreeAnnotation::NoteBox(id).name(),
                &format_box(rect),
            )?;
            Ok(id)
        })
    }

    /// Sets the text of a canvas note. The note stays when the text is
    /// empty.
    pub fn set_canvas_note_text(
        &mut self,
        file: FileId,
        tree: &str,
        id: u32,
        text: &str,
    ) -> Result<(), CommandError> {
        self.atomic("Edit note", |ed| {
            let element = ed.check_note(file, tree, id)?;
            ed.set_attribute(file, element, &TreeAnnotation::NoteText(id).name(), text)
        })
    }

    /// Moves or resizes a canvas note. The size is at least
    /// [`MIN_NOTE_SIZE`].
    pub fn set_canvas_note_box(
        &mut self,
        file: FileId,
        tree: &str,
        id: u32,
        rect: [f32; 4],
    ) -> Result<(), CommandError> {
        self.atomic("Move note", |ed| {
            let rect = check_box(rect)?;
            let element = ed.check_note(file, tree, id)?;
            ed.set_attribute(
                file,
                element,
                &TreeAnnotation::NoteBox(id).name(),
                &format_box(rect),
            )
        })
    }

    /// Sets the color of a canvas note, or removes it (`None`: the default
    /// color).
    pub fn set_canvas_note_color(
        &mut self,
        file: FileId,
        tree: &str,
        id: u32,
        color: Option<&str>,
    ) -> Result<(), CommandError> {
        self.atomic("Note color", |ed| {
            let element = ed.check_note(file, tree, id)?;
            let color = color.and_then(parse_color);
            let name = TreeAnnotation::NoteColor(id).name();
            ed.set_or_remove(file, element, &name, color.as_deref())
        })
    }

    /// Deletes a canvas note.
    pub fn delete_canvas_note(
        &mut self,
        file: FileId,
        tree: &str,
        id: u32,
    ) -> Result<(), CommandError> {
        self.atomic("Delete note", |ed| {
            let element = ed.check_note(file, tree, id)?;
            for attribute in [
                TreeAnnotation::NoteText(id),
                TreeAnnotation::NoteBox(id),
                TreeAnnotation::NoteColor(id),
            ] {
                ed.remove_attribute(file, element, &attribute.name())?;
            }
            Ok(())
        })
    }
}

/// The element before `node` among its siblings (text and comments are
/// skipped).
fn element_before(doc: &crate::xml::Document, node: NodeId) -> Option<NodeId> {
    let mut current = doc.previous_sibling(node);
    while let Some(n) = current {
        if doc.is_element(n) {
            return Some(n);
        }
        current = doc.previous_sibling(n);
    }
    None
}

/// The element after `node` among its siblings.
fn element_after(doc: &crate::xml::Document, node: NodeId) -> Option<NodeId> {
    let mut current = doc.next_sibling(node);
    while let Some(n) = current {
        if doc.is_element(n) {
            return Some(n);
        }
        current = doc.next_sibling(n);
    }
    None
}

/// Checks a note box and makes it at least [`MIN_NOTE_SIZE`].
fn check_box(rect: [f32; 4]) -> Result<[f32; 4], CommandError> {
    if rect.iter().any(|v| !v.is_finite()) {
        return Err(CommandError::InvalidPosition([rect[0], rect[1]]));
    }
    Ok([
        rect[0],
        rect[1],
        rect[2].max(MIN_NOTE_SIZE[0]),
        rect[3].max(MIN_NOTE_SIZE[1]),
    ])
}

/// The canvas notes and the groups of a tree, for callers that only have a
/// project.
pub fn tree_annotations(
    project: &Project,
    file: FileId,
    tree: &str,
) -> (
    Vec<crate::bt::annotations::GroupInfo>,
    Vec<crate::bt::annotations::CanvasNoteInfo>,
) {
    let Some(doc) = project.file(file).map(|f| f.doc()) else {
        return (Vec::new(), Vec::new());
    };
    match tree_element(project, file, tree) {
        Ok(element) => (tree_groups(doc, element), tree_notes(doc, element)),
        Err(_) => (Vec::new(), Vec::new()),
    }
}
