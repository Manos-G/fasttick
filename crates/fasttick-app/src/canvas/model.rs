//! The canvas view model: [`CanvasTree`].
//!
//! A `CanvasTree` is the tree that the canvas draws. It has one
//! [`CanvasNode`] for each visible node: the instance name, the type ID, the
//! category, the children, a short port summary and the decorations (status,
//! breakpoint, substitution, problem, search match, selection).
//!
//! The canvas does not know where a tree comes from. Builders make a
//! `CanvasTree` from a source:
//!
//! - [`CanvasTree::from_runtime`]: the flat runtime XML of an executor or a
//!   log, or an authoring file that was read with
//!   [`RuntimeTree::parse`](fasttick_core::runtime::RuntimeTree::parse).
//! - Later: a tree of an editable `BtFile`. Use [`CanvasTreeBuilder`] and
//!   [`NodeSource::Xml`] for that.
//!
//! A `<SubTree>` node can be expanded inline (the root of its tree becomes
//! its only child) or collapsed. The builder takes the set of expanded
//! SubTree nodes ([`Expansion`]).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use fasttick_core::runtime::{InstanceIndex, NodeIndex, RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use fasttick_core::xml::NodeId;

use super::categories::CategoryLookup;

/// Index of a node in [`CanvasTree::nodes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanvasNodeId(pub u32);

impl CanvasNodeId {
    /// The index as `usize`.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// The category of a node. The canvas gives each category its own colour
/// band and glyph.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum NodeCategory {
    /// A leaf that does work.
    Action,
    /// A leaf that checks a condition.
    Condition,
    /// A node with one or more children.
    Control,
    /// A node with exactly one child.
    Decorator,
    /// A reference to another tree.
    SubTree,
    /// No model is known and the category cannot be inferred.
    Unknown,
}

impl NodeCategory {
    /// All categories, in display order.
    pub const ALL: [NodeCategory; 6] = [
        NodeCategory::Action,
        NodeCategory::Condition,
        NodeCategory::Control,
        NodeCategory::Decorator,
        NodeCategory::SubTree,
        NodeCategory::Unknown,
    ];

    /// The display name.
    pub fn name(self) -> &'static str {
        match self {
            NodeCategory::Action => "Action",
            NodeCategory::Condition => "Condition",
            NodeCategory::Control => "Control",
            NodeCategory::Decorator => "Decorator",
            NodeCategory::SubTree => "SubTree",
            NodeCategory::Unknown => "Unknown",
        }
    }

    /// The category for an explicit-form tag (`<Action ID="X">`).
    pub fn from_tag(tag: &str) -> Option<NodeCategory> {
        match tag {
            "Action" => Some(NodeCategory::Action),
            "Condition" => Some(NodeCategory::Condition),
            "Control" => Some(NodeCategory::Control),
            "Decorator" => Some(NodeCategory::Decorator),
            "SubTree" => Some(NodeCategory::SubTree),
            _ => None,
        }
    }
}

/// Where a canvas node comes from. The value is stable while the source
/// does not change, so it can carry the selection and the expansion state
/// over a rebuild of the canvas tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeSource {
    /// A node of a [`RuntimeTree`].
    Runtime(NodeIndex),
    /// An element of an `xml::Document` (edit mode).
    Xml(NodeId),
}

/// One line of the port summary: a port name and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortLine {
    /// The port (or remapping) name.
    pub name: String,
    /// The value as written in the XML.
    pub value: String,
}

/// SubTree data of a `<SubTree>` node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreeInfo {
    /// The ID of the tree that the node refers to.
    pub tree_id: String,
    /// True if the tree of the SubTree is shown inline as the child.
    pub expanded: bool,
    /// True if the target tree is known, so that the node can expand.
    pub can_expand: bool,
    /// The number of nodes in the target tree (all levels). Zero if the
    /// target is not known.
    pub inner_count: usize,
}

/// A breakpoint marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BreakpointMarker {
    /// The breakpoint is active.
    Enabled,
    /// The breakpoint exists but is not active.
    Disabled,
    /// The executor stopped at this breakpoint now.
    Hit,
}

/// A substitution marker: the executor does not tick the node, but returns
/// a fixed status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Substitution {
    /// The node returns SUCCESS.
    ForcedSuccess,
    /// The node returns FAILURE.
    ForcedFailure,
}

/// A validation problem marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProblemMarker {
    /// A warning.
    Warning,
    /// An error.
    Error,
}

/// The decorations of a node: everything that the canvas shows in addition
/// to the node itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Decorations {
    /// The status now.
    pub status: NodeStatus,
    /// The status before the node went back to IDLE. The canvas shows it
    /// faded while `status` is IDLE.
    pub previous_status: Option<NodeStatus>,
    /// A breakpoint on the node.
    pub breakpoint: Option<BreakpointMarker>,
    /// A substitution (fault injection) on the node.
    pub substitution: Option<Substitution>,
    /// The most severe problem of the node.
    pub problem: Option<ProblemMarker>,
    /// True if the node matches the current search.
    pub search_match: bool,
    /// True if the node is selected.
    pub selected: bool,
}

