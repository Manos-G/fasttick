//! Open documents and their views.
//!
//! A [`Document`] is one source of trees: a tree XML file (edit mode), the
//! tree of a live executor (live mode) or a `.btlog` file (replay mode).
//! A [`View`] shows one tree of a document on a canvas. A document can
//! have several views (split view); each view has its own viewport,
//! selection and SubTree expansion.

use std::path::PathBuf;
use std::sync::Arc;

use fasttick_core::runtime::{InstanceIndex, NodeIndex, RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use fasttick_log::replay::ReplayState;

use crate::app::Mode;
use crate::canvas::{
    CanvasState, CanvasTree, EdgeStyle, Expansion, LayoutDirection, NodeSource,
    RuntimeBuildOptions, SubtreeLinking, TableCategories,
};
use crate::editor::EditorDoc;
use crate::live::LiveController;
use crate::replay::ReplayModel;
use fasttick_core::project::FileId;
use fasttick_core::xml::NodeId;

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
    /// A tree XML file or a project file, open for editing. The path is
    /// empty for a new file that was not saved yet.
    TreeFile {
        /// The file.
        path: PathBuf,
    },
    /// The tree of a live executor.
    Live {
        /// `host:port` of the executor.
        endpoint: String,
    },
    /// A log: a `.btlog` file or one session of a SQLite database.
    Replay {
        /// The file.
        path: PathBuf,
        /// The session ID in a SQLite database.
        session: Option<i64>,
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
            DocKind::TreeFile { path } | DocKind::Replay { path, .. } => Some(path),
            DocKind::Live { .. } => None,
        }
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
    /// The replay model (replay documents only).
    pub replay: Option<ReplayModel>,
    /// The editor (edit documents only). For an edit document,
    /// [`Document::runtime`] is an empty placeholder and an
    /// [`InstanceIndex`] is a tree index of the main file.
    pub editor: Option<Box<EditorDoc>>,
    /// The main tree: `main_tree_to_execute` of a tree file, else the
    /// first tree (the root instance of a runtime tree).
    pub main_instance: InstanceIndex,
    /// The categories of the node types of a runtime tree, from the
    /// `<TreeNodesModel>` that the executor or the log sends with the tree
    /// (see [`runtime_categories`]).
    pub categories: TableCategories,
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
    /// The decoration key (live generation or replay position) that the
    /// canvas tree shows. `None` after the tree was built again.
    pub decorated: Option<u64>,
    /// The editor revision that the canvas shows (edit documents only).
    pub built_revision: u64,
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
    /// Show the minimap.
    pub minimap: bool,
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
        replay: Option<ReplayModel>,
    ) -> DocId {
        let id = DocId(self.next());
        let linking = SubtreeLinking::detect(&runtime);
        let categories = runtime_categories(&runtime);
        self.docs.push(Document {
            id,
            kind,
            title: title.into(),
            categories,
            runtime,
            linking,
            replay,
            editor: None,
            main_instance: InstanceIndex(0),
        });
        id
    }

    /// Adds an edit document. Returns its ID.
    pub fn add_editor_doc(
        &mut self,
        path: PathBuf,
        title: impl Into<String>,
        editor: EditorDoc,
    ) -> DocId {
        let id = DocId(self.next());
        let main = InstanceIndex(editor.main_tree_index());
        self.docs.push(Document {
            id,
            kind: DocKind::TreeFile { path },
            title: title.into(),
            runtime: Arc::new(placeholder_runtime()),
            linking: SubtreeLinking::ByTreeId,
            replay: None,
            editor: Some(Box::new(editor)),
            main_instance: main,
            categories: TableCategories::default(),
        });
        id
    }

    /// Builds the canvas of each edit view again if its editor changed
    /// after the last build. The selection and the viewport stay. A view
    /// whose tree no longer exists shows the main tree.
    pub fn refresh_editor_views(&mut self) {
        let Workspace { docs, views, .. } = self;
        for view in views.iter_mut() {
            let Some(document) = docs.iter().find(|d| d.id == view.doc) else {
                continue;
            };
            let Some(editor) = document.editor.as_deref() else {
                continue;
            };
            if view.built_revision == editor.revision() {
                continue;
            }
            // The view keeps its tree when the tree indices move (a tree
            // was added or removed before it). After a rename the index
            // stays.
            let trees = editor.trees();
            let shown = &view.canvas.tree().title;
            if trees.get(view.root_instance.0).map(|(_, id)| id) != Some(shown)
                && let Some(index) = editor.tree_index(shown)
            {
                view.root_instance = InstanceIndex(index);
            }
            if view.root_instance.0 >= trees.len() {
                view.root_instance = InstanceIndex(editor.main_tree_index());
            }
            let tree = editor.build_canvas(view.root_instance.0, &view.expansion);
            view.canvas.replace_tree(tree, None);
            view.built_revision = editor.revision();
        }
    }

    /// The editor of a document, for changes.
    pub fn editor_mut(&mut self, doc: DocId) -> Option<&mut EditorDoc> {
        self.doc_mut(doc)?.editor.as_deref_mut()
    }

    /// Selects an element of an edit document in a view and moves the view
    /// to it. Returns true if the node is on the canvas.
    pub fn reveal_xml(&mut self, id: ViewId, file: FileId, node: NodeId, select: bool) -> bool {
        self.refresh_editor_views();
        // A node of another tree: show that tree first.
        let shown = self.view(id).is_some_and(|v| {
            v.canvas
                .tree()
                .find_source(NodeSource::Xml(file, node))
                .is_some()
        });
        if !shown
            && let Some(index) = self
                .doc_of_view(id)
                .and_then(|d| d.editor.as_deref())
                .and_then(|e| e.tree_of_node(file, node))
        {
            self.set_view_root(id, InstanceIndex(index));
        }
        let Some(view) = self.view_mut(id) else {
            return false;
        };
        let Some(canvas_id) = view.canvas.tree().find_source(NodeSource::Xml(file, node)) else {
            return false;
        };
        if select {
            view.canvas.tree_mut().select_only(canvas_id);
        }
        view.canvas.focus_on(canvas_id, true);
        true
    }

    /// Replaces the trees of a document (for example after a restart of
    /// the executor). The views show the main tree again.
    pub fn replace_runtime(&mut self, doc: DocId, runtime: Arc<RuntimeTree>) {
        let Some(document) = self.doc_mut(doc) else {
            return;
        };
        document.linking = SubtreeLinking::detect(&runtime);
        document.categories = runtime_categories(&runtime);
        document.runtime = runtime;
        let ids: Vec<ViewId> = self.views_of(doc).map(|v| v.id).collect();
        for id in ids {
            let Some((view, document)) = self.view_and_doc_mut(id) else {
                continue;
            };
            view.root_instance = InstanceIndex(0);
            view.expansion = default_expansion(document, view.root_instance);
            let tree = build_tree(document, view.root_instance, &view.expansion);
            view.canvas = fresh_canvas(tree, &view.canvas);
            view.decorated = None;
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
        let expansion = default_expansion(document, root_instance);
        let tree = build_tree(document, root_instance, &expansion);
        let built_revision = document.editor.as_ref().map_or(0, |e| e.revision());
        let mut canvas = CanvasState::new(tree);
        canvas.set_direction(options.direction);
        canvas.set_edge_style(options.edge_style);
        canvas.set_minimap(options.minimap);
        let id = ViewId(self.next());
        self.views.push(View {
            id,
            doc,
            root_instance,
            expansion,
            canvas,
            decorated: None,
            built_revision,
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
        let built_revision = document.editor.as_ref().map_or(0, |e| e.revision());
        let mut tree = build_tree(document, root, &expansion);
        tree.copy_decorations_from(view.canvas.tree());
        tree.clear_selection();
        let mut canvas = CanvasState::new(tree);
        canvas.set_direction(direction);
        canvas.set_edge_style(edge_style);
        canvas.set_minimap(view.canvas.minimap());
        canvas.set_viewport(viewport);
        canvas.fit_to_view(false);
        let new_id = ViewId(self.next());
        self.views.push(View {
            id: new_id,
            doc,
            root_instance: root,
            expansion,
            canvas,
            decorated: None,
            built_revision,
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
        view.expansion = default_expansion(document, root_instance);
        let tree = build_tree(document, root_instance, &view.expansion);
        view.canvas = fresh_canvas(tree, &view.canvas);
        view.decorated = None;
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
        view.decorated = None;
    }

    /// Expands or collapses all SubTree nodes of a view.
    pub fn set_all_subtrees(&mut self, id: ViewId, expanded: bool) {
        let Some((view, document)) = self.view_and_doc_mut(id) else {
            return;
        };
        let mut expansion = Expansion::none();
        if expanded {
            if let Some(editor) = &document.editor {
                expansion = CanvasTree::all_project_subtrees(editor.project());
            } else {
                for (index, node) in document.runtime.nodes.iter().enumerate() {
                    if node.is_subtree() {
                        expansion.set(NodeSource::Runtime(NodeIndex(index)), true);
                    }
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
        view.decorated = None;
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
            view.decorated = None;
        }
        if let Some(node) = view.canvas.tree().find_source(NodeSource::Runtime(target)) {
            view.canvas.focus_on(node, true);
        }
    }

    /// Shows a node of the runtime tree in a view: expands the SubTree
    /// nodes above it, selects it and moves the view to it. Does nothing if
    /// the node is not in the tree that the view shows. Returns true if
    /// the node is on the canvas.
    pub fn reveal_node(&mut self, id: ViewId, node: NodeIndex, select: bool) -> bool {
        let Some((view, document)) = self.view_and_doc_mut(id) else {
            return false;
        };
        let runtime = &document.runtime;
        if node.0 >= runtime.len() {
            return false;
        }
        // Expand the SubTree nodes above the node, up to the view root.
        let mut changed = false;
        let mut current = runtime.node(node).instance;
        while current != view.root_instance {
            let Some(parent) = runtime.instances.get(current.0).and_then(|i| i.parent_node) else {
                break;
            };
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
            view.decorated = None;
        }
        let Some(id) = view.canvas.tree().find_source(NodeSource::Runtime(node)) else {
            return false;
        };
        if select {
            view.canvas.tree_mut().select_only(id);
        }
        view.canvas.focus_on(id, true);
        true
    }

    /// Like [`Workspace::reveal_node`], for a node UID.
    pub fn reveal_uid(&mut self, id: ViewId, uid: Uid, select: bool) -> bool {
        let Some(node) = self
            .doc_of_view(id)
            .and_then(|d| d.runtime.index_of_uid(uid))
        else {
            return false;
        };
        self.reveal_node(id, node, select)
    }

    /// Decorates the canvas trees whose decorations are old: the live
    /// statuses and markers, or the replay state at the cursor.
    pub fn refresh_decorations(&mut self, live: &LiveController, live_doc: Option<DocId>) {
        let Workspace { docs, views, .. } = self;
        for document in docs.iter() {
            let key = if let Some(replay) = &document.replay {
                replay.decoration_key()
            } else if live_doc == Some(document.id) {
                live.generation()
            } else {
                continue;
            };
            let mut state = None;
            for view in views
                .iter_mut()
                .filter(|v| v.doc == document.id && v.decorated != Some(key))
            {
                if let Some(replay) = &document.replay {
                    let state = state.get_or_insert_with(|| replay.state());
                    apply_replay_state(view.canvas.tree_mut(), state);
                } else {
                    live.decorate(view.canvas.tree_mut());
                }
                view.decorated = Some(key);
            }
        }
    }

    /// The replay model of a document.
    pub fn replay_mut(&mut self, doc: DocId) -> Option<&mut ReplayModel> {
        self.doc_mut(doc)?.replay.as_mut()
    }

    /// The first replay document of the active view, else the first replay
    /// document.
    pub fn active_replay_doc(&self) -> Option<DocId> {
        self.active()
            .and_then(|v| self.doc(v.doc))
            .filter(|d| d.replay.is_some())
            .or_else(|| self.docs.iter().find(|d| d.replay.is_some()))
            .map(|d| d.id)
    }

    /// The view of a document to act on: the active view if it shows the
    /// document, else its first view.
    pub fn view_for_doc(&self, doc: DocId) -> Option<ViewId> {
        self.active()
            .filter(|v| v.doc == doc)
            .or_else(|| self.views_of(doc).next())
            .map(|v| v.id)
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

/// A new canvas for a tree with the display settings of an old one.
fn fresh_canvas(tree: CanvasTree, old: &CanvasState) -> CanvasState {
    let mut canvas = CanvasState::new(tree);
    canvas.set_direction(old.direction());
    canvas.set_edge_style(old.edge_style());
    canvas.set_minimap(old.minimap());
    canvas
}

/// The build options for a document and a root instance.
pub fn build_options(document: &Document, root_instance: InstanceIndex) -> RuntimeBuildOptions {
    RuntimeBuildOptions {
        root_instance,
        linking: document.linking,
    }
}

/// The SubTree expansion of a new view. Edit views start with all
/// SubTrees collapsed (inline trees are read-only there).
pub fn default_expansion(document: &Document, root_instance: InstanceIndex) -> Expansion {
    if document.editor.is_some() {
        return Expansion::none();
    }
    CanvasTree::default_expansion(&document.runtime, build_options(document, root_instance))
}

/// The empty runtime tree of an edit document.
fn placeholder_runtime() -> RuntimeTree {
    // This text is valid runtime XML, so the parse cannot fail; the
    // fallback keeps the code free of panics.
    RuntimeTree::parse(r#"<root><BehaviorTree ID=""/></root>"#)
        .or_else(|_| RuntimeTree::parse("<BehaviorTree/>"))
        .unwrap_or_else(|_| unreachable!("constant runtime XML is valid"))
}

/// Builds the canvas tree of a view.
pub fn build_tree(
    document: &Document,
    root_instance: InstanceIndex,
    expansion: &Expansion,
) -> CanvasTree {
    if let Some(editor) = &document.editor {
        return editor.build_canvas(root_instance.0, expansion);
    }
    CanvasTree::from_runtime(
        &document.runtime,
        build_options(document, root_instance),
        expansion,
        &document.categories,
    )
}

/// The categories of the node types in the `<TreeNodesModel>` of a runtime
/// tree. The publisher and the loggers of BT.CPP write the models of all
/// registered nodes with the tree, so custom conditions, controls and
/// decorators get their real category. Types without a model fall back to
/// the built-in table (see [`TableCategories`]).
pub fn runtime_categories(runtime: &RuntimeTree) -> TableCategories {
    let Ok(parsed) = fasttick_core::models::parse_models_text(
        &runtime.xml,
        fasttick_core::models::ModelSource::Unattached,
    ) else {
        return TableCategories::default();
    };
    TableCategories::new(
        parsed
            .into_models()
            .into_iter()
            .map(|model| (model.id, crate::canvas::category_of_kind(model.kind))),
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
        let transitions = vec![
            Transition::new(10, Uid(1), NodeStatus::Running),
            Transition::new(20, Uid(3), NodeStatus::Success),
            Transition::new(30, Uid(3), NodeStatus::Idle),
        ];
        let log = fasttick_log::btlog::BtLog::from_parts(XML.to_owned(), 0, transitions).unwrap();
        let index = fasttick_log::replay::ReplayIndex::from_log(&log);
        let runtime = Arc::new(log.tree.clone());
        let doc = ws.add_doc(
            DocKind::Replay {
                path: "/tmp/x.btlog".into(),
                session: None,
            },
            "x.btlog",
            runtime,
            Some(ReplayModel::new(log, index)),
        );
        let view = ws
            .open_view(doc, InstanceIndex(0), ViewOptions::default())
            .unwrap();
        assert_eq!(ws.active_replay_doc(), Some(doc));
        assert_eq!(ws.view_for_doc(doc), Some(view));
        let live = LiveController::default();
        let status = |ws: &Workspace, uid: u16| {
            let tree = ws.view(view).unwrap().canvas.tree();
            tree.get(tree.find_uid(Uid(uid)).unwrap()).unwrap().deco
        };
        // The model starts at the end; the views decorate lazily.
        ws.refresh_decorations(&live, None);
        assert_eq!(status(&ws, 3).status, NodeStatus::Idle);
        assert_eq!(status(&ws, 3).previous_status, Some(NodeStatus::Success));
        ws.replay_mut(doc).unwrap().step_back();
        ws.refresh_decorations(&live, None);
        assert_eq!(status(&ws, 3).status, NodeStatus::Success);
        ws.replay_mut(doc).unwrap().go_to_start();
        ws.refresh_decorations(&live, None);
        let tree = ws.view(view).unwrap().canvas.tree();
        assert!(
            tree.nodes()
                .iter()
                .all(|n| n.deco.status == NodeStatus::Idle)
        );
        assert_eq!(ws.view(view).unwrap().decorated, Some(0));
    }

    #[test]
    fn reveal_node_expands_and_selects() {
        let (mut ws, _doc, view) = workspace();
        ws.set_all_subtrees(view, false);
        assert_eq!(ws.view(view).unwrap().canvas.tree().len(), 3);
        // UID 6 is in the innermost instance.
        assert!(ws.reveal_uid(view, Uid(6), true));
        let v = ws.view(view).unwrap();
        assert_eq!(v.canvas.tree().len(), 6);
        assert_eq!(v.decorated, None);
        let selected: Vec<_> = v.canvas.tree().selected().collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(v.canvas.tree().get(selected[0]).unwrap().uid, Some(Uid(6)));
        assert!(!ws.reveal_uid(view, Uid(99), true));
        // A view of the inner tree does not show the outer nodes.
        ws.set_view_root(view, InstanceIndex(2));
        assert!(!ws.reveal_uid(view, Uid(1), true));
        assert!(ws.reveal_uid(view, Uid(6), false));
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
