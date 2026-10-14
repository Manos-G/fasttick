//! Edits that keep the indentation of the document.

use super::document::Document;
use super::edit::Change;
use super::error::EditError;
use super::node::{Node, NodeId, Text};

impl Document {
    /// Returns true if the node is a text node with only white space.
    pub(crate) fn is_whitespace_text(&self, id: NodeId) -> bool {
        matches!(self.node(id), Some(Node::Text(t)) if t.is_whitespace())
    }

    fn raw_text(&self, id: NodeId) -> &str {
        match self.node(id) {
            Some(Node::Text(t)) => &t.raw,
            _ => "",
        }
    }

    fn new_whitespace(&mut self, raw: String) -> Result<NodeId, EditError> {
        self.alloc(Node::Text(Text { raw }))
    }

    /// Inserts the detached node `child` as element child number `index` of
    /// `parent` (`index` counts only the element children, from 0 to their
    /// number), and adds white space so that the output stays indented like
    /// the siblings.
    ///
    /// - The indentation is the indentation of the sibling element at the
    ///   insertion point, or the indentation of the parent plus one unit
    ///   ([`Document::indent_unit`]). New line ends use
    ///   [`Document::line_ending`].
    /// - Comments directly before an element stay with that element: the new
    ///   element goes before them.
    /// - Siblings on one line (white space without a line end) keep that
    ///   style. Siblings without white space between them get none.
    /// - The white space inside the subtree of `child` is changed to the new
    ///   depth.
    ///
    /// Returns the changes that undo the insertion, in the order in which to
    /// apply them (see [`Document::apply_all`]).
    pub fn insert_element_pretty(
        &mut self,
        parent: NodeId,
        index: usize,
        child: NodeId,
    ) -> Result<Vec<Change>, EditError> {
        self.check_insert(parent, child)?;
        if self.parent(child).is_some() {
            return Err(EditError::AlreadyAttached(child));
        }
        let kids = self.children(parent).to_vec();
        let elems: Vec<usize> = kids
            .iter()
            .enumerate()
            .filter(|&(_, &k)| self.is_element(k))
            .map(|(i, _)| i)
            .collect();
        if index > elems.len() {
            return Err(EditError::IndexOutOfRange {
                index,
                len: elems.len(),
            });
        }
        let nl = self.line_ending.as_str();
        let top_level = parent == self.root();
        let parent_indent = if top_level {
            String::new()
        } else {
            self.indentation(parent).unwrap_or("").to_owned()
        };
        let reference = elems.get(index).or(elems.last()).map(|&p| kids[p]);
        let child_indent = reference
            .and_then(|r| self.indentation(r))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if top_level {
                    String::new()
                } else {
                    format!("{parent_indent}{}", self.indent_unit)
                }
            });
        let mut plan = self.plan_reindent(child, &child_indent);
        let line = format!("{nl}{child_indent}");
        // The index of the last node that is not white space, plus one.
        let content_end = kids
            .iter()
            .rposition(|&k| !self.is_whitespace_text(k))
            .map_or(0, |i| i + 1);

        if let Some(&pos) = elems.get(index) {
            // Before an element: skip back over the comments and white space
            // that belong to it.
            let mut run = pos;
            while run > 0
                && (self.is_whitespace_text(kids[run - 1])
                    || matches!(self.node(kids[run - 1]), Some(Node::Comment(_))))
            {
                run -= 1;
            }
            let run_has_newline = kids[run..pos]
                .iter()
                .any(|&k| self.is_whitespace_text(k) && self.raw_text(k).contains('\n'));
            if run < pos && self.is_whitespace_text(kids[run]) {
                let lead = self.raw_text(kids[run]);
                let sep = if lead.contains('\n') {
                    line
                } else {
                    lead.to_owned()
                };
                let ws = self.new_whitespace(sep)?;
                plan.push(insert(parent, run + 1, child));
                plan.push(insert(parent, run + 2, ws));
            } else if run_has_newline {
                let ws = self.new_whitespace(line)?;
                plan.push(insert(parent, run, child));
                plan.push(insert(parent, run + 1, ws));
            } else {
                plan.push(insert(parent, run, child));
            }
        } else if let Some(&last) = elems.last() {
            // After the last element, before the closing white space.
            let lead = last
                .checked_sub(1)
                .map(|i| kids[i])
                .filter(|&k| self.is_whitespace_text(k));
            match lead {
                Some(lead) => {
                    let lead = self.raw_text(lead);
                    let sep = if lead.contains('\n') {
                        line
                    } else {
                        lead.to_owned()
                    };
                    let ws = self.new_whitespace(sep)?;
                    plan.push(insert(parent, content_end, ws));
                    plan.push(insert(parent, content_end + 1, child));
                }
                None => plan.push(insert(parent, content_end, child)),
            }
        } else if top_level {
            if content_end > 0 {
                let ws = self.new_whitespace(nl.to_owned())?;
                plan.push(insert(parent, content_end, ws));
                plan.push(insert(parent, content_end + 1, child));
            } else {
                plan.push(insert(parent, 0, child));
            }
        } else {
            // The parent has no element children.
            let open = self.new_whitespace(line)?;
            plan.push(insert(parent, content_end, open));
            plan.push(insert(parent, content_end + 1, child));
            if content_end == kids.len() {
                let close = self.new_whitespace(format!("{nl}{parent_indent}"))?;
                plan.push(insert(parent, content_end + 2, close));
            }
        }
        self.apply_all(plan)
    }

    /// Plans the changes that indent the white space inside the subtree of
    /// `node` for the indentation `base` of `node`.
    ///
    /// A white-space text node with a line end gets the indentation of the
    /// node after it: one unit more than its parent element, or the
    /// indentation of the parent when it is the last child (before the end
    /// tag). The number of line ends stays the same; their style becomes the
    /// line end of the document.
    fn plan_reindent(&self, node: NodeId, base: &str) -> Vec<Change> {
        let nl = self.line_ending.as_str();
        let mut plan = Vec::new();
        let mut stack = vec![(node, 0usize)];
        while let Some((element, depth)) = stack.pop() {
            let kids = self.children(element);
            for (i, &k) in kids.iter().enumerate() {
                match self.node(k) {
                    Some(Node::Element(_)) => stack.push((k, depth + 1)),
                    Some(Node::Text(t)) if t.is_whitespace() && t.raw.contains('\n') => {
                        let level = if i + 1 == kids.len() {
                            depth
                        } else {
                            depth + 1
                        };
                        let lines = t.raw.matches('\n').count();
                        let mut raw = nl.repeat(lines);
                        raw.push_str(base);
                        for _ in 0..level {
                            raw.push_str(&self.indent_unit);
                        }
                        if raw != t.raw {
                            plan.push(Change::SetRaw { node: k, raw });
                        }
                    }
                    _ => {}
                }
            }
        }
        plan
    }

    /// Removes an element (or a comment, CDATA section or processing
    /// instruction) with the white space that belongs to it, so that no
    /// blank line stays.
    ///
    /// The white space that belongs to the node is the white-space text node
    /// directly before it. If there is none, it is the white-space text node
    /// directly after it.
    ///
    /// Returns the changes that undo the removal, in the order in which to
    /// apply them (see [`Document::apply_all`]).
    pub fn remove_element_pretty(&mut self, node: NodeId) -> Result<Vec<Change>, EditError> {
        match self.node(node) {
            None => return Err(EditError::UnknownNode(node)),
            Some(Node::Document) => return Err(EditError::DocumentNode),
            Some(Node::Text(_)) => return Err(EditError::WrongKind(node)),
            Some(_) => {}
        }
        let parent = self.parent(node).ok_or(EditError::NotAttached(node))?;
        let pos = self
            .index_in_parent(node)
            .ok_or(EditError::NotAttached(node))?;
        let kids = self.children(parent);
        let before = pos
            .checked_sub(1)
            .map(|i| kids[i])
            .filter(|&k| self.is_whitespace_text(k));
        let after = kids
            .get(pos + 1)
            .copied()
            .filter(|&k| self.is_whitespace_text(k));
        let mut plan = vec![Change::Detach { node }];
        if let Some(ws) = before.or(after) {
            plan.push(Change::Detach { node: ws });
        }
        self.apply_all(plan)
    }
}

fn insert(parent: NodeId, index: usize, node: NodeId) -> Change {
    Change::Insert {
        parent,
        index,
        node,
    }
}