/// Display text and size of a node card. [`super::card::measure_tree`]
/// fills it in; the layout reads [`CardText::size`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CardText {
    /// The label, shortened to fit the card.
    pub label: String,
    /// The type line, if the label is not the type ID.
    pub type_line: Option<String>,
    /// The port lines that the card shows (name and value), shortened to
    /// fit.
    pub ports: Vec<(String, String)>,
    /// Text such as "+3 more", if not all ports fit.
    pub more: Option<String>,
    /// The card size in world units.
    pub size: egui::Vec2,
}

/// One node of a [`CanvasTree`].
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasNode {
    /// Where the node comes from.
    pub source: NodeSource,
    /// The UID in a running tree.
    pub uid: Option<Uid>,
    /// The instance name (the `name` attribute, else the type ID).
    pub label: String,
    /// The registration ID, for example `Sequence`.
    pub type_id: String,
    /// The category.
    pub category: NodeCategory,
    /// True if no model gave the category and the builder inferred it.
    pub category_inferred: bool,
    /// The parent node.
    pub parent: Option<CanvasNodeId>,
    /// The children, in order.
    pub children: Vec<CanvasNodeId>,
    /// The distance from the root (the root has depth 0).
    pub depth: u32,
    /// The ports and remappings, in document order.
    pub ports: Vec<PortLine>,
    /// SubTree data, for `<SubTree>` nodes.
    pub subtree: Option<SubtreeInfo>,
    /// The full path (for example `sub_a/fb`), if the source has one.
    pub path: String,
    /// The decorations.
    pub deco: Decorations,
    /// Display text and size. Empty until the canvas measures the tree.
    pub card: CardText,
}

impl CanvasNode {
    /// A node with no children and no decorations.
    pub fn new(source: NodeSource, label: impl Into<String>, type_id: impl Into<String>) -> Self {
        CanvasNode {
            source,
            uid: None,
            label: label.into(),
            type_id: type_id.into(),
            category: NodeCategory::Unknown,
            category_inferred: false,
            parent: None,
            children: Vec::new(),
            depth: 0,
            ports: Vec::new(),
            subtree: None,
            path: String::new(),
            deco: Decorations::default(),
            card: CardText::default(),
        }
    }

    /// True if the label is only the type ID (the node has no instance
    /// name).
    pub fn label_is_type(&self) -> bool {
        self.label == self.type_id
    }

    /// True for a collapsed SubTree node that can expand.
    pub fn is_collapsed_subtree(&self) -> bool {
        self.subtree
            .as_ref()
            .is_some_and(|info| info.can_expand && !info.expanded)
    }
}

/// The set of expanded SubTree nodes, by source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expansion {
    expanded: HashSet<NodeSource>,
}

impl Expansion {
    /// A set with no expanded nodes.
    pub fn none() -> Self {
        Expansion::default()
    }

    /// True if the node is expanded.
    pub fn is_expanded(&self, source: NodeSource) -> bool {
        self.expanded.contains(&source)
    }

    /// Expands or collapses a node.
    pub fn set(&mut self, source: NodeSource, expanded: bool) {
        if expanded {
            self.expanded.insert(source);
        } else {
            self.expanded.remove(&source);
        }
    }

    /// Changes the state of a node. Returns the new state.
    pub fn toggle(&mut self, source: NodeSource) -> bool {
        let expanded = !self.is_expanded(source);
        self.set(source, expanded);
        expanded
    }

    /// The number of expanded nodes.
    pub fn len(&self) -> usize {
        self.expanded.len()
    }

    /// True if no node is expanded.
    pub fn is_empty(&self) -> bool {
        self.expanded.is_empty()
    }
}

/// How SubTree nodes of a [`RuntimeTree`] find their tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtreeLinking {
    /// Each SubTree node has its own instance
    /// ([`RuntimeNode::expands_to`](fasttick_core::runtime::RuntimeNode::expands_to)).
    /// This is the form of the executor XML and of logs.
    ByInstance,
    /// A SubTree node refers to the `<BehaviorTree>` with the same `ID`.
    /// This is the form of an authoring file. Several SubTree nodes can
    /// use the same tree.
    ByTreeId,
}

impl SubtreeLinking {
    /// The linking that fits the tree: [`SubtreeLinking::ByInstance`] if
    /// the XML has `_uid` attributes (executor output), else
    /// [`SubtreeLinking::ByTreeId`].
    pub fn detect(tree: &RuntimeTree) -> SubtreeLinking {
        if tree.nodes.iter().any(|node| node.uid.is_some()) {
            SubtreeLinking::ByInstance
        } else {
            SubtreeLinking::ByTreeId
        }
    }
}

/// Above this number of nodes (with all SubTrees expanded), the default
/// expansion collapses the SubTrees.
pub const LARGE_TREE_NODES: usize = 400;

