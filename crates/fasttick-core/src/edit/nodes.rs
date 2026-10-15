//! Commands for tree nodes: add, delete, move, attributes, type.

use std::fmt;

use crate::bt::names::{is_allowed_port_name, validate_instance_name, validate_model_name};
use crate::bt::{PostCondition, PreCondition};
use crate::builtin;
use crate::models::{NodeKind, NodeModel};
use crate::project::FileId;
use crate::xml::{Attribute, Change, Document, EditError, NodeId, is_valid_name};

use super::editor::{
    Editor, NodePlace, ParentPlace, element_index, node_place, parent_place, top_most,
};
use super::error::{CommandError, NameKind};
use super::transaction::Edit;

/// A pre-condition or a post-condition of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Condition {
    /// A pre-condition (`_failureIf`, `_successIf`, `_skipIf`, `_while`).
    Pre(PreCondition),
    /// A post-condition (`_onHalted`, `_onFailure`, `_onSuccess`, `_post`).
    Post(PostCondition),
}

impl Condition {
    /// The attribute name, for example `_skipIf`.
    pub fn attribute_name(self) -> &'static str {
        match self {
            Condition::Pre(c) => c.attribute_name(),
            Condition::Post(c) => c.attribute_name(),
        }
    }
}

impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.attribute_name())
    }
}

impl From<PreCondition> for Condition {
    fn from(c: PreCondition) -> Condition {
        Condition::Pre(c)
    }
}

impl From<PostCondition> for Condition {
    fn from(c: PostCondition) -> Condition {
        Condition::Post(c)
    }
}

/// The XML form of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `<TypeId .../>`.
    Compact,
    /// `<Action ID="TypeId" .../>` with this tag.
    Explicit(&'static str),
    /// `<SubTree ID="TreeId" .../>`.
    SubTree,
}

/// The tags that are not tree nodes: a clipboard text cannot contain them,
/// and a node type cannot use them in the compact form.
pub(crate) const STRUCTURE_TAGS: [&str; 4] = ["root", "BehaviorTree", "TreeNodesModel", "include"];

/// Checks a type ID (`validateModelName` in `src/xml_parsing.cpp`).
pub(crate) fn check_type_id(type_id: &str) -> Result<(), CommandError> {
    validate_model_name(type_id).map_err(|e| CommandError::name(NameKind::TypeId, type_id, e))
}

/// Chooses the form of a node of this type.
///
/// - A SubTree model gives `<SubTree ID="...">`; BT.CPP accepts a SubTree
///   only in this form (`createNodeFromXML`).
/// - Else the compact form `<TypeId>`, if the type ID is a valid XML name
///   and is not a category tag (`<Action>` would be the explicit form
///   without `ID`) or a structure tag.
/// - Else the explicit form `<Kind ID="TypeId">`, if the kind is known.
fn choose_form(type_id: &str, kind: Option<NodeKind>) -> Result<Form, CommandError> {
    if kind == Some(NodeKind::SubTree) {
        if type_id == "SubTree" {
            return Err(CommandError::SubTreeNeedsTarget);
        }
        return Ok(Form::SubTree);
    }
    if is_valid_name(type_id)
        && NodeKind::from_tag(type_id).is_none()
        && !STRUCTURE_TAGS.contains(&type_id)
    {
        return Ok(Form::Compact);
    }
    match kind {
        Some(kind) => Ok(Form::Explicit(kind.tag())),
        None => Err(CommandError::TypeNeedsKind(type_id.to_owned())),
    }
}

/// Makes a detached node element of this form.
fn build_node(doc: &mut Document, type_id: &str, form: Form) -> Result<NodeId, CommandError> {
    let element = match form {
        Form::Compact => doc.create_element(type_id)?,
        Form::Explicit(tag) => {
            let e = doc.create_element(tag)?;
            doc.set_attribute(e, "ID", type_id)?;
            e
        }
        Form::SubTree => {
            let e = doc.create_element("SubTree")?;
            doc.set_attribute(e, "ID", type_id)?;
            e
        }
    };
    Ok(element)
}

