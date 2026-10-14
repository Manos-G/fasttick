//! Editing functions.
//!
//! Each function checks its arguments first. If a check fails, the function
//! returns an [`EditError`] and the document does not change. Each function
//! returns the data that is necessary to undo it. The [`Change`] type
//! describes the primitive changes; [`Document::apply`] applies one change
//! and returns its inverse.

use super::document::Document;
use super::error::EditError;
use super::escape::{escape_text, is_valid_name};
use super::node::{Attribute, CData, Comment, Element, Node, NodeId, Quote, Slot, Text};

/// The place of a node in the children of its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    /// The parent node.
    pub parent: NodeId,
    /// The index in the children of the parent.
    pub index: usize,
}

/// One primitive change of a document.
///
/// [`Document::apply`] applies a change and returns the change that undoes
/// it. An undo/redo history can store these values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Insert the detached node `node` at `index` in the children of
    /// `parent`. The inverse is [`Change::Detach`].
    Insert {
        /// The new parent.
        parent: NodeId,
        /// The index in the children of the parent.
        index: usize,
        /// The detached node.
        node: NodeId,
    },
    /// Detach a node from its parent. The inverse is [`Change::Insert`].
    Detach {
        /// The node to detach.
        node: NodeId,
    },
    /// Move an attached node. `index` is the index in the children of
    /// `parent` after the node is removed from its old place. The inverse is
    /// a move back to the old place.
    Move {
        /// The node to move.
        node: NodeId,
        /// The new parent.
        parent: NodeId,
        /// The index in the new parent, after the removal.
        index: usize,
    },
    /// Insert an attribute at `index`. The inverse is
    /// [`Change::RemoveAttribute`].
    InsertAttribute {
        /// The element.
        node: NodeId,
        /// The attribute index.
        index: usize,
        /// The attribute.
        attribute: Attribute,
    },
    /// Remove the attribute at `index`. The inverse is
    /// [`Change::InsertAttribute`] with the old attribute.
    RemoveAttribute {
        /// The element.
        node: NodeId,
        /// The attribute index.
        index: usize,
    },
    /// Replace the attribute at `index`. The inverse puts the old attribute
    /// back.
    ReplaceAttribute {
        /// The element.
        node: NodeId,
        /// The attribute index.
        index: usize,
        /// The new attribute.
        attribute: Attribute,
    },
    /// Rename an element. The inverse puts the old name back.
    Rename {
        /// The element.
        node: NodeId,
        /// The new name.
        name: String,
    },
    /// Replace the source text of a text, CDATA or comment node. The
    /// inverse puts the old text back.
    SetRaw {
        /// The node.
        node: NodeId,
        /// The new source text (for CDATA and comments, with the markers).
        raw: String,
    },
}

/// The result of [`Document::set_attribute`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeChange {
    /// The element.
    pub node: NodeId,
    /// The index of the attribute.
    pub index: usize,
    /// The old attribute, or `None` if the attribute is new.
    pub old: Option<Attribute>,
}

impl AttributeChange {
    /// Returns the change that undoes the [`Document::set_attribute`] call.
    pub fn inverse(&self) -> Change {
        match &self.old {
            Some(old) => Change::ReplaceAttribute {
                node: self.node,
                index: self.index,
                attribute: old.clone(),
            },
            None => Change::RemoveAttribute {
                node: self.node,
                index: self.index,
            },
        }
    }
}