/// Options for [`CanvasTree::from_runtime`].
#[derive(Debug, Clone, Copy)]
pub struct RuntimeBuildOptions {
    /// The instance whose root is the root of the canvas tree.
    pub root_instance: InstanceIndex,
    /// How SubTree nodes find their tree.
    pub linking: SubtreeLinking,
}

impl RuntimeBuildOptions {
    /// Options for the main tree with the linking of
    /// [`SubtreeLinking::detect`].
    pub fn for_tree(tree: &RuntimeTree) -> Self {
        RuntimeBuildOptions {
            root_instance: InstanceIndex(0),
            linking: SubtreeLinking::detect(tree),
        }
    }
}

/// A builder for a [`CanvasTree`]. It keeps the nodes in depth-first
/// order. Add the root first, then each child after its parent.
#[derive(Debug, Default)]
pub struct CanvasTreeBuilder {
    nodes: Vec<CanvasNode>,
}

impl CanvasTreeBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        CanvasTreeBuilder::default()
    }

    /// Adds a node as the last child of `parent` (or as the root if
    /// `parent` is `None`). Returns the ID of the new node.
    ///
    /// A second root is ignored: the function then returns the ID of the
    /// new node, but [`CanvasTreeBuilder::finish`] keeps only the first
    /// root.
    pub fn push(&mut self, parent: Option<CanvasNodeId>, mut node: CanvasNode) -> CanvasNodeId {
        let id = CanvasNodeId(self.nodes.len() as u32);
        node.parent = parent.filter(|parent| parent.index() < self.nodes.len());
        node.children.clear();
        node.depth = match node.parent {
            Some(parent) => {
                let parent_node = &mut self.nodes[parent.index()];
                parent_node.children.push(id);
                parent_node.depth + 1
            }
            None => 0,
        };
        self.nodes.push(node);
        id
    }

    /// The number of nodes so far.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True if no node was added.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Makes the tree.
    pub fn finish(self, title: impl Into<String>) -> CanvasTree {
        CanvasTree::from_nodes(title.into(), self.nodes)
    }
}

/// The tree that the canvas draws. See the module documentation.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasTree {
    /// The tree ID of the root tree.
    pub title: String,
    nodes: Vec<CanvasNode>,
    by_uid: HashMap<Uid, CanvasNodeId>,
    by_source: HashMap<NodeSource, CanvasNodeId>,
}

impl Default for CanvasTree {
    fn default() -> Self {
        CanvasTree::from_nodes(String::new(), Vec::new())
    }
}

impl CanvasTree {
    fn from_nodes(title: String, nodes: Vec<CanvasNode>) -> CanvasTree {
        let mut by_uid = HashMap::with_capacity(nodes.len());
        let mut by_source = HashMap::with_capacity(nodes.len());
        for (index, node) in nodes.iter().enumerate() {
            let id = CanvasNodeId(index as u32);
            if let Some(uid) = node.uid {
                by_uid.entry(uid).or_insert(id);
            }
            by_source.entry(node.source).or_insert(id);
        }
        CanvasTree {
            title,
            nodes,
            by_uid,
            by_source,
        }
    }

    /// Builds the canvas tree of one tree of a [`RuntimeTree`].
    ///
    /// SubTree nodes in `expansion` show their tree inline. A SubTree that
    /// refers to one of its own ancestor trees (a cycle) does not expand.
    pub fn from_runtime(
        tree: &RuntimeTree,
        options: RuntimeBuildOptions,
        expansion: &Expansion,
        categories: &dyn CategoryLookup,
    ) -> CanvasTree {
        let instance = tree.instances.get(options.root_instance.0);
        let title = instance.map(|i| i.tree_id.clone()).unwrap_or_default();
        let Some(root) = instance.and_then(|i| i.root) else {
            return CanvasTree::from_nodes(title, Vec::new());
        };
        let linker = Linker::new(tree, options.linking);
        let mut builder = CanvasTreeBuilder::new();
        // Each stack item: the runtime node, the canvas parent, and the
        // tree IDs of the open SubTree instances on the path (for cycles).
        let mut ancestry: Vec<String> = vec![title.clone()];
        enum Step {
            Visit(NodeIndex, Option<CanvasNodeId>),
            Leave,
        }
        let mut stack = vec![Step::Visit(root, None)];
        while let Some(step) = stack.pop() {
            let (index, parent) = match step {
                Step::Visit(index, parent) => (index, parent),
                Step::Leave => {
                    ancestry.pop();
                    continue;
                }
            };
            let runtime_node = tree.node(index);
            let source = NodeSource::Runtime(index);
            let mut node = CanvasNode::new(source, &runtime_node.name, &runtime_node.type_id);
            node.uid = runtime_node.uid;
            node.path = tree.node_path(index);
            node.ports = runtime_node
                .port_attributes()
                .map(|(name, value)| PortLine {
                    name: name.to_owned(),
                    value: value.to_owned(),
                })
                .collect();

            let target = if runtime_node.is_subtree() {
                linker.target(index)
            } else {
                None
            };
            let target_id = target.map(|t| tree.instances[t.0].tree_id.clone());
            let cycle = target_id
                .as_ref()
                .is_some_and(|id| linker.linking == SubtreeLinking::ByTreeId && ancestry.contains(id));
            let target_root = target
                .filter(|_| !cycle)
                .and_then(|t| tree.instances[t.0].root);

            let (category, inferred) = categorize(runtime_node, categories);
            node.category = category;
            node.category_inferred = inferred;

            if runtime_node.is_subtree() {
                let tree_id = runtime_node
                    .attribute("ID")
                    .map(str::to_owned)
                    .or(target_id.clone())
                    .unwrap_or_default();
                let expanded = target_root.is_some() && expansion.is_expanded(source);
                node.subtree = Some(SubtreeInfo {
                    tree_id,
                    expanded,
                    can_expand: target_root.is_some(),
                    inner_count: target
                        .filter(|_| target_root.is_some())
                        .map(|t| linker.instance_size(t))
                        .unwrap_or(0),
                });
                let id = builder.push(parent, node);
                if expanded && let (Some(root), Some(target_id)) = (target_root, target_id) {
                    ancestry.push(target_id);
                    stack.push(Step::Leave);
                    stack.push(Step::Visit(root, Some(id)));
                }
            } else {
                let id = builder.push(parent, node);
                for child in runtime_node.children.iter().rev() {
                    stack.push(Step::Visit(*child, Some(id)));
                }
            }
        }
        builder.finish(title)
    }

