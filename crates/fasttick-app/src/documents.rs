//! Open documents and their views.
//!
//! A [`Document`] is one source of trees: a tree XML file (edit mode), the
//! tree of a live executor (live mode) or a `.btlog` file (replay mode).
//! A [`View`] shows one tree of a document on a canvas. A document can
//! have several views (split view); each view has its own viewport,
//! selection and SubTree expansion.

use std::path::PathBuf;
use std::sync::Arc;

use fasttick_core::runtime::{InstanceIndex, NodeIndex, RuntimeTree};
use fasttick_core::status::NodeStatus;
use fasttick_log::replay::{ReplayIndex, ReplayState};

use crate::app::Mode;
use crate::canvas::{
    BuiltinCategories, CanvasState, CanvasTree, EdgeStyle, Expansion, LayoutDirection, NodeSource,
    RuntimeBuildOptions, SubtreeLinking,
};

/// The ID of a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocId(pub u64);

/// The ID of a view.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ViewId(pub u64);

/// The kind of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocKind {
    /// A tree XML file, read-only for now.
    TreeFile {
        /// The file.
        path: PathBuf,
    },
    /// The tree of a live executor.
    Live {
        /// `host:port` of the executor.
        endpoint: String,
    },
    /// A `.btlog` file.
    Replay {
        /// The file.
        path: PathBuf,
    },
}

impl DocKind {
    /// The mode that shows documents of this kind.
    pub fn mode(&self) -> Mode {
        match self {
            DocKind::TreeFile { .. } => Mode::Edit,
            DocKind::Live { .. } => Mode::Live,
            DocKind::Replay { .. } => Mode::Replay,
        }
    }

    /// The file of the document, if any.
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            DocKind::TreeFile { path } | DocKind::Replay { path } => Some(path),
            DocKind::Live { .. } => None,
        }
    }
}

/// Replay data of a `.btlog` document.
#[derive(Debug, Clone)]
pub struct ReplayData {
    /// The replay index.
    pub index: ReplayIndex,
    /// The number of transitions that the views show as applied.
    pub position: usize,
    /// The start time of the log (microseconds since the Unix epoch).
    pub start_time_us: i64,
}

impl ReplayData {
    /// The state at the current position.
    pub fn state(&self) -> ReplayState {
        self.index.state_after(self.position)
    }
}

/// One open document.
#[derive(Debug)]
pub struct Document {
    /// The ID.
    pub id: DocId,
    /// The kind.
    pub kind: DocKind,
    /// The tab title.
    pub title: String,
    /// The trees.
    pub runtime: Arc<RuntimeTree>,
    /// How SubTree nodes find their tree.
    pub linking: SubtreeLinking,
    /// Replay data (replay documents only).
    pub replay: Option<ReplayData>,
}

impl Document {
    /// The mode of the document.
    pub fn mode(&self) -> Mode {
        self.kind.mode()
    }
}

/// One view of a document.
#[derive(Debug)]
pub struct View {
    /// The ID.
    pub id: ViewId,
    /// The document.
    pub doc: DocId,
    /// The tree that the view shows.
    pub root_instance: InstanceIndex,
    /// The expanded SubTree nodes.
    pub expansion: Expansion,
    /// The canvas.
    pub canvas: CanvasState,
}

/// All open documents and views.
#[derive(Debug, Default)]
pub struct Workspace {
    docs: Vec<Document>,
    views: Vec<View>,
    next_id: u64,
    /// The view that the commands of the View menu act on.
    pub active_view: Option<ViewId>,
}

/// Options for new views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewOptions {
    /// The layout direction.
    pub direction: LayoutDirection,
    /// The edge style.
    pub edge_style: EdgeStyle,
}