impl Editor<'_> {
    /// The kind of a type ID: from the model if it is given, else from the
    /// model registry (also an inferred SubTree model).
    pub(crate) fn kind_of(
        &mut self,
        file: FileId,
        type_id: &str,
        model: Option<&NodeModel>,
    ) -> Option<NodeKind> {
        match model {
            Some(m) => Some(m.kind),
            None => self
                .synced()
                .registry()
                .lookup(file, type_id)
                .map(|(m, _)| m.kind),
        }
    }

    /// Makes a detached node element for a new node of a type.
    pub(crate) fn new_node(
        &mut self,
        file: FileId,
        type_id: &str,
        model: Option<&NodeModel>,
    ) -> Result<NodeId, CommandError> {
        check_type_id(type_id)?;
        let kind = self.kind_of(file, type_id, model);
        let form = choose_form(type_id, kind)?;
        self.alloc(file, |doc| build_node(doc, type_id, form))
    }

    /// The number of element children of a node.
    pub(crate) fn element_count(&self, file: FileId, node: NodeId) -> usize {
        self.project()
            .file(file)
            .map_or(0, |bt| bt.doc().element_children(node).count())
    }

    /// Adds a new node of the type `type_id` as element child number
    /// `index` of `parent`, and returns the new node.
    ///
    /// `parent` is a tree node, an unconnected node, or a `<BehaviorTree>`
    /// element (then the node becomes the root node of the tree). The node
    /// uses the compact form `<TypeId/>` and gets the indentation of its
    /// siblings. A type whose model is a SubTree model gives
    /// `<SubTree ID="TypeId"/>`. A type ID that cannot be an element name
    /// gives the explicit form `<Kind ID="TypeId"/>`.
    ///
    /// `model` is the model of the type (for example from the palette); if
    /// it is `None`, the command looks it up in the project. The command
    /// does not add port attributes: the model defaults apply, and the
    /// validator reports missing required ports.
    pub fn add_node(
        &mut self,
        file: FileId,
        parent: NodeId,
        index: usize,
        type_id: &str,
        model: Option<&NodeModel>,
    ) -> Result<NodeId, CommandError> {
        self.atomic("Add node", |ed| {
            parent_place(ed.project(), file, parent)?;
            let count = ed.element_count(file, parent);
            check_index(index, count)?;
            ed.check_capacity(file, parent, count, 1)?;
            let node = ed.new_node(file, type_id, model)?;
            ed.xml(file, |doc| {
                Ok(((), doc.insert_element_pretty(parent, index, node)?))
            })?;
            Ok(node)
        })
    }

    /// Deletes nodes with their subtrees. A node whose ancestor is also in
    /// the selection is deleted with the ancestor. Unconnected nodes are
    /// removed from the unconnected area.
    pub fn delete_nodes(&mut self, file: FileId, nodes: &[NodeId]) -> Result<(), CommandError> {
        self.atomic("Delete", |ed| {
            if nodes.is_empty() {
                return Err(CommandError::EmptySelection);
            }
            for &node in nodes {
                node_place(ed.project(), file, node)?;
            }
            let doc = ed.doc(file)?;
            let tops = top_most(doc, nodes);
            for node in tops {
                match node_place(ed.project(), file, node)? {
                    NodePlace::Unconnected { root, tree_id } if root == node => {
                        let index = ed.unconnected_index(file, &tree_id, node)?;
                        ed.apply(Edit::RemoveUnconnected {
                            file,
                            tree: tree_id,
                            index,
                        })?;
                    }
                    _ => {
                        ed.xml(file, |doc| Ok(((), doc.remove_element_pretty(node)?)))?;
                    }
                }
            }
            Ok(())
        })
    }

    /// Moves a node (with its subtree) to element child number `index` of
    /// `new_parent`. This re-parents or re-orders a node. `index` counts the
    /// element children of `new_parent` after the node is removed from its
    /// old place.
    ///
    /// The command refuses to move a node into its own subtree. It also
    /// refuses a new parent that cannot take one more child (see
    /// [`StructurePolicy`](super::StructurePolicy)); a re-order in the same
    /// parent is always allowed. If the node is an unconnected node, the
    /// command connects it (see [`Editor::connect`]).
    pub fn move_node(
        &mut self,
        file: FileId,
        node: NodeId,
        new_parent: NodeId,
        index: usize,
    ) -> Result<(), CommandError> {
        if let NodePlace::Unconnected { root, .. } = node_place(self.project(), file, node)?
            && root == node
        {
            return self.connect(file, node, new_parent, index);
        }
        self.atomic("Move node", |ed| {
            parent_place(ed.project(), file, new_parent)?;
            let doc = ed.doc(file)?;
            if new_parent == node || doc.ancestors(new_parent).any(|a| a == node) {
                return Err(CommandError::IntoOwnSubtree(node));
            }
            let old_parent = doc.parent(node).ok_or(EditError::NotAttached(node))?;
            let same_parent = old_parent == new_parent;
            let count = ed.element_count(file, new_parent) - usize::from(same_parent);
            check_index(index, count)?;
            if same_parent {
                if element_index(ed.doc(file)?, node) == Some(index) {
                    return Ok(());
                }
            } else {
                ed.check_capacity(file, new_parent, count, 1)?;
            }
            ed.xml(file, |doc| move_pretty(doc, node, new_parent, index))
        })
    }

    /// Sets an attribute of an element. An existing attribute keeps its
    /// place and its quotes; a new attribute goes after the last one.
    ///
    /// The name must be a valid XML name. The value of `name` must be a
    /// valid instance name, and the value of `ID` a valid type ID or tree ID
    /// (BT.CPP rules). Setting the same value changes nothing.
    pub fn set_attribute(
        &mut self,
        file: FileId,
        node: NodeId,
        name: &str,
        value: &str,
    ) -> Result<(), CommandError> {
        self.atomic("Set attribute", |ed| {
            let view = ed.view(file, node)?;
            if !is_valid_name(name) {
                return Err(CommandError::InvalidAttributeName(name.to_owned()));
            }
            match name {
                "name" => validate_instance_name(value)
                    .map_err(|e| CommandError::name(NameKind::InstanceName, value, e))?,
                "ID" => {
                    let kind = if view.is_subtree() || view.tag() == "BehaviorTree" {
                        NameKind::TreeId
                    } else {
                        NameKind::TypeId
                    };
                    validate_model_name(value).map_err(|e| CommandError::name(kind, value, e))?;
                }
                _ => {}
            }
            if view.attribute(name) == Some(value) {
                return Ok(());
            }
            ed.xml(file, |doc| {
                let change = doc.set_attribute(node, name, value)?;
                Ok(((), vec![change.inverse()]))
            })
        })
    }

    /// Removes an attribute of an element. Nothing happens if the element
    /// does not have it.
    pub fn remove_attribute(
        &mut self,
        file: FileId,
        node: NodeId,
        name: &str,
    ) -> Result<(), CommandError> {
        self.atomic("Remove attribute", |ed| {
            let view = ed.view(file, node)?;
            if view.attribute(name).is_none() {
                return Ok(());
            }
            ed.xml(file, |doc| {
                let undo = match doc.remove_attribute(node, name)? {
                    Some((index, attribute)) => vec![Change::InsertAttribute {
                        node,
                        index,
                        attribute,
                    }],
                    None => Vec::new(),
                };
                Ok(((), undo))
            })
        })
    }

    /// Sets (`Some`) or removes (`None`) an attribute of a tree node.
    fn set_node_attribute(
        &mut self,
        file: FileId,
        node: NodeId,
        name: &str,
        value: Option<&str>,
    ) -> Result<(), CommandError> {
        node_place(self.project(), file, node)?;
        match value {
            Some(value) => self.set_attribute(file, node, name, value),
            None => self.remove_attribute(file, node, name),
        }
    }

    /// Sets the instance name of a tree node (the `name` attribute), or
    /// removes it (`None`; BT.CPP then uses the type ID as the name). The
    /// name must pass `validateInstanceName` (`src/xml_parsing.cpp`).
    pub fn set_instance_name(
        &mut self,
        file: FileId,
        node: NodeId,
        name: Option<&str>,
    ) -> Result<(), CommandError> {
        self.atomic("Rename node", |ed| {
            ed.set_node_attribute(file, node, "name", name)
        })
    }

    /// Sets (`Some`) or removes (`None`) the value of a port of a tree node
    /// (for a SubTree node: a remapping). The value is the text of the
    /// attribute, for example `42`, `{key}` or `{=}`.
    ///
    /// The port name must be a name that BT.CPP reads as a port
    /// (`IsAllowedPortName` in `src/basic_types.cpp`).
    pub fn set_port_value(
        &mut self,
        file: FileId,
        node: NodeId,
        port: &str,
        value: Option<&str>,
    ) -> Result<(), CommandError> {
        self.atomic("Set port", |ed| {
            if !is_allowed_port_name(port) {
                return Err(CommandError::NotAPortName(port.to_owned()));
            }
            ed.set_node_attribute(file, node, port, value)
        })
    }

    /// Sets (`Some`) or removes (`None`) the script of a pre-condition or a
    /// post-condition of a tree node.
    pub fn set_condition(
        &mut self,
        file: FileId,
        node: NodeId,
        condition: impl Into<Condition>,
        script: Option<&str>,
    ) -> Result<(), CommandError> {
        let condition = condition.into();
        self.atomic("Set condition", |ed| {
            ed.set_node_attribute(file, node, condition.attribute_name(), script)
        })
    }

    /// Changes the type of a tree node. The attributes and the children
    /// stay.
    ///
    /// - The compact form `<Old>` becomes `<New>`.
    /// - The explicit form `<Action ID="Old">` keeps its form: the `ID`
    ///   changes, and the tag becomes the kind of the new type if it is
    ///   known.
    /// - A new type with a SubTree model gives `<SubTree ID="New">`; a
    ///   SubTree node that gets another type loses its `ID` attribute.
    ///
    /// The command refuses a type that cannot have the current number of
    /// children (see [`StructurePolicy`](super::StructurePolicy)).
    pub fn change_node_type(
        &mut self,
        file: FileId,
        node: NodeId,
        type_id: &str,
        model: Option<&NodeModel>,
    ) -> Result<(), CommandError> {
        self.atomic("Change node type", |ed| {
            node_place(ed.project(), file, node)?;
            check_type_id(type_id)?;
            let kind = ed.kind_of(file, type_id, model);
            let view = ed.view(file, node)?;
            let form = if view.is_explicit_form() && kind != Some(NodeKind::SubTree) {
                Form::Explicit(kind.map_or_else(
                    || NodeKind::from_tag(view.tag()).map_or("Action", NodeKind::tag),
                    NodeKind::tag,
                ))
            } else {
                choose_form(type_id, kind)?
            };
            let count = view.child_count();
            if ed.structure_policy() == super::StructurePolicy::Enforce {
                let registration = if form == Form::SubTree {
                    "SubTree"
                } else {
                    type_id
                };
                if let Some(rule) = builtin::child_rule(registration, kind)
                    && rule.max().is_some_and(|max| count > max)
                {
                    return Err(CommandError::TooManyChildren {
                        type_id: type_id.to_owned(),
                        rule,
                        count,
                    });
                }
            }
            let (tag, id) = match form {
                Form::Compact => (type_id, None),
                Form::Explicit(tag) => (tag, Some(type_id)),
                Form::SubTree => ("SubTree", Some(type_id)),
            };
            let has_id = view.attribute("ID").map(str::to_owned);
            // Only the explicit form and the SubTree form use `ID`.
            let used_id = view.category_tag().is_some();
            let old_tag = view.tag().to_owned();
            if old_tag != tag {
                ed.xml(file, |doc| {
                    let old = doc.rename_element(node, tag)?;
                    Ok(((), vec![Change::Rename { node, name: old }]))
                })?;
            }
            match (id, has_id.as_deref()) {
                (Some(id), Some(old)) if old != id => {
                    ed.xml(file, |doc| {
                        let change = doc.set_attribute(node, "ID", id)?;
                        Ok(((), vec![change.inverse()]))
                    })?;
                }
                (Some(id), None) => {
                    // The ID goes first, as in `<Action ID="X" ...>`.
                    ed.apply_change(
                        file,
                        Change::InsertAttribute {
                            node,
                            index: 0,
                            attribute: Attribute::new("ID", id),
                        },
                    )?;
                }
                (None, Some(_)) if used_id => ed.remove_attribute(file, node, "ID")?,
                _ => {}
            }
            Ok(())
        })
    }

    /// The document of a file.
    pub(crate) fn doc(&self, file: FileId) -> Result<&Document, CommandError> {
        self.project()
            .file(file)
            .map(|bt| bt.doc())
            .ok_or(CommandError::UnknownFile(file))
    }

    /// The index of an unconnected node in the list of its tree.
    pub(crate) fn unconnected_index(
        &self,
        file: FileId,
        tree: &str,
        node: NodeId,
    ) -> Result<usize, CommandError> {
        self.project()
            .unconnected(file, tree)
            .iter()
            .position(|n| n.node == node)
            .ok_or(CommandError::NotUnconnected(node))
    }

    /// Checks that a parent for new children is a tree node or a tree
    /// element, and returns the number of its element children.
    pub(crate) fn insertion_point(
        &self,
        file: FileId,
        parent: NodeId,
        index: usize,
    ) -> Result<(ParentPlace, usize), CommandError> {
        let place = parent_place(self.project(), file, parent)?;
        let count = self.element_count(file, parent);
        check_index(index, count)?;
        Ok((place, count))
    }
}

/// Checks an element index for an insertion.
pub(crate) fn check_index(index: usize, count: usize) -> Result<(), CommandError> {
    if index > count {
        Err(EditError::IndexOutOfRange { index, len: count }.into())
    } else {
        Ok(())
    }
}

/// Moves an attached element with its white space: removes it (see
/// [`Document::remove_element_pretty`]) and inserts it at the new place
/// (see [`Document::insert_element_pretty`]). Returns the changes that undo
/// the move. On error, the document does not change.
pub(crate) fn move_pretty(
    doc: &mut Document,
    node: NodeId,
    new_parent: NodeId,
    index: usize,
) -> Result<((), Vec<Change>), CommandError> {
    let undo_remove = doc.remove_element_pretty(node)?;
    match doc.insert_element_pretty(new_parent, index, node) {
        Ok(mut undo) => {
            undo.extend(undo_remove);
            Ok(((), undo))
        }
        Err(error) => {
            // The inverse of a removal that just succeeded is valid.
            let _ = doc.apply_all(undo_remove);
            Err(error.into())
        }
    }
}