    /// The default expansion for a tree: all SubTree nodes expand if the
    /// expanded tree has at most [`LARGE_TREE_NODES`] nodes; else none.
    pub fn default_expansion(tree: &RuntimeTree, options: RuntimeBuildOptions) -> Expansion {
        let mut all = Expansion::none();
        for (index, node) in tree.nodes.iter().enumerate() {
            if node.is_subtree() {
                all.set(NodeSource::Runtime(NodeIndex(index)), true);
            }
        }
        if all.is_empty() {
            return all;
        }
        let linker = Linker::new(tree, options.linking);
        let total = linker.instance_size(options.root_instance);
        if total <= LARGE_TREE_NODES {
            all
        } else {
            Expansion::none()
        }
    }

    /// All nodes, in depth-first order. The index of a node is its
    /// [`CanvasNodeId`].
    pub fn nodes(&self) -> &[CanvasNode] {
        &self.nodes
    }

    /// The number of nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True if the tree has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The root node.
    pub fn root(&self) -> Option<CanvasNodeId> {
        (!self.nodes.is_empty()).then_some(CanvasNodeId(0))
    }

    /// A node. Returns `None` for an ID that is not in this tree.
    pub fn get(&self, id: CanvasNodeId) -> Option<&CanvasNode> {
        self.nodes.get(id.index())
    }

    /// A node, for changes of the decorations or the card text.
    pub fn get_mut(&mut self, id: CanvasNodeId) -> Option<&mut CanvasNode> {
        self.nodes.get_mut(id.index())
    }

    /// All node IDs.
    pub fn ids(&self) -> impl Iterator<Item = CanvasNodeId> + use<> {
        (0..self.nodes.len() as u32).map(CanvasNodeId)
    }

    /// The node with this UID.
    pub fn find_uid(&self, uid: Uid) -> Option<CanvasNodeId> {
        self.by_uid.get(&uid).copied()
    }

    /// The node with this source.
    pub fn find_source(&self, source: NodeSource) -> Option<CanvasNodeId> {
        self.by_source.get(&source).copied()
    }

    /// Iterates over the nodes and their decorations, for bulk changes.
    pub fn nodes_mut(&mut self) -> impl Iterator<Item = &mut CanvasNode> {
        self.nodes.iter_mut()
    }

    /// The largest depth plus one (0 for an empty tree).
    pub fn level_count(&self) -> usize {
        self.nodes
            .iter()
            .map(|node| node.depth as usize + 1)
            .max()
            .unwrap_or(0)
    }

    // ----- Status -----

    /// Sets the status of the node with this UID. Returns true if a
    /// decoration changed.
    pub fn set_status(
        &mut self,
        uid: Uid,
        status: NodeStatus,
        previous: Option<NodeStatus>,
    ) -> bool {
        let Some(id) = self.find_uid(uid) else {
            return false;
        };
        let deco = &mut self.nodes[id.index()].deco;
        let previous = if status == NodeStatus::Idle {
            previous.filter(|p| *p != NodeStatus::Idle)
        } else {
            None
        };
        if deco.status == status && deco.previous_status == previous {
            return false;
        }
        deco.status = status;
        deco.previous_status = previous;
        true
    }

    /// Sets all nodes to IDLE with no previous status.
    pub fn reset_status(&mut self) {
        for node in &mut self.nodes {
            node.deco.status = NodeStatus::Idle;
            node.deco.previous_status = None;
        }
    }