impl Workspace {
    fn next(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// All documents.
    pub fn docs(&self) -> &[Document] {
        &self.docs
    }

    /// All views.
    pub fn views(&self) -> &[View] {
        &self.views
    }

    /// A document.
    pub fn doc(&self, id: DocId) -> Option<&Document> {
        self.docs.iter().find(|d| d.id == id)
    }

    /// A document, for changes.
    pub fn doc_mut(&mut self, id: DocId) -> Option<&mut Document> {
        self.docs.iter_mut().find(|d| d.id == id)
    }

    /// A view.
    pub fn view(&self, id: ViewId) -> Option<&View> {
        self.views.iter().find(|v| v.id == id)
    }

    /// A view, for changes.
    pub fn view_mut(&mut self, id: ViewId) -> Option<&mut View> {
        self.views.iter_mut().find(|v| v.id == id)
    }

    /// A view and its document.
    pub fn view_and_doc_mut(&mut self, id: ViewId) -> Option<(&mut View, &Document)> {
        let view = self.views.iter_mut().find(|v| v.id == id)?;
        let doc = self.docs.iter().find(|d| d.id == view.doc)?;
        Some((view, doc))
    }

    /// The active view, if it still exists.
    pub fn active(&self) -> Option<&View> {
        self.active_view.and_then(|id| self.view(id))
    }

    /// The active view, for changes.
    pub fn active_mut(&mut self) -> Option<&mut View> {
        let id = self.active_view?;
        self.view_mut(id)
    }

    /// The document of a view.
    pub fn doc_of_view(&self, id: ViewId) -> Option<&Document> {
        self.view(id).and_then(|v| self.doc(v.doc))
    }

    /// The views of a document.
    pub fn views_of(&self, doc: DocId) -> impl Iterator<Item = &View> {
        self.views.iter().filter(move |v| v.doc == doc)
    }

    /// The first document of a kind that matches.
    pub fn find_doc(&self, predicate: impl Fn(&Document) -> bool) -> Option<DocId> {
        self.docs.iter().find(|d| predicate(d)).map(|d| d.id)
    }

    /// Adds a document. Returns its ID.
    pub fn add_doc(
        &mut self,
        kind: DocKind,
        title: impl Into<String>,
        runtime: Arc<RuntimeTree>,
        replay: Option<ReplayData>,
    ) -> DocId {
        let id = DocId(self.next());
        let linking = SubtreeLinking::detect(&runtime);
        self.docs.push(Document {
            id,
            kind,
            title: title.into(),
            runtime,
            linking,
            replay,
        });
        id
    }

    /// Replaces the trees of a document (for example after a restart of
    /// the executor). The views show the main tree again.
    pub fn replace_runtime(&mut self, doc: DocId, runtime: Arc<RuntimeTree>) {
        let Some(document) = self.doc_mut(doc) else {
            return;
        };
        document.linking = SubtreeLinking::detect(&runtime);
        document.runtime = runtime;
        let ids: Vec<ViewId> = self.views_of(doc).map(|v| v.id).collect();
        for id in ids {
            let Some((view, document)) = self.view_and_doc_mut(id) else {
                continue;
            };
            view.root_instance = InstanceIndex(0);
            let options = build_options(document, view.root_instance);
            view.expansion = CanvasTree::default_expansion(&document.runtime, options);
            let tree = build_tree(document, view.root_instance, &view.expansion);
            let direction = view.canvas.direction();
            let edge_style = view.canvas.edge_style();
            view.canvas = CanvasState::new(tree);
            view.canvas.set_direction(direction);
            view.canvas.set_edge_style(edge_style);
        }
    }

    /// Opens a new view of one tree of a document.
    pub fn open_view(
        &mut self,
        doc: DocId,
        root_instance: InstanceIndex,
        options: ViewOptions,
    ) -> Option<ViewId> {
        let document = self.doc(doc)?;
        let build = build_options(document, root_instance);
        let expansion = CanvasTree::default_expansion(&document.runtime, build);
        let mut tree = build_tree(document, root_instance, &expansion);
        if let Some(replay) = &document.replay {
            apply_replay_state(&mut tree, &replay.state());
        }
        let mut canvas = CanvasState::new(tree);
        canvas.set_direction(options.direction);
        canvas.set_edge_style(options.edge_style);
        let id = ViewId(self.next());
        self.views.push(View {
            id,
            doc,
            root_instance,
            expansion,
            canvas,
        });
        Some(id)
    }

    /// Opens a second view of the same tree, with the same viewport and
    /// expansion.
    pub fn split_view(&mut self, id: ViewId) -> Option<ViewId> {
        let view = self.view(id)?;
        let (doc, root, expansion) = (view.doc, view.root_instance, view.expansion.clone());
        let (viewport, direction, edge_style) = (
            view.canvas.viewport(),
            view.canvas.direction(),
            view.canvas.edge_style(),
        );
        let document = self.doc(doc)?;
        let mut tree = build_tree(document, root, &expansion);
        tree.copy_decorations_from(view.canvas.tree());
        tree.clear_selection();
        let mut canvas = CanvasState::new(tree);
        canvas.set_direction(direction);
        canvas.set_edge_style(edge_style);
        canvas.set_viewport(viewport);
        canvas.fit_to_view(false);
        let new_id = ViewId(self.next());
        self.views.push(View {
            id: new_id,
            doc,
            root_instance: root,
            expansion,
            canvas,
        });
        Some(new_id)
    }

    /// Closes a view. A document without views closes too. Returns the
    /// closed document, if any.
    pub fn close_view(&mut self, id: ViewId) -> Option<DocId> {
        let index = self.views.iter().position(|v| v.id == id)?;
        let view = self.views.remove(index);
        if self.active_view == Some(id) {
            self.active_view = None;
        }
        if self.views.iter().any(|v| v.doc == view.doc) {
            return None;
        }
        self.docs.retain(|d| d.id != view.doc);
        Some(view.doc)
    }

    /// Shows another tree of the document in a view.
    pub fn set_view_root(&mut self, id: ViewId, root_instance: InstanceIndex) {
        let Some((view, document)) = self.view_and_doc_mut(id) else {
            return;
        };
        if view.root_instance == root_instance {
            return;
        }
        view.root_instance = root_instance;
        let options = build_options(document, root_instance);
        view.expansion = CanvasTree::default_expansion(&document.runtime, options);
        let mut tree = build_tree(document, root_instance, &view.expansion);
        if let Some(replay) = &document.replay {
            apply_replay_state(&mut tree, &replay.state());
        }
        let direction = view.canvas.direction();
        let edge_style = view.canvas.edge_style();
        view.canvas = CanvasState::new(tree);
        view.canvas.set_direction(direction);
        view.canvas.set_edge_style(edge_style);
    }

    /// Expands or collapses a SubTree node of a view. The node stays at its
    /// place on the screen.
    pub fn toggle_subtree(&mut self, id: ViewId, source: NodeSource) {
        let Some((view, document)) = self.view_and_doc_mut(id) else {
            return;
        };
        view.expansion.toggle(source);
        let tree = build_tree(document, view.root_instance, &view.expansion);
        view.canvas.replace_tree(tree, Some(source));
    }

    /// Expands or collapses all SubTree nodes of a view.
    pub fn set_all_subtrees(&mut self, id: ViewId, expanded: bool) {
        let Some((view, document)) = self.view_and_doc_mut(id) else {
            return;
        };
        let mut expansion = Expansion::none();
        if expanded {
            for (index, node) in document.runtime.nodes.iter().enumerate() {
                if node.is_subtree() {
                    expansion.set(NodeSource::Runtime(NodeIndex(index)), true);
                }
            }
        }
        view.expansion = expansion;
        let anchor = view
            .canvas
            .tree()
            .root()
            .and_then(|r| view.canvas.tree().get(r))
            .map(|n| n.source);
        let tree = build_tree(document, view.root_instance, &view.expansion);
        view.canvas.replace_tree(tree, anchor);
        view.canvas.fit_to_view(true);
    }

    /// Expands the SubTree node of an instance and all SubTree nodes above
    /// it, and focuses the SubTree node (runtime trees only).
    pub fn reveal_instance(&mut self, id: ViewId, instance: InstanceIndex) {
        let Some((view, document)) = self.view_and_doc_mut(id) else {
            return;
        };
        let runtime = &document.runtime;
        let Some(target) = runtime
            .instances
            .get(instance.0)
            .and_then(|i| i.parent_node)
        else {
            // The root instance: focus the root.
            if let Some(root) = view.canvas.tree().root() {
                view.canvas.focus_on(root, true);
            }
            return;
        };
        // Expand the target and all SubTree nodes above it.
        let mut changed = false;
        if !view.expansion.is_expanded(NodeSource::Runtime(target)) {
            view.expansion.set(NodeSource::Runtime(target), true);
            changed = true;
        }
        let mut current = runtime.node(target).instance;
        while let Some(parent) = runtime.instances.get(current.0).and_then(|i| i.parent_node) {
            let source = NodeSource::Runtime(parent);
            if !view.expansion.is_expanded(source) {
                view.expansion.set(source, true);
                changed = true;
            }
            current = runtime.node(parent).instance;
        }
        if changed {
            let tree = build_tree(document, view.root_instance, &view.expansion);
            view.canvas.replace_tree(tree, None);
        }
        if let Some(node) = view.canvas.tree().find_source(NodeSource::Runtime(target)) {
            view.canvas.focus_on(node, true);
        }
    }

    /// Sets the replay position of a document and updates its views.
    pub fn set_replay_position(&mut self, doc: DocId, position: usize) {
        let Some(document) = self.doc_mut(doc) else {
            return;
        };
        let Some(replay) = &mut document.replay else {
            return;
        };
        replay.position = position.min(replay.index.len());
        let state = replay.state();
        for view in self.views.iter_mut().filter(|v| v.doc == doc) {
            apply_replay_state(view.canvas.tree_mut(), &state);
        }
    }

    /// Applies a function to the canvas trees of all views of a document.
    pub fn for_each_tree(&mut self, doc: DocId, mut f: impl FnMut(&mut CanvasTree)) {
        for view in self.views.iter_mut().filter(|v| v.doc == doc) {
            f(view.canvas.tree_mut());
        }
    }

    /// The views of documents of a mode.
    pub fn views_in_mode(&self, mode: Mode) -> Vec<ViewId> {
        self.views
            .iter()
            .filter(|v| self.doc(v.doc).is_some_and(|d| d.mode() == mode))
            .map(|v| v.id)
            .collect()
    }
}

/// The build options for a document and a root instance.
pub fn build_options(document: &Document, root_instance: InstanceIndex) -> RuntimeBuildOptions {
    RuntimeBuildOptions {
        root_instance,
        linking: document.linking,
    }
}

/// Builds the canvas tree of a view.
pub fn build_tree(
    document: &Document,
    root_instance: InstanceIndex,
    expansion: &Expansion,
) -> CanvasTree {
    // TODO(after the merge of `fasttick_core::models`): use the model
    // registry of the project for the categories.
    CanvasTree::from_runtime(
        &document.runtime,
        build_options(document, root_instance),
        expansion,
        &BuiltinCategories,
    )
}

/// Shows a replay state on a canvas tree.
pub fn apply_replay_state(tree: &mut CanvasTree, state: &ReplayState) {
    tree.reset_status();
    for (uid, node) in state.iter() {
        let previous = if node.status == NodeStatus::Idle {
            node.last_non_idle
        } else {
            None
        };
        tree.set_status(uid, node.status, previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fasttick_core::runtime::Uid;
    use fasttick_log::btlog::Transition;

    const XML: &str = r#"<root BTCPP_format="4">
        <BehaviorTree ID="Main"><Sequence _uid="1"><SubTree ID="Sub" _fullpath="s" _uid="2"/><AlwaysSuccess _uid="3"/></Sequence></BehaviorTree>
        <BehaviorTree ID="Sub" _fullpath="s"><Sequence _uid="4"><SubTree ID="Leaf" _fullpath="s/l" _uid="5"/></Sequence></BehaviorTree>
        <BehaviorTree ID="Leaf" _fullpath="s/l"><AlwaysFailure _uid="6"/></BehaviorTree>
    </root>"#;

    fn workspace() -> (Workspace, DocId, ViewId) {
        let mut ws = Workspace::default();
        let runtime = Arc::new(RuntimeTree::parse(XML).unwrap());
        let doc = ws.add_doc(
            DocKind::Live {
                endpoint: "localhost:1667".into(),
            },
            "Main",
            runtime,
            None,
        );
        let view = ws
            .open_view(doc, InstanceIndex(0), ViewOptions::default())
            .unwrap();
        (ws, doc, view)
    }

    #[test]
    fn views_and_documents() {
        let (mut ws, doc, view) = workspace();
        assert_eq!(ws.doc(doc).unwrap().mode(), Mode::Live);
        assert_eq!(ws.doc(doc).unwrap().linking, SubtreeLinking::ByInstance);
        // A small tree expands all SubTrees.
        assert_eq!(ws.view(view).unwrap().canvas.tree().len(), 6);
        let second = ws.split_view(view).unwrap();
        assert_eq!(ws.views_of(doc).count(), 2);
        assert_eq!(ws.views_in_mode(Mode::Live).len(), 2);
        assert!(ws.views_in_mode(Mode::Edit).is_empty());
        assert_eq!(ws.close_view(view), None);
        assert!(ws.doc(doc).is_some());
        ws.active_view = Some(second);
        assert_eq!(ws.close_view(second), Some(doc));
        assert!(ws.doc(doc).is_none());
        assert_eq!(ws.active_view, None);
    }

    #[test]
    fn toggle_and_reveal() {
        let (mut ws, _doc, view) = workspace();
        let sub = NodeSource::Runtime(NodeIndex(1));
        ws.toggle_subtree(view, sub);
        assert_eq!(ws.view(view).unwrap().canvas.tree().len(), 3);
        ws.set_all_subtrees(view, false);
        assert_eq!(ws.view(view).unwrap().canvas.tree().len(), 3);
        // Reveal the innermost instance: both SubTrees expand.
        ws.reveal_instance(view, InstanceIndex(2));
        let v = ws.view(view).unwrap();
        assert_eq!(v.canvas.tree().len(), 6);
        let focused = v.canvas.primary().unwrap();
        assert_eq!(v.canvas.tree().get(focused).unwrap().uid, Some(Uid(5)));
        ws.set_all_subtrees(view, true);
        assert_eq!(ws.view(view).unwrap().canvas.tree().len(), 6);
    }

    #[test]
    fn replay_positions_update_views() {
        let mut ws = Workspace::default();
        let runtime = Arc::new(RuntimeTree::parse(XML).unwrap());
        let transitions = vec![
            Transition::new(10, Uid(1), NodeStatus::Running),
            Transition::new(20, Uid(3), NodeStatus::Success),
            Transition::new(30, Uid(3), NodeStatus::Idle),
        ];
        let index = ReplayIndex::new(&transitions);
        let doc = ws.add_doc(
            DocKind::Replay {
                path: "/tmp/x.btlog".into(),
            },
            "x.btlog",
            runtime,
            Some(ReplayData {
                index,
                position: 3,
                start_time_us: 0,
            }),
        );
        let view = ws
            .open_view(doc, InstanceIndex(0), ViewOptions::default())
            .unwrap();
        let tree = ws.view(view).unwrap().canvas.tree();
        let node3 = tree.get(tree.find_uid(Uid(3)).unwrap()).unwrap();
        assert_eq!(node3.deco.status, NodeStatus::Idle);
        assert_eq!(node3.deco.previous_status, Some(NodeStatus::Success));
        ws.set_replay_position(doc, 2);
        let tree = ws.view(view).unwrap().canvas.tree();
        let node3 = tree.get(tree.find_uid(Uid(3)).unwrap()).unwrap();
        assert_eq!(node3.deco.status, NodeStatus::Success);
        ws.set_replay_position(doc, 0);
        let tree = ws.view(view).unwrap().canvas.tree();
        assert!(
            tree.nodes()
                .iter()
                .all(|n| n.deco.status == NodeStatus::Idle)
        );
        // Out of range clamps.
        ws.set_replay_position(doc, 999);
        assert_eq!(ws.doc(doc).unwrap().replay.as_ref().unwrap().position, 3);
    }

    #[test]
    fn replace_runtime_rebuilds_views() {
        let (mut ws, doc, view) = workspace();
        let small = Arc::new(
            RuntimeTree::parse(
                r#"<root><BehaviorTree ID="T"><AlwaysSuccess _uid="1"/></BehaviorTree></root>"#,
            )
            .unwrap(),
        );
        ws.replace_runtime(doc, small);
        assert_eq!(ws.view(view).unwrap().canvas.tree().len(), 1);
        assert_eq!(ws.view(view).unwrap().canvas.tree().title, "T");
    }

    #[test]
    fn set_view_root_shows_other_tree() {
        let (mut ws, _doc, view) = workspace();
        ws.set_view_root(view, InstanceIndex(2));
        let v = ws.view(view).unwrap();
        assert_eq!(v.canvas.tree().title, "Leaf");
        assert_eq!(v.canvas.tree().len(), 1);
    }
}
