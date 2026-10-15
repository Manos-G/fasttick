//! Create a SubTree from selected nodes.

use crate::bt::names::is_allowed_port_name;
use crate::bt::{AttributeClass, NodeView};
use crate::models::{NodeKind, NodeModel, PortDirection, PortModel};
use crate::project::FileId;
use crate::xml::{Document, NodeId};

use super::editor::{Editor, element_index, node_place, top_most};
use super::error::CommandError;
use super::nodes::move_pretty;

/// The built-in node types with a script in a port: `Script` (`code`),
/// `ScriptCondition` (`code`) and `Precondition` (`if`).
const SCRIPT_NODE_TYPES: [&str; 3] = ["Script", "ScriptCondition", "Precondition"];

/// The result of [`Editor::create_subtree_from_selection`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSubTree {
    /// The new `<BehaviorTree>` element.
    pub tree: NodeId,
    /// The new `<SubTree>` node at the old place of the selected nodes.
    pub subtree_node: NodeId,
    /// The `<Sequence>` that holds the selected nodes in the new tree, or
    /// `None` if one node was selected (it is then the root node of the
    /// new tree).
    pub sequence: Option<NodeId>,
    /// The blackboard keys that the SubTree node remaps, in the order of
    /// their first use.
    pub remapped_keys: Vec<String>,
    /// True if the SubTree node got `_autoremap="true"`, because a moved
    /// SubTree node uses `_autoremap="true"`: the keys of its tree are not
    /// known here.
    pub autoremap: bool,
    /// The moved nodes with a script: a pre-condition or a post-condition,
    /// or a `Script`, `ScriptCondition` or `Precondition` node. The command
    /// does not read scripts, so it does not remap their variables; check
    /// these nodes.
    pub script_nodes: Vec<NodeId>,
}

/// What the moved nodes use.
#[derive(Debug, Default)]
struct Usage {
    keys: Vec<String>,
    autoremap: bool,
    script_nodes: Vec<NodeId>,
}

/// Finds the blackboard keys that the subtrees of `tops` use in port
/// values (and SubTree remappings), in document order.
///
/// - `{key}` uses `key`; `{=}` (and `=` for a node that is not a SubTree)
///   uses the port name ([`crate::bt::PortValue::blackboard_key`]).
/// - `{@key}` uses the root blackboard. A SubTree cannot change that, so
///   such keys do not need a remapping (`Blackboard::getEntry` in
///   `src/blackboard.cpp`).
fn usage(doc: &Document, tops: &[NodeId]) -> Usage {
    let mut usage = Usage::default();
    for &top in tops {
        for node in doc.descendants(top) {
            let Some(view) = NodeView::new(doc, node) else {
                continue;
            };
            if view.is_subtree() && view.autoremap() == Some(Ok(true)) {
                usage.autoremap = true;
            }
            let mut script = view
                .type_id()
                .is_some_and(|t| SCRIPT_NODE_TYPES.contains(&t));
            for attribute in view.attributes() {
                match attribute.class {
                    AttributeClass::Port => {
                        let key = view
                            .port_value(attribute.name)
                            .and_then(|v| v.blackboard_key(attribute.name));
                        if let Some(key) = key
                            && !key.starts_with('@')
                            && !usage.keys.iter().any(|k| *k == key)
                        {
                            usage.keys.push(key.into_owned());
                        }
                    }
                    AttributeClass::PreCondition(_) | AttributeClass::PostCondition(_) => {
                        script = true;
                    }
                    _ => {}
                }
            }
            if script {
                usage.script_nodes.push(node);
            }
        }
    }
    usage
}