    // ----- Selection -----

    /// The selected nodes, in tree order.
    pub fn selected(&self) -> impl Iterator<Item = CanvasNodeId> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.deco.selected)
            .map(|(index, _)| CanvasNodeId(index as u32))
    }

    /// The number of selected nodes.
    pub fn selected_count(&self) -> usize {
        self.nodes.iter().filter(|node| node.deco.selected).count()
    }

    /// True if the node is selected.
    pub fn is_selected(&self, id: CanvasNodeId) -> bool {
        self.get(id).is_some_and(|node| node.deco.selected)
    }

    /// Clears the selection. Returns true if it changed.
    pub fn clear_selection(&mut self) -> bool {
        let mut changed = false;
        for node in &mut self.nodes {
            changed |= std::mem::replace(&mut node.deco.selected, false);
        }
        changed
    }

    /// Selects or deselects one node. Returns true if it changed.
    pub fn set_selected(&mut self, id: CanvasNodeId, selected: bool) -> bool {
        match self.nodes.get_mut(id.index()) {
            Some(node) => std::mem::replace(&mut node.deco.selected, selected) != selected,
            None => false,
        }
    }

    /// Selects only this node. Returns true if the selection changed.
    pub fn select_only(&mut self, id: CanvasNodeId) -> bool {
        let mut changed = false;
        for (index, node) in self.nodes.iter_mut().enumerate() {
            let selected = index == id.index();
            changed |= std::mem::replace(&mut node.deco.selected, selected) != selected;
        }
        changed
    }

    /// Selects all nodes. Returns true if the selection changed.
    pub fn select_all(&mut self) -> bool {
        let mut changed = false;
        for node in &mut self.nodes {
            changed |= !std::mem::replace(&mut node.deco.selected, true);
        }
        changed
    }

    /// The sources of the selected nodes.
    pub fn selected_sources(&self) -> Vec<NodeSource> {
        self.selected()
            .map(|id| self.nodes[id.index()].source)
            .collect()
    }

    /// Selects the nodes with these sources (and no others).
    pub fn select_sources(&mut self, sources: &[NodeSource]) {
        self.clear_selection();
        for source in sources {
            if let Some(id) = self.find_source(*source) {
                self.nodes[id.index()].deco.selected = true;
            }
        }
    }

    /// Copies the decorations (status, markers, selection) of the nodes of
    /// `old` to the nodes of this tree with the same source. Use this after
    /// a rebuild (for example after a SubTree expands).
    pub fn copy_decorations_from(&mut self, old: &CanvasTree) {
        for node in &mut self.nodes {
            if let Some(old_id) = old.find_source(node.source) {
                node.deco = old.nodes[old_id.index()].deco;
            }
        }
    }

    // ----- Navigation -----

    /// The node in the direction `step` from `from`.
    pub fn navigate(&self, from: CanvasNodeId, step: NavStep) -> Option<CanvasNodeId> {
        let node = self.get(from)?;
        match step {
            NavStep::Parent => node.parent,
            NavStep::FirstChild => node.children.first().copied(),
            NavStep::PreviousSibling | NavStep::NextSibling => {
                let parent = self.get(node.parent?)?;
                let position = parent.children.iter().position(|c| *c == from)?;
                let next = match step {
                    NavStep::PreviousSibling => position.checked_sub(1)?,
                    _ => position + 1,
                };
                parent.children.get(next).copied()
            }
        }
    }

    /// The node IDs of the subtree of `id` (the node and all descendants),
    /// in depth-first order. Because the nodes are stored depth first, this
    /// is a contiguous range.
    pub fn subtree_range(&self, id: CanvasNodeId) -> std::ops::Range<usize> {
        let start = id.index();
        let Some(node) = self.get(id) else {
            return start..start;
        };
        let depth = node.depth;
        let end = self.nodes[start + 1..]
            .iter()
            .position(|n| n.depth <= depth)
            .map_or(self.nodes.len(), |offset| start + 1 + offset);
        start..end
    }
}

/// A keyboard navigation step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavStep {
    /// To the parent.
    Parent,
    /// To the first child.
    FirstChild,
    /// To the sibling before.
    PreviousSibling,
    /// To the sibling after.
    NextSibling,
}

/// Category of a runtime node and true if it was inferred.
fn categorize(
    node: &fasttick_core::runtime::RuntimeNode,
    categories: &dyn CategoryLookup,
) -> (NodeCategory, bool) {
    if node.is_subtree() {
        return (NodeCategory::SubTree, false);
    }
    // The explicit form names the category in the tag.
    if node.tag != node.type_id
        && let Some(category) = NodeCategory::from_tag(&node.tag)
    {
        return (category, false);
    }
    if let Some(category) = categories.category(&node.type_id) {
        return (category, false);
    }
    // BT.CPP allows children only for controls and decorators, so the
    // number of children gives a good guess.
    let guess = match node.children.len() {
        0 => NodeCategory::Action,
        1 => NodeCategory::Decorator,
        _ => NodeCategory::Control,
    };
    (guess, true)
}