fn check_no_nul(text: &str) -> Result<(), EditError> {
    if text.contains('\0') {
        Err(EditError::InvalidContent(
            "XML cannot contain the character U+0000".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn check_name(name: &str) -> Result<(), EditError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(EditError::InvalidName(name.to_owned()))
    }
}

impl Document {
    /// Adds a detached node to the arena.
    pub(crate) fn alloc(&mut self, node: Node) -> Result<NodeId, EditError> {
        let id = u32::try_from(self.slots.len())
            .map(NodeId)
            .map_err(|_| EditError::TooManyNodes)?;
        self.slots.push(Slot::new(node, None));
        Ok(id)
    }

    fn check_node(&self, id: NodeId) -> Result<&Slot, EditError> {
        self.slot(id).ok_or(EditError::UnknownNode(id))
    }

    fn element_mut(&mut self, id: NodeId) -> Result<&mut Element, EditError> {
        match self.slot_mut(id).map(|s| &mut s.node) {
            Some(Node::Element(e)) => Ok(e),
            Some(_) => Err(EditError::NotAnElement(id)),
            None => Err(EditError::UnknownNode(id)),
        }
    }

    /// Makes a new detached element. It is written as `<name/>` while it
    /// has no children.
    pub fn create_element(&mut self, name: &str) -> Result<NodeId, EditError> {
        check_name(name)?;
        self.alloc(Node::Element(Element::new(name)))
    }

    /// Makes a new detached text node. The text is escaped; line ends use
    /// the line end of the document.
    pub fn create_text(&mut self, text: &str) -> Result<NodeId, EditError> {
        check_no_nul(text)?;
        let raw = escape_text(text, self.line_ending.as_str());
        self.alloc(Node::Text(Text { raw }))
    }

    /// Makes a new detached comment. The text cannot contain `--` or end
    /// with `-`.
    pub fn create_comment(&mut self, text: &str) -> Result<NodeId, EditError> {
        let raw = comment_raw(text)?;
        self.alloc(Node::Comment(Comment { raw }))
    }

    /// Makes a new detached CDATA section. The text cannot contain `]]>`.
    pub fn create_cdata(&mut self, text: &str) -> Result<NodeId, EditError> {
        let raw = cdata_raw(text)?;
        self.alloc(Node::CData(CData { raw }))
    }

    /// Checks that `child` can be inserted into `parent`.
    pub(crate) fn check_insert(&self, parent: NodeId, child: NodeId) -> Result<(), EditError> {
        let parent_slot = self.check_node(parent)?;
        let child_slot = self.check_node(child)?;
        if child == self.root() {
            return Err(EditError::DocumentNode);
        }
        match parent_slot.node {
            Node::Document => {}
            Node::Element(_) => {
                if matches!(child_slot.node, Node::Declaration(_)) {
                    return Err(EditError::WrongKind(child));
                }
            }
            _ => return Err(EditError::NotAContainer(parent)),
        }
        if parent == child || self.ancestors(parent).any(|a| a == child) {
            return Err(EditError::Cycle(child));
        }
        Ok(())
    }

    /// Inserts the detached node `child` at `index` in the children of
    /// `parent`. Undo with [`Document::detach`].
    pub fn insert_child(
        &mut self,
        parent: NodeId,
        index: usize,
        child: NodeId,
    ) -> Result<(), EditError> {
        self.check_insert(parent, child)?;
        if self.parent(child).is_some() {
            return Err(EditError::AlreadyAttached(child));
        }
        let len = self.children(parent).len();
        if index > len {
            return Err(EditError::IndexOutOfRange { index, len });
        }
        self.slots[parent.index()].children.insert(index, child);
        self.slots[child.index()].parent = Some(parent);
        Ok(())
    }

    /// Inserts the detached node `child` as the last child of `parent`.
    /// Returns the index of the child.
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) -> Result<usize, EditError> {
        let index = self.children(parent).len();
        self.insert_child(parent, index, child)?;
        Ok(index)
    }

    /// Detaches a node from its parent. The node and its subtree stay in
    /// the arena. Returns the old position; undo with
    /// [`Document::insert_child`].
    pub fn detach(&mut self, node: NodeId) -> Result<Position, EditError> {
        self.check_node(node)?;
        if node == self.root() {
            return Err(EditError::DocumentNode);
        }
        let parent = self.parent(node).ok_or(EditError::NotAttached(node))?;
        let index = self
            .index_in_parent(node)
            .ok_or(EditError::NotAttached(node))?;
        self.slots[parent.index()].children.remove(index);
        self.slots[node.index()].parent = None;
        Ok(Position { parent, index })
    }

    /// Removes a node from the tree. This is the same as
    /// [`Document::detach`].
    pub fn remove(&mut self, node: NodeId) -> Result<Position, EditError> {
        self.detach(node)
    }

    /// Moves an attached node to `index` in the children of `new_parent`.
    /// `index` is the index after the node is removed from its old place.
    /// Returns the old position; undo with a move back to it.
    pub fn move_node(
        &mut self,
        node: NodeId,
        new_parent: NodeId,
        index: usize,
    ) -> Result<Position, EditError> {
        self.check_insert(new_parent, node)?;
        let old_parent = self.parent(node).ok_or(EditError::NotAttached(node))?;
        let old_index = self
            .index_in_parent(node)
            .ok_or(EditError::NotAttached(node))?;
        let len = self.children(new_parent).len() - usize::from(new_parent == old_parent);
        if index > len {
            return Err(EditError::IndexOutOfRange { index, len });
        }
        self.slots[old_parent.index()].children.remove(old_index);
        self.slots[new_parent.index()].children.insert(index, node);
        self.slots[node.index()].parent = Some(new_parent);
        Ok(Position {
            parent: old_parent,
            index: old_index,
        })
    }

    /// Sets the value of an attribute.
    ///
    /// An existing attribute keeps its position, its quote character and
    /// its white space. A new attribute goes after the last attribute, with
    /// the white space and the quote of the last attribute (or one space and
    /// `"`). The value is escaped; see [`Attribute`].
    ///
    /// Undo with [`AttributeChange::inverse`].
    pub fn set_attribute(
        &mut self,
        node: NodeId,
        name: &str,
        value: &str,
    ) -> Result<AttributeChange, EditError> {
        check_name(name)?;
        check_no_nul(value)?;
        let element = self.element_mut(node)?;
        if let Some(index) = element.attribute_index(name) {
            let attribute = &mut element.attributes[index];
            let old = attribute.clone();
            if attribute.value != value {
                attribute.set_value(value);
                element.dirty = true;
            }
            return Ok(AttributeChange {
                node,
                index,
                old: Some(old),
            });
        }
        let (quote, leading) = match element.attributes.last() {
            Some(last) if !last.leading.is_empty() => (last.quote, last.leading.clone()),
            Some(last) => (last.quote, " ".to_owned()),
            None => (Quote::Double, " ".to_owned()),
        };
        element
            .attributes
            .push(Attribute::with_style(name, value, quote, &leading));
        element.dirty = true;
        Ok(AttributeChange {
            node,
            index: element.attributes.len() - 1,
            old: None,
        })
    }

    /// Removes an attribute. Returns its old index and the attribute, or
    /// `None` if the element has no attribute with this name. Undo with
    /// [`Document::insert_attribute`].
    pub fn remove_attribute(
        &mut self,
        node: NodeId,
        name: &str,
    ) -> Result<Option<(usize, Attribute)>, EditError> {
        let element = self.element_mut(node)?;
        let Some(index) = element.attribute_index(name) else {
            return Ok(None);
        };
        element.dirty = true;
        Ok(Some((index, element.attributes.remove(index))))
    }

    /// Removes the attribute at `index` and returns it.
    pub fn remove_attribute_at(
        &mut self,
        node: NodeId,
        index: usize,
    ) -> Result<Attribute, EditError> {
        let element = self.element_mut(node)?;
        let len = element.attributes.len();
        if index >= len {
            return Err(EditError::IndexOutOfRange { index, len });
        }
        element.dirty = true;
        Ok(element.attributes.remove(index))
    }

    /// Inserts an attribute at `index`. The element must not have an
    /// attribute with the same name.
    pub fn insert_attribute(
        &mut self,
        node: NodeId,
        index: usize,
        attribute: Attribute,
    ) -> Result<(), EditError> {
        check_name(&attribute.name)?;
        let element = self.element_mut(node)?;
        let len = element.attributes.len();
        if index > len {
            return Err(EditError::IndexOutOfRange { index, len });
        }
        if element.attribute_index(&attribute.name).is_some() {
            return Err(EditError::DuplicateAttribute(attribute.name));
        }
        element.attributes.insert(index, attribute);
        element.dirty = true;
        Ok(())
    }

    /// Replaces the attribute at `index` and returns the old attribute.
    pub fn replace_attribute(
        &mut self,
        node: NodeId,
        index: usize,
        attribute: Attribute,
    ) -> Result<Attribute, EditError> {
        check_name(&attribute.name)?;
        let element = self.element_mut(node)?;
        let len = element.attributes.len();
        if index >= len {
            return Err(EditError::IndexOutOfRange { index, len });
        }
        if let Some(other) = element.attribute_index(&attribute.name) {
            if other != index {
                return Err(EditError::DuplicateAttribute(attribute.name));
            }
        }
        element.dirty = true;
        Ok(std::mem::replace(&mut element.attributes[index], attribute))
    }

    /// Renames an element and returns the old name. The end tag changes
    /// too.
    pub fn rename_element(&mut self, node: NodeId, name: &str) -> Result<String, EditError> {
        check_name(name)?;
        let element = self.element_mut(node)?;
        if element.name == name {
            return Ok(name.to_owned());
        }
        element.dirty = true;
        Ok(std::mem::replace(&mut element.name, name.to_owned()))
    }

    /// Sets the content of a text, CDATA or comment node, and returns the
    /// old source text. Undo with [`Document::set_raw`].
    ///
    /// Text is escaped. CDATA cannot contain `]]>`. A comment cannot
    /// contain `--` or end with `-`.
    pub fn set_text(&mut self, node: NodeId, text: &str) -> Result<String, EditError> {
        let raw = match self.node(node) {
            Some(Node::Text(_)) => {
                check_no_nul(text)?;
                escape_text(text, self.line_ending.as_str())
            }
            Some(Node::CData(_)) => cdata_raw(text)?,
            Some(Node::Comment(_)) => comment_raw(text)?,
            Some(_) => return Err(EditError::WrongKind(node)),
            None => return Err(EditError::UnknownNode(node)),
        };
        self.set_raw(node, raw)
    }

    /// Sets the source text of a text, CDATA or comment node, and returns
    /// the old source text.
    ///
    /// For CDATA and comments, `raw` has the markers (`<![CDATA[` ... `]]>`,
    /// `<!--` ... `-->`). The text must give the same node when the
    /// document is parsed again.
    pub fn set_raw(&mut self, node: NodeId, raw: String) -> Result<String, EditError> {
        check_no_nul(&raw)?;
        let target = match self.slot_mut(node).map(|s| &mut s.node) {
            Some(Node::Text(t)) => {
                if raw.contains('<') {
                    return Err(EditError::InvalidContent(
                        "text cannot contain '<'".to_owned(),
                    ));
                }
                &mut t.raw
            }
            Some(Node::CData(c)) => {
                let ok = raw.len() >= 12
                    && raw.starts_with("<![CDATA[")
                    && raw.ends_with("]]>")
                    && !raw[9..raw.len() - 3].contains("]]>");
                if !ok {
                    return Err(EditError::InvalidContent(
                        "malformed CDATA section".to_owned(),
                    ));
                }
                &mut c.raw
            }
            Some(Node::Comment(c)) => {
                let ok = raw.len() >= 7
                    && raw.starts_with("<!--")
                    && raw.ends_with("-->")
                    && !raw[4..raw.len() - 3].contains("-->");
                if !ok {
                    return Err(EditError::InvalidContent("malformed comment".to_owned()));
                }
                &mut c.raw
            }
            Some(_) => return Err(EditError::WrongKind(node)),
            None => return Err(EditError::UnknownNode(node)),
        };
        Ok(std::mem::replace(target, raw))
    }

    /// Makes a deep copy of a subtree in this document. The copy is
    /// detached and keeps the source text of all nodes, so it is written
    /// with the same bytes.
    pub fn clone_subtree(&mut self, node: NodeId) -> Result<NodeId, EditError> {
        let base = self.slots.len();
        let copied = copy_slots(&self.slots, node, base)?;
        self.slots.extend(copied);
        Ok(NodeId(base as u32))
    }

    /// Makes a deep copy of a subtree of another document in this
    /// document. The copy is detached and keeps the source text of all
    /// nodes.
    pub fn import_subtree(&mut self, from: &Document, node: NodeId) -> Result<NodeId, EditError> {
        let base = self.slots.len();
        let copied = copy_slots(&from.slots, node, base)?;
        self.slots.extend(copied);
        Ok(NodeId(base as u32))
    }

    /// Applies one change and returns the change that undoes it.
    pub fn apply(&mut self, change: Change) -> Result<Change, EditError> {
        Ok(match change {
            Change::Insert {
                parent,
                index,
                node,
            } => {
                self.insert_child(parent, index, node)?;
                Change::Detach { node }
            }
            Change::Detach { node } => {
                let old = self.detach(node)?;
                Change::Insert {
                    parent: old.parent,
                    index: old.index,
                    node,
                }
            }
            Change::Move {
                node,
                parent,
                index,
            } => {
                let old = self.move_node(node, parent, index)?;
                Change::Move {
                    node,
                    parent: old.parent,
                    index: old.index,
                }
            }
            Change::InsertAttribute {
                node,
                index,
                attribute,
            } => {
                self.insert_attribute(node, index, attribute)?;
                Change::RemoveAttribute { node, index }
            }
            Change::RemoveAttribute { node, index } => {
                let attribute = self.remove_attribute_at(node, index)?;
                Change::InsertAttribute {
                    node,
                    index,
                    attribute,
                }
            }
            Change::ReplaceAttribute {
                node,
                index,
                attribute,
            } => {
                let attribute = self.replace_attribute(node, index, attribute)?;
                Change::ReplaceAttribute {
                    node,
                    index,
                    attribute,
                }
            }
            Change::Rename { node, name } => {
                let name = self.rename_element(node, &name)?;
                Change::Rename { node, name }
            }
            Change::SetRaw { node, raw } => {
                let raw = self.set_raw(node, raw)?;
                Change::SetRaw { node, raw }
            }
        })
    }

    /// Applies changes in order. Returns the changes that undo them, in the
    /// order in which to apply them.
    ///
    /// If a change fails, the function undoes the changes that it applied
    /// and returns the error. The document is then as before the call.
    pub fn apply_all(
        &mut self,
        changes: impl IntoIterator<Item = Change>,
    ) -> Result<Vec<Change>, EditError> {
        let mut inverses = Vec::new();
        for change in changes {
            match self.apply(change) {
                Ok(inverse) => inverses.push(inverse),
                Err(e) => {
                    for inverse in inverses.into_iter().rev() {
                        // An inverse of a change that just succeeded is valid.
                        let _ = self.apply(inverse);
                    }
                    return Err(e);
                }
            }
        }
        inverses.reverse();
        Ok(inverses)
    }
}

/// Makes the source text of a comment.
fn comment_raw(text: &str) -> Result<String, EditError> {
    check_no_nul(text)?;
    if text.contains("--") || text.ends_with('-') {
        return Err(EditError::InvalidContent(
            "a comment cannot contain \"--\" or end with \"-\"".to_owned(),
        ));
    }
    Ok(format!("<!--{text}-->"))
}

/// Makes the source text of a CDATA section.
fn cdata_raw(text: &str) -> Result<String, EditError> {
    check_no_nul(text)?;
    if text.contains("]]>") {
        return Err(EditError::InvalidContent(
            "a CDATA section cannot contain \"]]>\"".to_owned(),
        ));
    }
    Ok(format!("<![CDATA[{text}]]>"))
}

/// Copies the subtree of `node` from `src`. The copied slots get the
/// identifiers `base`, `base + 1`, ... in pre-order; the first slot is the
/// copy of `node`, and it is detached. Source offsets are not copied.
fn copy_slots(src: &[Slot], node: NodeId, base: usize) -> Result<Vec<Slot>, EditError> {
    let slot = src.get(node.index()).ok_or(EditError::UnknownNode(node))?;
    if matches!(slot.node, Node::Document) {
        return Err(EditError::DocumentNode);
    }
    let mut out: Vec<Slot> = Vec::new();
    let mut stack: Vec<(NodeId, Option<usize>)> = vec![(node, None)];
    while let Some((id, parent)) = stack.pop() {
        let Some(source) = src.get(id.index()) else {
            continue;
        };
        let local = out.len();
        let new_id = u32::try_from(base + local)
            .map(NodeId)
            .map_err(|_| EditError::TooManyNodes)?;
        let mut copy = Slot::new(source.node.clone(), None);
        if let Some(p) = parent {
            copy.parent = Some(NodeId((base + p) as u32));
            out[p].children.push(new_id);
        }
        out.push(copy);
        stack.extend(source.children.iter().rev().map(|&c| (c, Some(local))));
    }
    Ok(out)
}