impl Editor<'_> {
    /// Moves selected sibling nodes into a new tree and puts a SubTree node
    /// at their place.
    ///
    /// - The selected nodes must have the same parent and be next to each
    ///   other (comments between them are allowed). A node whose ancestor is
    ///   also selected moves with the ancestor.
    /// - The new `<BehaviorTree ID="tree_id">` goes into the same file
    ///   (see [`Editor::add_tree`]). One node becomes its root node; several
    ///   nodes go into a new `<Sequence>`, in their order.
    /// - The new `<SubTree ID="tree_id">` node remaps each blackboard key
    ///   that the moved nodes use: `key="{key}"`. BT.CPP then connects the
    ///   key in the new tree to the same key in the old blackboard
    ///   (`recursivelyCreateSubtree` in `src/xml_parsing.cpp`). Keys of the
    ///   root blackboard (`{@key}`) need no remapping.
    /// - If keys are remapped, the file gets a `<SubTree ID="tree_id">`
    ///   model with one in/out port for each key.
    ///
    /// The command refuses keys that BT.CPP cannot remap: a remapping is an
    /// attribute, so the key must be an allowed port name
    /// (`IsAllowedPortName`). If a moved SubTree node uses `_autoremap`, the
    /// new SubTree node gets `_autoremap="true"` too, and only private keys
    /// (`_key`, which `_autoremap` skips) are refused.
    pub fn create_subtree_from_selection(
        &mut self,
        file: FileId,
        nodes: &[NodeId],
        tree_id: &str,
    ) -> Result<NewSubTree, CommandError> {
        self.atomic("Create SubTree", |ed| {
            if nodes.is_empty() {
                return Err(CommandError::EmptySelection);
            }
            for &node in nodes {
                node_place(ed.project(), file, node)?;
            }
            let doc = ed.doc(file)?;
            let tops = top_most(doc, nodes);
            let parent = doc.parent(tops[0]).ok_or(CommandError::NoParent(tops[0]))?;
            let mut positions = Vec::with_capacity(tops.len());
            for &node in &tops {
                if doc.parent(node) != Some(parent) {
                    return Err(CommandError::NotSiblings);
                }
                positions.push(element_index(doc, node).unwrap_or(0));
            }
            if positions.windows(2).any(|w| w[1] != w[0] + 1) {
                return Err(CommandError::NotContiguous);
            }
            let first_index = positions[0];
            let usage = usage(doc, &tops);
            let (mappable, unmappable): (Vec<String>, Vec<String>) = usage
                .keys
                .into_iter()
                .partition(|k| is_allowed_port_name(k));
            let refused: Vec<String> = if usage.autoremap {
                // `_autoremap` covers the other keys, but not private keys
                // (`IsPrivateKey` in `src/blackboard.cpp`).
                unmappable
                    .into_iter()
                    .filter(|k| k.starts_with('_'))
                    .collect()
            } else {
                unmappable
            };
            if !refused.is_empty() {
                return Err(CommandError::KeysNotRemappable(refused));
            }
            ed.check_new_tree_id(tree_id)?;
            if ed
                .synced()
                .registry()
                .lookup_declared(file, tree_id)
                .is_some()
            {
                return Err(CommandError::ModelExists(tree_id.to_owned()));
            }

            let tree = ed.add_tree(file, tree_id)?;
            let sequence = if tops.len() > 1 {
                let sequence = ed.alloc(file, |doc| Ok(doc.create_element("Sequence")?))?;
                ed.xml(file, |doc| {
                    Ok(((), doc.insert_element_pretty(tree, 0, sequence)?))
                })?;
                Some(sequence)
            } else {
                None
            };
            let container = sequence.unwrap_or(tree);
            for (i, &node) in tops.iter().enumerate() {
                ed.xml(file, |doc| move_pretty(doc, node, container, i))?;
            }
            let subtree_node = ed.alloc(file, |doc| {
                let e = doc.create_element("SubTree")?;
                doc.set_attribute(e, "ID", tree_id)?;
                for key in &mappable {
                    doc.set_attribute(e, key, &format!("{{{key}}}"))?;
                }
                if usage.autoremap {
                    doc.set_attribute(e, "_autoremap", "true")?;
                }
                Ok(e)
            })?;
            ed.xml(file, |doc| {
                Ok((
                    (),
                    doc.insert_element_pretty(parent, first_index, subtree_node)?,
                ))
            })?;
            if !mappable.is_empty() {
                let mut model = NodeModel::new(tree_id, NodeKind::SubTree);
                model.ports = mappable
                    .iter()
                    .map(|k| PortModel::new(k.clone(), PortDirection::InOut))
                    .collect();
                ed.add_model(file, &model)?;
            }
            Ok(NewSubTree {
                tree,
                subtree_node,
                sequence,
                remapped_keys: mappable,
                autoremap: usage.autoremap,
                script_nodes: usage.script_nodes,
            })
        })
    }
}