/// Finds the tree of a SubTree node.
struct Linker<'a> {
    tree: &'a RuntimeTree,
    linking: SubtreeLinking,
    by_tree_id: HashMap<&'a str, InstanceIndex>,
    /// Memo of [`Linker::instance_size`].
    sizes: RefCell<HashMap<usize, usize>>,
}

/// The deepest SubTree nesting that [`Linker::instance_size`] follows.
const MAX_SUBTREE_NESTING: usize = 256;

impl<'a> Linker<'a> {
    fn new(tree: &'a RuntimeTree, linking: SubtreeLinking) -> Self {
        let mut by_tree_id = HashMap::new();
        if linking == SubtreeLinking::ByTreeId {
            for (index, instance) in tree.instances.iter().enumerate() {
                by_tree_id
                    .entry(instance.tree_id.as_str())
                    .or_insert(InstanceIndex(index));
            }
        }
        Linker {
            tree,
            linking,
            by_tree_id,
            sizes: RefCell::new(HashMap::new()),
        }
    }

    fn target(&self, index: NodeIndex) -> Option<InstanceIndex> {
        let node = self.tree.node(index);
        match self.linking {
            SubtreeLinking::ByInstance => node.expands_to,
            SubtreeLinking::ByTreeId => node
                .attribute("ID")
                .and_then(|id| self.by_tree_id.get(id).copied()),
        }
    }

    /// The number of nodes of an instance with all SubTrees expanded. A
    /// SubTree that is part of a cycle counts as one node. The result
    /// saturates at `usize::MAX`.
    fn instance_size(&self, instance: InstanceIndex) -> usize {
        let mut active = HashSet::new();
        self.instance_size_inner(instance, &mut active).0
    }

    /// The size and true if a cycle cut the count (then the size depends
    /// on the path and is not kept in the memo).
    fn instance_size_inner(
        &self,
        instance: InstanceIndex,
        active: &mut HashSet<usize>,
    ) -> (usize, bool) {
        if let Some(size) = self.sizes.borrow().get(&instance.0) {
            return (*size, false);
        }
        let Some(root) = self.tree.instances.get(instance.0).and_then(|i| i.root) else {
            return (0, false);
        };
        if active.len() >= MAX_SUBTREE_NESTING || !active.insert(instance.0) {
            return (0, true);
        }
        let mut count: usize = 0;
        let mut cut = false;
        let mut stack = vec![root];
        while let Some(index) = stack.pop() {
            count = count.saturating_add(1);
            let node = self.tree.node(index);
            if node.is_subtree() {
                if let Some(target) = self.target(index) {
                    let (inner, inner_cut) = self.instance_size_inner(target, active);
                    count = count.saturating_add(inner);
                    cut |= inner_cut;
                }
            } else {
                stack.extend(node.children.iter().copied());
            }
        }
        active.remove(&instance.0);
        if !cut {
            self.sizes.borrow_mut().insert(instance.0, count);
        }
        (count, cut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::categories::BuiltinCategories;

    const RUNTIME: &str = r#"<root BTCPP_format="4">
    <BehaviorTree ID="MainTree" _fullpath="">
        <Sequence name="root_sequence" _uid="1">
            <SaySomething name="hello" _uid="2" message="hello &amp; welcome"/>
            <SubTree ID="Sub" _fullpath="sub_a" _uid="3" target="{goal}"/>
            <Inverter _uid="4"><AlwaysFailure _uid="5"/></Inverter>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Sub" _fullpath="sub_a">
        <Fallback name="fb" _uid="6">
            <AlwaysFailure _uid="7"/>
            <Action ID="Talk" name="talk" _uid="8"/>
        </Fallback>
    </BehaviorTree>
</root>"#;

    const AUTHORING: &str = r#"<root BTCPP_format="4" main_tree_to_execute="Main">
    <BehaviorTree ID="Main">
        <Sequence>
            <SubTree ID="Shared" name="first"/>
            <SubTree ID="Shared" name="second"/>
            <SubTree ID="Missing"/>
            <SubTree ID="Main" name="loop"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Shared">
        <AlwaysSuccess/>
    </BehaviorTree>
</root>"#;

    fn build(xml: &str, expand_all: bool) -> CanvasTree {
        let tree = RuntimeTree::parse(xml).unwrap();
        let options = RuntimeBuildOptions::for_tree(&tree);
        let expansion = if expand_all {
            let mut e = Expansion::none();
            for (i, n) in tree.nodes.iter().enumerate() {
                if n.is_subtree() {
                    e.set(NodeSource::Runtime(NodeIndex(i)), true);
                }
            }
            e
        } else {
            Expansion::none()
        };
        CanvasTree::from_runtime(&tree, options, &expansion, &BuiltinCategories)
    }

    #[test]
    fn builds_collapsed_runtime_tree() {
        let canvas = build(RUNTIME, false);
        assert_eq!(canvas.title, "MainTree");
        let labels: Vec<&str> = canvas.nodes().iter().map(|n| n.label.as_str()).collect();
        assert_eq!(
            labels,
            ["root_sequence", "hello", "sub_a", "Inverter", "AlwaysFailure"]
        );
        let root = &canvas.nodes()[0];
        assert_eq!(root.category, NodeCategory::Control);
        assert_eq!(root.children.len(), 3);
        assert_eq!(root.depth, 0);

        let hello = &canvas.nodes()[1];
        assert_eq!(hello.type_id, "SaySomething");
        assert_eq!(hello.category, NodeCategory::Action);
        assert!(hello.category_inferred);
        assert_eq!(
            hello.ports,
            vec![PortLine {
                name: "message".into(),
                value: "hello & welcome".into()
            }]
        );
        assert_eq!(hello.uid, Some(Uid(2)));

        let sub = &canvas.nodes()[2];
        assert_eq!(sub.category, NodeCategory::SubTree);
        let info = sub.subtree.as_ref().unwrap();
        assert_eq!(info.tree_id, "Sub");
        assert!(info.can_expand);
        assert!(!info.expanded);
        assert_eq!(info.inner_count, 3);
        assert!(sub.children.is_empty());
        assert!(sub.is_collapsed_subtree());
        assert_eq!(sub.ports.len(), 1);

        assert_eq!(canvas.nodes()[3].category, NodeCategory::Decorator);
        assert!(!canvas.nodes()[3].category_inferred);
        assert_eq!(canvas.nodes()[4].depth, 2);
    }

    #[test]
    fn expands_subtree_inline() {
        let canvas = build(RUNTIME, true);
        assert_eq!(canvas.len(), 8);
        let sub = canvas.find_uid(Uid(3)).unwrap();
        let sub_node = canvas.get(sub).unwrap();
        assert!(sub_node.subtree.as_ref().unwrap().expanded);
        assert_eq!(sub_node.children.len(), 1);
        let fb = canvas.get(sub_node.children[0]).unwrap();
        assert_eq!(fb.label, "fb");
        assert_eq!(fb.path, "sub_a/fb");
        assert_eq!(fb.depth, 2);
        let talk = canvas.get(canvas.find_uid(Uid(8)).unwrap()).unwrap();
        assert_eq!(talk.type_id, "Talk");
        assert_eq!(talk.category, NodeCategory::Action);
        assert!(!talk.category_inferred);
        // Depth-first order: the subtree range of the SubTree node.
        assert_eq!(canvas.subtree_range(sub), 2..6);
        assert_eq!(canvas.subtree_range(CanvasNodeId(0)), 0..8);
        assert_eq!(canvas.subtree_range(CanvasNodeId(7)), 7..8);
    }

    #[test]
    fn authoring_file_links_by_tree_id_and_stops_cycles() {
        let tree = RuntimeTree::parse(AUTHORING).unwrap();
        assert_eq!(SubtreeLinking::detect(&tree), SubtreeLinking::ByTreeId);
        let canvas = build(AUTHORING, true);
        let labels: Vec<&str> = canvas.nodes().iter().map(|n| n.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Sequence",
                "first",
                "AlwaysSuccess",
                "second",
                "AlwaysSuccess",
                "Missing",
                "loop"
            ]
        );
        let missing = &canvas.nodes()[5];
        assert!(!missing.subtree.as_ref().unwrap().can_expand);
        let looped = &canvas.nodes()[6];
        assert!(!looped.subtree.as_ref().unwrap().can_expand);
        assert!(looped.children.is_empty());
        // Both uses of "Shared" have their own canvas nodes.
        assert_ne!(canvas.nodes()[2].depth, 0);
    }

    #[test]
    fn default_expansion_depends_on_size() {
        let tree = RuntimeTree::parse(RUNTIME).unwrap();
        let options = RuntimeBuildOptions::for_tree(&tree);
        let expansion = CanvasTree::default_expansion(&tree, options);
        assert_eq!(expansion.len(), 1);

        // A large tree: many SubTree uses of a big tree.
        let mut xml = String::from("<root><BehaviorTree ID=\"M\"><Sequence>");
        for _ in 0..10 {
            xml.push_str("<SubTree ID=\"Big\"/>");
        }
        xml.push_str("</Sequence></BehaviorTree><BehaviorTree ID=\"Big\"><Sequence>");
        for _ in 0..50 {
            xml.push_str("<AlwaysSuccess/>");
        }
        xml.push_str("</Sequence></BehaviorTree></root>");
        let tree = RuntimeTree::parse(&xml).unwrap();
        let options = RuntimeBuildOptions::for_tree(&tree);
        assert!(CanvasTree::default_expansion(&tree, options).is_empty());
    }

    #[test]
    fn status_and_selection() {
        let mut canvas = build(RUNTIME, false);
        assert!(canvas.set_status(Uid(2), NodeStatus::Running, None));
        assert!(!canvas.set_status(Uid(2), NodeStatus::Running, None));
        assert!(!canvas.set_status(Uid(99), NodeStatus::Running, None));
        assert!(canvas.set_status(Uid(2), NodeStatus::Idle, Some(NodeStatus::Success)));
        let hello = canvas.get(canvas.find_uid(Uid(2)).unwrap()).unwrap();
        assert_eq!(hello.deco.previous_status, Some(NodeStatus::Success));
        // A previous status is kept only while the node is IDLE.
        canvas.set_status(Uid(2), NodeStatus::Failure, Some(NodeStatus::Success));
        let hello = canvas.get(canvas.find_uid(Uid(2)).unwrap()).unwrap();
        assert_eq!(hello.deco.previous_status, None);
        canvas.reset_status();
        assert!(
            canvas
                .nodes()
                .iter()
                .all(|n| n.deco.status == NodeStatus::Idle)
        );

        assert!(canvas.select_only(CanvasNodeId(1)));
        assert!(!canvas.select_only(CanvasNodeId(1)));
        assert!(canvas.set_selected(CanvasNodeId(3), true));
        assert_eq!(
            canvas.selected().collect::<Vec<_>>(),
            [CanvasNodeId(1), CanvasNodeId(3)]
        );
        let sources = canvas.selected_sources();
        assert!(canvas.select_all());
        assert_eq!(canvas.selected_count(), 5);
        canvas.select_sources(&sources);
        assert_eq!(canvas.selected_count(), 2);
        assert!(canvas.clear_selection());
        assert!(!canvas.clear_selection());
    }

    #[test]
    fn decorations_survive_rebuild() {
        let mut collapsed = build(RUNTIME, false);
        collapsed.set_status(Uid(2), NodeStatus::Success, None);
        collapsed.select_only(CanvasNodeId(1));
        let mut expanded = build(RUNTIME, true);
        expanded.copy_decorations_from(&collapsed);
        let hello = expanded.get(expanded.find_uid(Uid(2)).unwrap()).unwrap();
        assert_eq!(hello.deco.status, NodeStatus::Success);
        assert!(hello.deco.selected);
        assert_eq!(expanded.selected_count(), 1);
    }

    #[test]
    fn navigation() {
        let canvas = build(RUNTIME, false);
        let root = CanvasNodeId(0);
        let first = canvas.navigate(root, NavStep::FirstChild).unwrap();
        assert_eq!(first, CanvasNodeId(1));
        assert_eq!(canvas.navigate(first, NavStep::Parent), Some(root));
        assert_eq!(canvas.navigate(root, NavStep::Parent), None);
        assert_eq!(canvas.navigate(first, NavStep::PreviousSibling), None);
        let second = canvas.navigate(first, NavStep::NextSibling).unwrap();
        assert_eq!(second, CanvasNodeId(2));
        let third = canvas.navigate(second, NavStep::NextSibling).unwrap();
        assert_eq!(canvas.navigate(third, NavStep::NextSibling), None);
        assert_eq!(canvas.navigate(third, NavStep::PreviousSibling), Some(second));
        assert_eq!(canvas.navigate(root, NavStep::NextSibling), None);
        assert_eq!(canvas.navigate(CanvasNodeId(99), NavStep::Parent), None);
    }

    #[test]
    fn builder_keeps_depth_first_structure() {
        let mut builder = CanvasTreeBuilder::new();
        assert!(builder.is_empty());
        let root = builder.push(None, CanvasNode::new(NodeSource::Runtime(NodeIndex(0)), "r", "Sequence"));
        let a = builder.push(Some(root), CanvasNode::new(NodeSource::Runtime(NodeIndex(1)), "a", "A"));
        builder.push(Some(a), CanvasNode::new(NodeSource::Runtime(NodeIndex(2)), "b", "B"));
        // An out-of-range parent becomes no parent.
        builder.push(Some(CanvasNodeId(42)), CanvasNode::new(NodeSource::Runtime(NodeIndex(3)), "c", "C"));
        assert_eq!(builder.len(), 4);
        let tree = builder.finish("T");
        assert_eq!(tree.get(a).unwrap().children, vec![CanvasNodeId(2)]);
        assert_eq!(tree.get(CanvasNodeId(2)).unwrap().depth, 2);
        assert_eq!(tree.level_count(), 3);
        assert_eq!(tree.find_source(NodeSource::Runtime(NodeIndex(2))), Some(CanvasNodeId(2)));
        assert!(tree.get(CanvasNodeId(3)).unwrap().parent.is_none());
    }

    #[test]
    fn empty_tree() {
        let tree = CanvasTree::default();
        assert!(tree.is_empty());
        assert_eq!(tree.root(), None);
        assert_eq!(tree.level_count(), 0);
        assert_eq!(tree.subtree_range(CanvasNodeId(0)), 0..0);
    }
}
