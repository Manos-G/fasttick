//! The [`TreeCanvas`] widget and its persistent [`CanvasState`].
//!
//! The canvas draws a [`CanvasTree`] with its own painter code:
//!
//! - It draws only the nodes and edges in the visible rectangle.
//! - It keeps the text layouts (galleys) of the visible nodes in a cache.
//!   Zoom values in one "bucket" (about 9 %) share a font size, so a zoom
//!   gesture does not lay out text again on each frame.
//! - It has three levels of detail: full cards, labels only, and plain
//!   boxes when the cards are very small ([`Detail`]).
//! - It collects all shapes in one buffer and gives them to egui in one
//!   call.
//!
//! Input: drag with the middle mouse button (or Space + drag, or drag on a
//! node) to pan; mouse wheel or touchpad scroll to pan; Ctrl + wheel or a
//! pinch to zoom at the pointer; click, Shift-click (add) and Ctrl-click
//! (toggle) to select; drag on the background to select with a rubber
//! band; double-click a SubTree to expand or collapse it; arrow keys to
//! move the selection to the parent, a child or a sibling.
//!
//! An editable canvas ([`TreeCanvas::editable`]) also moves nodes: drag a
//! node (with the selected nodes) onto another node, or onto an empty place
//! to disconnect it. A drag of a [`NewNodePayload`] (from the palette) adds
//! a node. Escape cancels a move. The canvas only reports the gesture as a
//! [`CanvasEvent`]; the owner changes the document.

use std::collections::HashMap;
use std::sync::Arc;

use egui::epaint::Mesh;
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Color32, CornerRadius, EventFilter, FontFamily, FontId, Galley, Id, Key, Modifiers,
    PointerButton, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};
use fasttick_core::status::NodeStatus;

use super::card::{self, CardGeometry, TextKind, TextMeasure};
use super::connect::{self, ConnectEnd};
use super::drop::{self, DropPlacement, DropSpot, NewNodePayload};
use super::hit;
use super::layout::{self, LayoutDirection, TreeLayout};
use super::model::{
    BreakpointMarker, CanvasNode, CanvasNodeId, CanvasTree, CardEntry, NavStep, NodeCategory,
    NodeSource, Substitution,
};
use super::paint::{self, EdgeStyle};
use super::viewport::{self, Viewport};
use crate::fonts;
use crate::theme::{CanvasPalette, mix};

/// The level of detail of the cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Detail {
    /// Plain boxes: no text.
    Boxes,
    /// Glyph, label and status badge.
    Labels,
    /// All text, including the type line and the ports.
    #[default]
    Full,
}

impl Detail {
    /// The detail level for a zoom factor.
    pub fn for_zoom(zoom: f32) -> Detail {
        if zoom >= 0.62 {
            Detail::Full
        } else if zoom >= 0.42 {
            Detail::Labels
        } else {
            Detail::Boxes
        }
    }
}

/// Statistics of the last frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CanvasStats {
    /// The number of nodes in the tree.
    pub total_nodes: usize,
    /// The number of nodes that the canvas drew.
    pub visible_nodes: usize,
    /// The time to build the shapes, in milliseconds.
    pub paint_ms: f32,
    /// The detail level.
    pub detail: Detail,
    /// The number of cached text layouts.
    pub cached_galleys: usize,
}

/// Something that happened on the canvas. The owner of the canvas reacts.
#[derive(Debug, Clone, PartialEq)]
pub enum CanvasEvent {
    /// The selection changed.
    SelectionChanged,
    /// The user asked to expand or collapse a SubTree node. The owner
    /// rebuilds the tree (see [`CanvasState::replace_tree`]).
    ToggleSubtree(NodeSource),
    /// The user asked to expand (true) or collapse (false) all SubTrees.
    SetAllSubtrees(bool),
    /// The user asked to fold a node (hide its children) or to unfold it.
    /// The owner rebuilds the tree (see [`Expansion::toggle_fold`]).
    ToggleFold(NodeSource),
    /// The user asked to unfold all nodes.
    UnfoldAll,
    /// The user double-clicked a node that is not a SubTree.
    Activated(CanvasNodeId),
    /// The user dragged nodes to a new place (editable canvas only). The
    /// spot is never [`DropSpot::Invalid`].
    MoveNodes {
        /// The moved nodes, in tree order. A node with a moved ancestor is
        /// not in the list.
        nodes: Vec<CanvasNodeId>,
        /// Where the nodes go.
        spot: DropSpot,
    },
    /// The user dropped a node type from the palette (editable canvas
    /// only). The spot is never [`DropSpot::Invalid`].
    DropNew {
        /// The type ID.
        type_id: String,
        /// Where the node goes.
        spot: DropSpot,
    },
    /// The user released a connection from the dot of a node, or clicked
    /// the dot (editable canvas only). The owner asks for the type of the
    /// new node. The spot is a [`DropSpot::Node`].
    AddChild {
        /// Where the node goes.
        spot: DropSpot,
        /// The screen position of the pointer, for the question.
        at: Pos2,
    },
}

/// The result of [`TreeCanvas::show`].
#[derive(Debug)]
pub struct CanvasOutput {
    /// The response of the canvas area.
    pub response: Response,
    /// The events of this frame.
    pub events: Vec<CanvasEvent>,
}

/// A view change that waits for the canvas size.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PendingView {
    /// The first view of a tree: fit if the cards stay readable, else
    /// show the root at a readable zoom.
    Initial,
    Fit {
        animate: bool,
    },
    Focus {
        id: CanvasNodeId,
        animate: bool,
    },
    Reveal {
        id: CanvasNodeId,
    },
    /// Keep a node at the same place on the screen after a rebuild.
    Anchor {
        source: NodeSource,
        offset: Vec2,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ViewAnimation {
    from: Viewport,
    to: Viewport,
    start: f64,
    duration: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum DragMode {
    #[default]
    None,
    Pan,
    Band {
        start: Pos2,
        additive: bool,
    },
    /// A move of the nodes in [`CanvasState::moving`]. `start` is the world
    /// position of the press.
    Move {
        start: Pos2,
    },
    /// A connection from the dot of a node (see [`connect`]).
    Connect {
        from: CanvasNodeId,
    },
}

/// Cached text layouts of the visible cards.
#[derive(Default)]
struct GalleyCache {
    bucket: i32,
    palette_key: u64,
    map: HashMap<u64, Arc<Galley>>,
}

impl std::fmt::Debug for GalleyCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GalleyCache")
            .field("bucket", &self.bucket)
            .field("entries", &self.map.len())
            .finish()
    }
}

/// Slots of the galley cache for one node.
const SLOT_LABEL: u64 = 0;
const SLOT_TYPE: u64 = 1;
const SLOT_CHIP: u64 = 9;
const SLOT_TAG: u64 = 10;
const SLOT_LABEL_SMALL: u64 = 11;
const SLOT_TAG_FAILURE: u64 = 12;
const SLOT_FOLD: u64 = 13;
/// The text lines of the ports, then of the conditions.
const SLOT_LINES: u64 = 16;

/// The smallest label size (screen points) at [`Detail::Labels`].
const LABEL_MIN_PX: f32 = 9.5;

impl GalleyCache {
    fn prepare(&mut self, bucket: i32, palette_key: u64) {
        if bucket != self.bucket || palette_key != self.palette_key {
            self.map.clear();
            self.bucket = bucket;
            self.palette_key = palette_key;
        }
    }

    fn get(&mut self, node: u32, slot: u64, make: impl FnOnce() -> Arc<Galley>) -> Arc<Galley> {
        let key = (u64::from(node) << 24) | slot;
        self.map.entry(key).or_insert_with(make).clone()
    }

    fn clear(&mut self) {
        self.map.clear();
    }
}

/// The persistent state of a canvas: the tree, its layout, the viewport,
/// the interaction state and the caches. Keep one for each view.
#[derive(Debug)]
pub struct CanvasState {
    tree: CanvasTree,
    layout: TreeLayout,
    groups: Vec<(CanvasNodeId, Rect)>,
    edge_bounds: Vec<Option<Rect>>,
    direction: LayoutDirection,
    edge_style: EdgeStyle,
    minimap: bool,
    viewport: Viewport,
    needs_layout: bool,
    pending: Option<PendingView>,
    animation: Option<ViewAnimation>,
    primary: Option<CanvasNodeId>,
    hovered: Option<CanvasNodeId>,
    context_target: Option<CanvasNodeId>,
    hover_start: f64,
    drag: DragMode,
    band_end: Option<Pos2>,
    editable: bool,
    moving: Vec<CanvasNodeId>,
    drop: Option<DropSpot>,
    drop_new: bool,
    edge_since: Option<f64>,
    hovered_dot: Option<CanvasNodeId>,
    connect_end: Option<ConnectEnd>,
    text_bucket: Option<i32>,
    last_zoom: f32,
    zoom_changed_at: f64,
    galleys: GalleyCache,
    visible: Vec<u32>,
    shapes: Vec<Shape>,
    points: Vec<Pos2>,
    size: Vec2,
    stats: CanvasStats,
    fonts: CardFonts,
    minimap_cache: Option<(u64, Arc<Mesh>)>,
}

/// The duration of view animations, in seconds.
const ANIMATION_SECONDS: f32 = 0.35;
/// The duration of short animations (keyboard navigation), in seconds.
const SHORT_ANIMATION_SECONDS: f32 = 0.15;
/// The delay before the hover tooltip, in seconds.
const TOOLTIP_DELAY: f64 = 0.45;
/// The time without a zoom change after which the text gets a new layout
/// at the new size, in seconds.
const TEXT_SETTLE_SECONDS: f64 = 0.12;

impl CanvasState {
    /// A state for a tree. The first frame fits the tree to the view.
    pub fn new(tree: CanvasTree) -> Self {
        CanvasState {
            stats: CanvasStats {
                total_nodes: tree.len(),
                ..CanvasStats::default()
            },
            tree,
            layout: TreeLayout::default(),
            groups: Vec::new(),
            edge_bounds: Vec::new(),
            direction: LayoutDirection::default(),
            edge_style: EdgeStyle::default(),
            minimap: true,
            viewport: Viewport::default(),
            needs_layout: true,
            pending: Some(PendingView::Initial),
            animation: None,
            primary: None,
            hovered: None,
            context_target: None,
            hover_start: 0.0,
            drag: DragMode::None,
            band_end: None,
            editable: false,
            moving: Vec::new(),
            drop: None,
            drop_new: false,
            edge_since: None,
            hovered_dot: None,
            connect_end: None,
            text_bucket: None,
            last_zoom: 0.0,
            zoom_changed_at: 0.0,
            galleys: GalleyCache::default(),
            visible: Vec::new(),
            shapes: Vec::new(),
            points: Vec::new(),
            size: Vec2::ZERO,
            fonts: CardFonts::default(),
            minimap_cache: None,
        }
    }

    /// The tree.
    pub fn tree(&self) -> &CanvasTree {
        &self.tree
    }

    /// The tree, for changes of the decorations (status, markers,
    /// selection). Do not change the structure or the text: use
    /// [`CanvasState::replace_tree`] for that.
    pub fn tree_mut(&mut self) -> &mut CanvasTree {
        &mut self.tree
    }

    /// Replaces the tree, for example after a SubTree expands. The
    /// decorations and the selection carry over by [`NodeSource`]. If
    /// `anchor` is a node of both trees, it stays at the same place on the
    /// screen; else the view does not move.
    pub fn replace_tree(&mut self, mut tree: CanvasTree, anchor: Option<NodeSource>) {
        tree.copy_decorations_from(&self.tree);
        tree.copy_measurements_from(&self.tree);
        let primary_source = self
            .primary
            .and_then(|id| self.tree.get(id))
            .map(|node| node.source);
        if let Some(source) = anchor
            && let Some(old) = self.tree.find_source(source)
            && let Some(rect) = self.layout.rect(old)
        {
            let screen = self.viewport.to_screen(Pos2::ZERO, rect.min);
            self.pending = Some(PendingView::Anchor {
                source,
                offset: screen.to_vec2(),
            });
        }
        self.primary = primary_source.and_then(|source| tree.find_source(source));
        self.hovered = None;
        self.context_target = None;
        if matches!(self.drag, DragMode::Move { .. }) {
            self.drag = DragMode::None;
        }
        self.moving.clear();
        self.drop = None;
        self.stats.total_nodes = tree.len();
        self.tree = tree;
        self.needs_layout = true;
        self.galleys.clear();
    }

    /// The layout direction.
    pub fn direction(&self) -> LayoutDirection {
        self.direction
    }

    /// Sets the layout direction. The tree is laid out again and fits the
    /// view.
    pub fn set_direction(&mut self, direction: LayoutDirection) {
        if direction != self.direction {
            self.direction = direction;
            self.needs_layout = true;
            self.pending = Some(PendingView::Initial);
        }
    }

    /// The edge style.
    pub fn edge_style(&self) -> EdgeStyle {
        self.edge_style
    }

    /// Sets the edge style.
    pub fn set_edge_style(&mut self, style: EdgeStyle) {
        self.edge_style = style;
    }

    /// True if the minimap shows when the tree does not fit the view.
    pub fn minimap(&self) -> bool {
        self.minimap
    }

    /// Shows or hides the minimap.
    pub fn set_minimap(&mut self, show: bool) {
        self.minimap = show;
    }

    /// The viewport.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Sets the viewport (stops an animation).
    pub fn set_viewport(&mut self, viewport: Viewport) {
        if viewport.is_valid() {
            self.viewport = viewport;
            self.animation = None;
        }
    }

    /// The layout of the last frame.
    pub fn layout(&self) -> &TreeLayout {
        &self.layout
    }

    /// True if the tree must be measured and laid out before the next
    /// frame.
    pub fn needs_layout(&self) -> bool {
        self.needs_layout
    }

    /// The statistics of the last frame.
    pub fn stats(&self) -> CanvasStats {
        self.stats
    }

    /// The node that keyboard navigation starts from.
    pub fn primary(&self) -> Option<CanvasNodeId> {
        self.primary
    }

    /// The node under the pointer.
    pub fn hovered(&self) -> Option<CanvasNodeId> {
        self.hovered
    }

    /// Fits the whole tree in the view.
    pub fn fit_to_view(&mut self, animate: bool) {
        self.pending = Some(PendingView::Fit { animate });
    }

    /// Centres the view on a node and selects it. If the zoom is too small
    /// to read the node, the view zooms in to 100 %. Breakpoints use this
    /// to show the node that stopped.
    pub fn focus_on(&mut self, id: CanvasNodeId, animate: bool) {
        if self.tree.get(id).is_some() {
            self.tree.select_only(id);
            self.primary = Some(id);
            self.pending = Some(PendingView::Focus { id, animate });
        }
    }

    /// Multiplies the zoom by a factor, around the centre of the view.
    /// The zoom does not go below [`CanvasState::min_zoom`].
    pub fn zoom_by(&mut self, factor: f32) {
        let center = (self.size * 0.5).to_pos2();
        let mut target = self.animation.map_or(self.viewport, |a| a.to);
        let min = self.min_zoom();
        let factor = factor.max(min / target.zoom);
        target.zoom_around(Pos2::ZERO, center, factor);
        self.animate_to(target, SHORT_ANIMATION_SECONDS);
    }

    /// The smallest zoom: the zoom at which the whole tree fits in the
    /// view (at most 100 %). The user cannot zoom out further.
    pub fn min_zoom(&self) -> f32 {
        if self.size.x <= 0.0 || self.size.y <= 0.0 || self.layout.rects.is_empty() {
            return viewport::MIN_ZOOM;
        }
        let fit = Viewport::fit(
            self.layout.bounds,
            self.size,
            viewport::FIT_MARGIN,
            viewport::FIT_MAX_ZOOM,
        );
        fit.zoom.clamp(viewport::MIN_ZOOM, viewport::FIT_MAX_ZOOM)
    }

    /// Zooms in one step.
    pub fn zoom_in(&mut self) {
        self.zoom_by(viewport::ZOOM_STEP);
    }

    /// Zooms out one step.
    pub fn zoom_out(&mut self) {
        self.zoom_by(1.0 / viewport::ZOOM_STEP);
    }

    /// Sets the zoom to 100 %, around the centre of the view.
    pub fn zoom_reset(&mut self) {
        let factor = 1.0 / self.viewport.zoom;
        self.zoom_by(factor);
    }

    /// Selects all nodes.
    pub fn select_all(&mut self) -> bool {
        self.tree.select_all()
    }

    /// Clears the selection.
    pub fn clear_selection(&mut self) -> bool {
        self.primary = None;
        self.tree.clear_selection()
    }

    /// Measures and lays out the tree with a given text measure. The
    /// canvas calls this itself with the egui fonts; tests can call it with
    /// a [`card::FixedMeasure`].
    pub fn prepare_with(&mut self, measure: &mut dyn TextMeasure) {
        card::measure_tree(&mut self.tree, measure);
        self.relayout();
    }

    fn relayout(&mut self) {
        self.minimap_cache = None;
        self.layout = layout::layout(&self.tree, self.direction);
        let bounds = layout::subtree_bounds(&self.tree, &self.layout);
        self.groups = self
            .tree
            .ids()
            .filter(|id| {
                self.tree.get(*id).is_some_and(|node| {
                    node.subtree.as_ref().is_some_and(|s| s.expanded) && !node.children.is_empty()
                })
            })
            .map(|id| (id, bounds[id.index()]))
            .collect();
        self.edge_bounds = self
            .tree
            .ids()
            .map(|id| layout::edge_bounds(&self.tree, &self.layout, id))
            .collect();
        self.needs_layout = false;
        self.stats.total_nodes = self.tree.len();
    }

    fn animate_to(&mut self, target: Viewport, duration: f32) {
        self.pending = None;
        self.animation = Some(ViewAnimation {
            from: self.viewport,
            to: target,
            start: f64::NAN,
            duration,
        });
    }

    /// Applies a pending view change. Needs the canvas size.
    fn apply_pending(&mut self, size: Vec2) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let target = match pending {
            PendingView::Initial => {
                self.viewport = initial_view(&self.layout, self.direction, size);
                self.animation = None;
                return;
            }
            PendingView::Fit { animate } => {
                let target = Viewport::fit(
                    self.layout.bounds,
                    size,
                    viewport::FIT_MARGIN,
                    viewport::FIT_MAX_ZOOM,
                );
                if !animate {
                    self.viewport = target;
                    self.animation = None;
                    return;
                }
                target
            }
            PendingView::Focus { id, animate } => {
                let Some(rect) = self.layout.rect(id) else {
                    return;
                };
                let zoom = if self.viewport.zoom < 0.6 {
                    1.0
                } else {
                    self.viewport.zoom
                };
                let target = Viewport::centered_on(rect.center(), size, zoom);
                if !animate {
                    self.viewport = target;
                    self.animation = None;
                    return;
                }
                target
            }
            PendingView::Reveal { id } => {
                let Some(rect) = self.layout.rect(id) else {
                    return;
                };
                let target = self.viewport.reveal(rect, size, 24.0);
                if target == self.viewport {
                    return;
                }
                self.animate_to(target, SHORT_ANIMATION_SECONDS);
                return;
            }
            PendingView::Anchor { source, offset } => {
                if let Some(id) = self.tree.find_source(source)
                    && let Some(rect) = self.layout.rect(id)
                {
                    let now = self.viewport.to_screen(Pos2::ZERO, rect.min);
                    self.viewport.pan_by(offset - now.to_vec2());
                }
                return;
            }
        };
        self.animate_to(target, ANIMATION_SECONDS);
    }

    fn step_animation(&mut self, now: f64) -> bool {
        let Some(animation) = &mut self.animation else {
            return false;
        };
        if animation.start.is_nan() {
            animation.start = now;
        }
        let t = ((now - animation.start) as f32 / animation.duration.max(1e-3)).clamp(0.0, 1.0);
        let eased = viewport::ease_out_cubic(t);
        self.viewport = Viewport::lerp(animation.from, animation.to, self.size, eased);
        if t >= 1.0 {
            self.viewport = animation.to;
            self.animation = None;
        }
        true
    }

    /// The world rectangle of a node in screen coordinates for a canvas at
    /// `origin`.
    pub fn node_screen_rect(&self, origin: Pos2, id: CanvasNodeId) -> Option<Rect> {
        self.layout
            .rect(id)
            .map(|rect| self.viewport.rect_to_screen(origin, rect))
    }
}

/// The zoom of the first view of a large tree.
const READABLE_ZOOM: f32 = 0.85;

/// The first view of a tree: the whole tree if it fits at a zoom where the
/// labels are readable; else the root at [`READABLE_ZOOM`], at the top
/// (top-down) or at the left (left-right) of the canvas.
fn initial_view(layout: &TreeLayout, direction: LayoutDirection, size: Vec2) -> Viewport {
    let fit = Viewport::fit(
        layout.bounds,
        size,
        viewport::FIT_MARGIN,
        viewport::FIT_MAX_ZOOM,
    );
    if Detail::for_zoom(fit.zoom) == Detail::Full {
        return fit;
    }
    let Some(root) = layout.rects.first() else {
        return fit;
    };
    let zoom = READABLE_ZOOM;
    let margin = viewport::FIT_MARGIN;
    let pan = match direction {
        LayoutDirection::TopDown => vec2(
            size.x * 0.5 - root.center().x * zoom,
            margin - root.min.y * zoom,
        ),
        LayoutDirection::LeftRight => vec2(
            margin - root.min.x * zoom,
            size.y * 0.5 - root.center().y * zoom,
        ),
    };
    Viewport::new(pan, zoom)
}

/// Measures text with the egui fonts (sum of glyph advances).
struct EguiMeasure<'a, 'b> {
    fonts: &'a mut egui::epaint::text::FontsView<'b>,
    ids: [FontId; 5],
}

impl EguiMeasure<'_, '_> {
    fn font(&self, kind: TextKind) -> &FontId {
        &self.ids[kind_index(kind)]
    }
}

fn kind_index(kind: TextKind) -> usize {
    match kind {
        TextKind::Label => 0,
        TextKind::Type => 1,
        TextKind::PortName => 2,
        TextKind::PortValue => 3,
        TextKind::Note => 4,
    }
}

/// The font families of the cards. The canvas uses the bundled Roboto
/// families when the app installed them (see [`crate::fonts`]), else the
/// default proportional family, so that the widget also works without the
/// app fonts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CardFonts {
    label: FontFamily,
    italic: FontFamily,
}

impl Default for CardFonts {
    fn default() -> Self {
        CardFonts {
            label: FontFamily::Proportional,
            italic: FontFamily::Proportional,
        }
    }
}

impl CardFonts {
    fn detect(ctx: &egui::Context) -> Self {
        ctx.fonts(|f| {
            let families = &f.definitions().families;
            let pick = |name: &str| {
                let family = FontFamily::Name(name.into());
                if families.contains_key(&family) {
                    family
                } else {
                    FontFamily::Proportional
                }
            };
            CardFonts {
                label: pick(fonts::MEDIUM),
                italic: pick(fonts::ITALIC),
            }
        })
    }

    fn font(&self, kind: TextKind, size: f32) -> FontId {
        let family = match kind {
            TextKind::Label => self.label.clone(),
            TextKind::PortValue => FontFamily::Monospace,
            TextKind::Type | TextKind::PortName | TextKind::Note => FontFamily::Proportional,
        };
        FontId::new(size, family)
    }
}

impl TextMeasure for EguiMeasure<'_, '_> {
    fn width(&mut self, text: &str, kind: TextKind) -> f32 {
        let font = self.font(kind).clone();
        text.chars().map(|c| self.fonts.glyph_width(&font, c)).sum()
    }

    fn char_width(&mut self, c: char, kind: TextKind) -> f32 {
        let font = self.font(kind).clone();
        self.fonts.glyph_width(&font, c)
    }
}

/// Extra entries at the top of the context menu. The owner of the canvas
/// adds mode-specific actions (for example breakpoints in live mode, or
/// edit commands). The function gets the node under the pointer at the
/// right-click (`None` on an empty place) and returns true if it added
/// entries.
pub type NodeMenu<'a> = dyn FnMut(&mut Ui, Option<&CanvasNode>) -> bool + 'a;

/// The tree canvas widget. See the module documentation.
pub struct TreeCanvas<'a> {
    state: &'a mut CanvasState,
    palette: &'a CanvasPalette,
    id_salt: Id,
    node_menu: Option<&'a mut NodeMenu<'a>>,
    editable: bool,
}

impl std::fmt::Debug for TreeCanvas<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeCanvas")
            .field("id_salt", &self.id_salt)
            .field("node_menu", &self.node_menu.is_some())
            .finish_non_exhaustive()
    }
}

impl<'a> TreeCanvas<'a> {
    /// A canvas for a state, drawn with a palette.
    pub fn new(state: &'a mut CanvasState, palette: &'a CanvasPalette) -> Self {
        TreeCanvas {
            state,
            palette,
            id_salt: Id::new("tree_canvas"),
            node_menu: None,
            editable: false,
        }
    }

    /// Lets the user move nodes and drop new nodes (edit mode). See the
    /// module documentation.
    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    /// Adds entries at the top of the context menu of a node. The function
    /// gets the node under the pointer at the right-click.
    pub fn node_menu(mut self, menu: &'a mut NodeMenu<'a>) -> Self {
        self.node_menu = Some(menu);
        self
    }

    /// Sets the ID salt. Use a different salt for each canvas on screen.
    pub fn id_salt(mut self, salt: impl std::hash::Hash + std::fmt::Debug) -> Self {
        self.id_salt = Id::new(salt);
        self
    }

    /// Shows the canvas in all the available space.
    pub fn show(self, ui: &mut Ui) -> CanvasOutput {
        let TreeCanvas {
            state,
            palette,
            id_salt,
            node_menu,
            editable,
        } = self;
        let id = ui.make_persistent_id(id_salt);
        let available = ui.available_rect_before_wrap();
        let rect = Rect::from_min_size(available.min, available.size().max(vec2(64.0, 64.0)));
        ui.advance_cursor_after_rect(rect);
        let response = ui.interact(rect, id, Sense::click_and_drag());
        state.size = rect.size();

        let card_fonts = CardFonts::detect(ui.ctx());
        if card_fonts != state.fonts {
            state.fonts = card_fonts;
            state.needs_layout = true;
            state.galleys.clear();
            // Measure all cards again with the new fonts.
            for node in state.tree.nodes_mut() {
                node.card.size = Vec2::ZERO;
            }
        }
        if state.needs_layout {
            let ids = [
                TextKind::Label,
                TextKind::Type,
                TextKind::PortName,
                TextKind::PortValue,
                TextKind::Note,
            ]
            .map(|kind| state.fonts.font(kind, kind.size()));
            ui.ctx().fonts_mut(|fonts| {
                let mut measure = EguiMeasure { fonts, ids };
                card::measure_missing(&mut state.tree, &mut measure);
            });
            state.relayout();
        }
        state.apply_pending(rect.size());

        state.editable = editable;
        if !editable && !state.moving.is_empty() {
            state.moving.clear();
            state.drag = DragMode::None;
        }
        let mut events = Vec::new();
        handle_input(state, ui, &response, rect, &mut events);
        if editable {
            handle_palette_drop(state, ui, &response, rect, &mut events);
        } else {
            state.drop_new = false;
        }

        let now = ui.input(|i| i.time);
        if state.step_animation(now) {
            ui.ctx().request_repaint();
        }

        let started = std::time::Instant::now();
        paint_canvas(state, ui, rect, palette, response.has_focus());
        state.stats.paint_ms = started.elapsed().as_secs_f32() * 1000.0;

        if state.minimap {
            minimap(state, ui, rect, palette, id);
        }
        describe_for_accesskit(state, ui, rect, id);
        show_tooltip(state, ui, &response, id, now, palette);

        // Context menu, for the node under the pointer at the right-click.
        let target = state.context_target;
        response.context_menu(|ui| {
            context_menu(state, ui, target, &mut events, node_menu);
        });

        CanvasOutput { response, events }
    }
}

/// How a click changes the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectMode {
    Replace,
    Add,
    Toggle,
}

fn select_mode(modifiers: Modifiers) -> SelectMode {
    if modifiers.command {
        SelectMode::Toggle
    } else if modifiers.shift {
        SelectMode::Add
    } else {
        SelectMode::Replace
    }
}

fn handle_input(
    state: &mut CanvasState,
    ui: &Ui,
    response: &Response,
    rect: Rect,
    events: &mut Vec<CanvasEvent>,
) {
    let origin = rect.min;
    let (pointer, modifiers, zoom_delta, scroll, space) = ui.input(|i| {
        (
            i.pointer.hover_pos(),
            i.modifiers,
            i.zoom_delta(),
            i.smooth_scroll_delta(),
            i.key_down(Key::Space),
        )
    });

    // Zoom and scroll.
    if response.contains_pointer() {
        if (zoom_delta - 1.0).abs() > 1e-4 {
            let anchor = pointer.unwrap_or(rect.center());
            state.animation = None;
            // Not smaller than the zoom at which the whole tree fits.
            let min = state.min_zoom();
            let factor = if zoom_delta < 1.0 {
                zoom_delta.max(min / state.viewport.zoom).min(1.0)
            } else {
                zoom_delta
            };
            state.viewport.zoom_around(origin, anchor, factor);
        }
        if scroll != Vec2::ZERO {
            state.animation = None;
            state.viewport.pan_by(scroll);
        }
    }

    // Hover.
    let world_pointer = pointer.map(|p| state.viewport.to_world(origin, p));
    let hovered = if response.hovered() && state.drag == DragMode::None {
        world_pointer.and_then(|p| hit::hit_node(&state.layout.rects, p, 2.0 / state.viewport.zoom))
    } else {
        None
    };
    if hovered != state.hovered {
        state.hovered = hovered;
        state.hover_start = ui.input(|i| i.time);
    }
    let dot_under = |state: &CanvasState, p: Option<Pos2>| {
        let zoom = state.viewport.zoom;
        if !state.editable || Detail::for_zoom(zoom) == Detail::Boxes {
            return None;
        }
        connect::dot_at(
            &state.tree,
            &state.layout.rects,
            state.direction,
            p?,
            connect::grab_px(zoom) / zoom,
        )
    };
    state.hovered_dot = if response.hovered() && state.drag == DragMode::None {
        dot_under(state, world_pointer)
    } else {
        None
    };
    if state.hovered_dot.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }

    // Drags.
    if response.drag_started() {
        let press = ui.input(|i| i.pointer.press_origin()).or(pointer);
        let press_world = press.map(|p| state.viewport.to_world(origin, p));
        let on_node = press_world.and_then(|p| hit::hit_node(&state.layout.rects, p, 0.0));
        let on_dot = dot_under(state, press_world)
            .filter(|_| !space && response.dragged_by(PointerButton::Primary));
        let moving = match on_node {
            Some(id) if state.editable && !space && response.dragged_by(PointerButton::Primary) => {
                drop::moving_nodes(&state.tree, id)
            }
            _ => Vec::new(),
        };
        state.drag = if let Some(from) = on_dot {
            DragMode::Connect { from }
        } else if let (Some(id), false) = (on_node, moving.is_empty()) {
            // Grabbing a node that is not selected selects it.
            if !state.tree.is_selected(id) && click_select(state, Some(id), SelectMode::Replace) {
                events.push(CanvasEvent::SelectionChanged);
            }
            state.moving = moving;
            DragMode::Move {
                start: press_world.unwrap_or(Pos2::ZERO),
            }
        } else if response.dragged_by(PointerButton::Middle)
            || (response.dragged_by(PointerButton::Primary) && (space || on_node.is_some()))
        {
            DragMode::Pan
        } else if response.dragged_by(PointerButton::Primary) {
            DragMode::Band {
                start: press.map_or(Pos2::ZERO, |p| state.viewport.to_world(origin, p)),
                additive: modifiers.shift || modifiers.command,
            }
        } else {
            DragMode::None
        };
        state.animation = None;
    }
    if matches!(state.drag, DragMode::Move { .. } | DragMode::Connect { .. })
        && ui.input(|i| i.key_pressed(Key::Escape))
    {
        // Escape cancels a move or a connection.
        state.drag = DragMode::None;
        state.moving.clear();
        state.drop = None;
        state.connect_end = None;
        state.edge_since = None;
        state.band_end = None;
    }
    match state.drag {
        DragMode::Pan if response.dragged() => {
            state.viewport.pan_by(response.drag_delta());
        }
        DragMode::Band { .. } if response.dragged() => {
            state.band_end = world_pointer;
        }
        DragMode::Move { .. } if response.dragged() => {
            state.band_end = world_pointer;
            state.drop = world_pointer.map(|p| {
                drop::drop_spot(
                    &state.tree,
                    &state.layout.rects,
                    state.direction,
                    p,
                    &state.moving,
                    drop::SNAP_DISTANCE / state.viewport.zoom,
                )
            });
            if let Some(p) = pointer {
                edge_pan(state, ui, rect, p);
            }
        }
        DragMode::Connect { from } if response.dragged() => {
            state.band_end = world_pointer;
            state.connect_end = world_pointer.map(|p| {
                connect::connect_end(&state.tree, &state.layout.rects, state.direction, from, p)
            });
            if let Some(p) = pointer {
                edge_pan(state, ui, rect, p);
            }
        }
        _ => {}
    }
    if response.drag_stopped() {
        if let DragMode::Move { .. } = state.drag
            && let Some(spot) = state.drop
            && spot != DropSpot::Invalid
            && !state.moving.is_empty()
        {
            events.push(CanvasEvent::MoveNodes {
                nodes: std::mem::take(&mut state.moving),
                spot,
            });
        }
        if let DragMode::Connect { from } = state.drag {
            match state.connect_end.take() {
                Some(ConnectEnd::Connect(child)) => events.push(CanvasEvent::MoveNodes {
                    nodes: vec![child],
                    spot: DropSpot::Node {
                        target: from,
                        placement: DropPlacement::Inside,
                    },
                }),
                Some(ConnectEnd::Add(spot)) => events.push(CanvasEvent::AddChild {
                    spot,
                    at: pointer.unwrap_or(rect.center()),
                }),
                None => {}
            }
        }
        state.moving.clear();
        state.drop = None;
        state.connect_end = None;
        state.edge_since = None;
        if let DragMode::Band { start, additive } = state.drag
            && let Some(end) = state.band_end.or(world_pointer)
        {
            let band = hit::rect_from_corners(start, end);
            let inside = hit::nodes_in_rect(&state.layout.rects, band);
            let mut changed = false;
            if !additive {
                changed |= state.tree.clear_selection();
            }
            for id in &inside {
                changed |= state.tree.set_selected(*id, true);
            }
            if let Some(first) = inside.first() {
                state.primary = Some(*first);
            }
            if changed {
                events.push(CanvasEvent::SelectionChanged);
            }
        }
        state.drag = DragMode::None;
        state.band_end = None;
    }

    // Clicks. A focused widget also "clicks" on Space and Enter; the
    // canvas uses those keys itself, so it reads only real pointer clicks.
    let clicked = response.clicked_by(PointerButton::Primary);
    if clicked || response.secondary_clicked() {
        response.request_focus();
    }
    if clicked && let Some(from) = dot_under(state, world_pointer) {
        // A click on a dot asks for a new last child.
        events.push(CanvasEvent::AddChild {
            spot: DropSpot::Node {
                target: from,
                placement: DropPlacement::Inside,
            },
            at: pointer.unwrap_or(rect.center()),
        });
    } else if clicked {
        let target = world_pointer.and_then(|p| hit::hit_node(&state.layout.rects, p, 0.0));
        if click_select(state, target, select_mode(modifiers)) {
            events.push(CanvasEvent::SelectionChanged);
        }
    }
    if response.secondary_clicked() {
        // Right-click selects the node under the pointer (if it is not
        // selected), so that the context menu acts on it.
        let target = world_pointer.and_then(|p| hit::hit_node(&state.layout.rects, p, 0.0));
        state.context_target = target;
        if let Some(id) = target
            && !state.tree.is_selected(id)
            && click_select(state, Some(id), SelectMode::Replace)
        {
            events.push(CanvasEvent::SelectionChanged);
        }
    }
    // egui counts a double click soon after another click as a triple
    // click; both count here.
    if (response.double_clicked() || response.triple_clicked())
        && let Some(id) = world_pointer.and_then(|p| hit::hit_node(&state.layout.rects, p, 0.0))
        && let Some(node) = state.tree.get(id)
    {
        if node.subtree.as_ref().is_some_and(|s| s.can_expand) {
            events.push(CanvasEvent::ToggleSubtree(node.source));
        } else if can_fold(node) {
            events.push(CanvasEvent::ToggleFold(node.source));
        } else {
            events.push(CanvasEvent::Activated(id));
        }
    }

    // Accessibility actions on the node items (screen readers, tests).
    let mut clicked_items = Vec::new();
    ui.input(|i| {
        for event in &i.events {
            if let egui::Event::AccessKitActionRequest(request) = event
                && matches!(
                    request.action,
                    egui::accesskit::Action::Click | egui::accesskit::Action::Focus
                )
            {
                clicked_items.push(request.target_node);
            }
        }
    });
    if !clicked_items.is_empty() {
        for id in state.tree.ids() {
            let item = node_item_id(response.id, id).accesskit_id();
            if clicked_items.contains(&item) && click_select(state, Some(id), SelectMode::Replace) {
                events.push(CanvasEvent::SelectionChanged);
            }
        }
    }

    // Keyboard.
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                response.id,
                EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                    tab: false,
                },
            );
        });
        let keys = ui.input(|i| {
            [
                Key::ArrowUp,
                Key::ArrowDown,
                Key::ArrowLeft,
                Key::ArrowRight,
                Key::Escape,
                Key::Enter,
                Key::Home,
            ]
            .map(|k| i.key_pressed(k))
        });
        let [up, down, left, right, escape, enter, home] = keys;
        let step = match (state.direction, up, down, left, right) {
            (LayoutDirection::TopDown, true, ..) => Some(NavStep::Parent),
            (LayoutDirection::TopDown, _, true, ..) => Some(NavStep::FirstChild),
            (LayoutDirection::TopDown, _, _, true, _) => Some(NavStep::PreviousSibling),
            (LayoutDirection::TopDown, _, _, _, true) => Some(NavStep::NextSibling),
            (LayoutDirection::LeftRight, true, ..) => Some(NavStep::PreviousSibling),
            (LayoutDirection::LeftRight, _, true, ..) => Some(NavStep::NextSibling),
            (LayoutDirection::LeftRight, _, _, true, _) => Some(NavStep::Parent),
            (LayoutDirection::LeftRight, _, _, _, true) => Some(NavStep::FirstChild),
            _ => None,
        };
        let target = if home {
            state.tree.root()
        } else if let Some(step) = step {
            match state.primary {
                Some(from) => state.tree.navigate(from, step),
                None => state.tree.root(),
            }
        } else {
            None
        };
        if let Some(target) = target {
            let mode = if modifiers.shift {
                SelectMode::Add
            } else {
                SelectMode::Replace
            };
            if click_select(state, Some(target), mode) {
                events.push(CanvasEvent::SelectionChanged);
            }
            state.pending = Some(PendingView::Reveal { id: target });
        }
        if escape && state.clear_selection() {
            events.push(CanvasEvent::SelectionChanged);
        }
        if enter && let Some(node) = state.primary.and_then(|id| state.tree.get(id)) {
            if node.subtree.as_ref().is_some_and(|s| s.can_expand) {
                events.push(CanvasEvent::ToggleSubtree(node.source));
            } else if let Some(id) = state.primary {
                events.push(CanvasEvent::Activated(id));
            }
        }
    }
}

/// Changes the selection for a click on `target` (or on the background).
/// Returns true if the selection changed.
fn click_select(state: &mut CanvasState, target: Option<CanvasNodeId>, mode: SelectMode) -> bool {
    match (target, mode) {
        (None, SelectMode::Replace) => state.clear_selection(),
        (None, _) => false,
        (Some(id), SelectMode::Replace) => {
            state.primary = Some(id);
            state.tree.select_only(id)
        }
        (Some(id), SelectMode::Add) => {
            state.primary = Some(id);
            state.tree.set_selected(id, true)
        }
        (Some(id), SelectMode::Toggle) => {
            let selected = !state.tree.is_selected(id);
            if selected {
                state.primary = Some(id);
            } else if state.primary == Some(id) {
                state.primary = None;
            }
            state.tree.set_selected(id, selected)
        }
    }
}

/// Shows where a palette drag would add a node, and reports the drop.
fn handle_palette_drop(
    state: &mut CanvasState,
    ui: &Ui,
    response: &Response,
    rect: Rect,
    events: &mut Vec<CanvasEvent>,
) {
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let world = pointer.map(|p| state.viewport.to_world(rect.min, p));
    let spot_at = |state: &CanvasState, p: Pos2| {
        drop::drop_spot(
            &state.tree,
            &state.layout.rects,
            state.direction,
            p,
            &[],
            drop::SNAP_DISTANCE / state.viewport.zoom,
        )
    };
    if let Some(payload) = response.dnd_release_payload::<NewNodePayload>() {
        if let Some(p) = world {
            let spot = spot_at(state, p);
            if spot != DropSpot::Invalid {
                events.push(CanvasEvent::DropNew {
                    type_id: payload.type_id.clone(),
                    spot,
                });
            }
        }
        state.drop_new = false;
        state.drop = None;
        state.edge_since = None;
        state.band_end = None;
        return;
    }
    let hovering = response.dnd_hover_payload::<NewNodePayload>().is_some();
    if hovering && let Some(p) = world {
        state.drop_new = true;
        state.drop = Some(spot_at(state, p));
        state.band_end = Some(p);
        if let Some(screen) = pointer {
            edge_pan(state, ui, rect, screen);
        }
    } else if state.drop_new {
        state.drop_new = false;
        state.drop = None;
        state.edge_since = None;
        state.band_end = None;
    }
}

/// The width of the border zone that pans the view during a drag, in
/// points.
const EDGE_PAN_ZONE: f32 = 28.0;
/// The pan speed at the border, in points per second.
const EDGE_PAN_SPEED: f32 = 600.0;
/// The time that the pointer stays in the border zone before the view
/// pans, in seconds. A drag that only crosses the border does not pan.
const EDGE_PAN_DELAY: f64 = 0.3;

/// Pans the view when the pointer stays near a border during a drag.
fn edge_pan(state: &mut CanvasState, ui: &Ui, rect: Rect, pointer: Pos2) {
    let inner = rect.shrink(EDGE_PAN_ZONE);
    let mut direction = Vec2::ZERO;
    if pointer.x < inner.min.x {
        direction.x = 1.0;
    } else if pointer.x > inner.max.x {
        direction.x = -1.0;
    }
    if pointer.y < inner.min.y {
        direction.y = 1.0;
    } else if pointer.y > inner.max.y {
        direction.y = -1.0;
    }
    if direction == Vec2::ZERO {
        state.edge_since = None;
        return;
    }
    let now = ui.input(|i| i.time);
    let since = *state.edge_since.get_or_insert(now);
    ui.ctx().request_repaint();
    if now - since >= EDGE_PAN_DELAY {
        let dt = ui.input(|i| i.stable_dt).min(0.05);
        state.animation = None;
        state.viewport.pan_by(direction * EDGE_PAN_SPEED * dt);
        ui.ctx().request_repaint();
    }
}

/// The ID of the accessibility item of a node.
fn node_item_id(canvas: Id, node: CanvasNodeId) -> Id {
    canvas.with(("node", node.0))
}

fn context_menu(
    state: &mut CanvasState,
    ui: &mut Ui,
    target: Option<CanvasNodeId>,
    events: &mut Vec<CanvasEvent>,
    node_menu: Option<&mut NodeMenu<'_>>,
) {
    if let Some(menu) = node_menu
        && menu(ui, target.and_then(|id| state.tree.get(id)))
    {
        ui.separator();
    }
    if let Some(node) = target.and_then(|id| state.tree.get(id)) {
        let source = node.source;
        let label = node.label.clone();
        let path = node.path.clone();
        if let Some(info) = node.subtree.as_ref().filter(|s| s.can_expand) {
            let text = if info.expanded {
                "Collapse SubTree"
            } else {
                "Expand SubTree"
            };
            if ui.button(text).clicked() {
                events.push(CanvasEvent::ToggleSubtree(source));
            }
        }
        if can_fold(node) {
            let text = if node.folded.is_some() {
                "Unfold children"
            } else {
                "Fold children"
            };
            if ui
                .button(text)
                .on_hover_text("Hide or show the nodes below this node (double-click)")
                .clicked()
            {
                events.push(CanvasEvent::ToggleFold(source));
            }
        }
        if ui.button("Center on node").clicked()
            && let Some(id) = target
        {
            state.focus_on(id, true);
        }
        if ui.button("Copy name").clicked() {
            ui.ctx().copy_text(label);
        }
        if !path.is_empty() && ui.button("Copy path").clicked() {
            ui.ctx().copy_text(path);
        }
        ui.separator();
    }
    if ui.button("Expand all SubTrees").clicked() {
        events.push(CanvasEvent::SetAllSubtrees(true));
    }
    if ui.button("Collapse all SubTrees").clicked() {
        events.push(CanvasEvent::SetAllSubtrees(false));
    }
    if state.tree.nodes().iter().any(|n| n.folded.is_some()) && ui.button("Unfold all").clicked() {
        events.push(CanvasEvent::UnfoldAll);
    }
    ui.separator();
    if ui.button("Fit to view").clicked() {
        state.fit_to_view(true);
    }
    if ui.button("Select all").clicked() && state.select_all() {
        events.push(CanvasEvent::SelectionChanged);
    }
}

/// Rounds a point to the pixel grid (crisp text).
/// The size of the card text of a frame.
struct TextScale {
    /// The zoom of the text layouts.
    zoom: f32,
    /// The zoom of the view divided by `zoom`: 1 at rest, other values
    /// during a zoom.
    scale: f32,
    /// The size of the font texture, for the texture coordinates of a
    /// scaled text.
    font_texture: [usize; 2],
}

/// Draws a text layout at `pos`. At rest the text is on the pixel grid.
/// During a zoom it is a mesh, scaled around `pos` by [`TextScale::scale`],
/// so that it grows and shrinks smoothly with its card.
fn push_text(
    shapes: &mut Vec<Shape>,
    galley: Arc<Galley>,
    pos: Pos2,
    text: &TextScale,
    color: Color32,
    ppp: f32,
) {
    // The text buckets are 1.5 % apart: at rest the scale is that close
    // to 1.
    if (text.scale - 1.0).abs() < 0.02 {
        shapes.push(Shape::galley(snap(pos, ppp), galley, color));
        return;
    }
    let uv = vec2(
        1.0 / text.font_texture[0].max(1) as f32,
        1.0 / text.font_texture[1].max(1) as f32,
    );
    let mut mesh = Mesh::default();
    for row in &galley.rows {
        let row_mesh = &row.visuals.mesh;
        if row_mesh.is_empty() {
            continue;
        }
        let offset = mesh.vertices.len() as u32;
        mesh.indices
            .extend(row_mesh.indices.iter().map(|index| index + offset));
        mesh.vertices
            .extend(row_mesh.vertices.iter().map(|vertex| egui::epaint::Vertex {
                pos: pos + (row.pos.to_vec2() + vertex.pos.to_vec2()) * text.scale,
                uv: (vertex.uv.to_vec2() * uv).to_pos2(),
                color: if vertex.color == Color32::PLACEHOLDER {
                    color
                } else {
                    vertex.color
                },
            }));
    }
    shapes.push(Shape::mesh(mesh));
}

fn snap(p: Pos2, ppp: f32) -> Pos2 {
    pos2((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp)
}

fn radius(value: f32) -> CornerRadius {
    CornerRadius::same(value.round().clamp(0.0, 255.0) as u8)
}

/// True if a port value is a blackboard reference (`{key}`).
/// True if a node can fold: it has children (or hides them), and it is not
/// a SubTree node (a SubTree expands and collapses instead).
fn can_fold(node: &CanvasNode) -> bool {
    node.subtree.is_none() && (node.folded.is_some() || !node.children.is_empty())
}

fn is_reference(value: &str) -> bool {
    value.starts_with('{') && value.ends_with('}')
}

/// The box around the conditions of a card: a solid outline and a
/// background in the condition colour.
fn condition_box(
    shapes: &mut Vec<Shape>,
    rect: Rect,
    palette: &CanvasPalette,
    alpha: f32,
    zoom: f32,
) {
    let fill = mix(
        palette.card,
        palette.condition,
        if palette.dark { 0.17 } else { 0.10 },
    );
    shapes.push(Shape::rect_filled(
        rect,
        radius(4.0 * zoom),
        fill.gamma_multiply(alpha),
    ));
    shapes.push(Shape::rect_stroke(
        rect,
        radius(4.0 * zoom),
        Stroke::new(
            (1.4 * zoom).clamp(1.0, 2.5),
            palette.condition.gamma_multiply(alpha),
        ),
        StrokeKind::Inside,
    ));
}

/// A frame in the style of the condition box of the cards, for the
/// conditions in tooltips and panels.
pub fn condition_frame(palette: &CanvasPalette) -> egui::Frame {
    egui::Frame::new()
        .fill(mix(
            palette.card,
            palette.condition,
            if palette.dark { 0.17 } else { 0.10 },
        ))
        .stroke(Stroke::new(1.4, palette.condition))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(8, 6))
}

/// The colours of a port or condition line.
#[derive(Clone, Copy)]
struct EntryColors {
    name: Color32,
    value: Color32,
    reference: Color32,
}

/// The layout of line `part` of a port or condition. The first line has
/// the name; the next lines start below the first character of the value
/// (an invisible copy of the name keeps the place).
fn entry_line_job(
    painter: &egui::Painter,
    fonts: &CardFonts,
    entry: &CardEntry,
    part: usize,
    colors: EntryColors,
    text_zoom: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    let name_format = TextFormat {
        font_id: fonts.font(TextKind::PortName, card::PORT_SIZE * text_zoom),
        color: if part == 0 {
            colors.name
        } else {
            Color32::TRANSPARENT
        },
        ..TextFormat::default()
    };
    job.append(&entry.name, 0.0, name_format.clone());
    job.append(card::PORT_SEPARATOR, 0.0, name_format);
    let value = entry.value.get(part).map_or("", String::as_str);
    let reference = entry.value.len() == 1 && is_reference(value);
    job.append(
        value,
        0.0,
        TextFormat {
            font_id: fonts.font(TextKind::PortValue, card::PORT_SIZE * text_zoom),
            color: if reference {
                colors.reference
            } else {
                colors.value
            },
            ..TextFormat::default()
        },
    );
    painter.layout_job(job)
}

/// For each text line of ports or conditions: the indent (world units, an
/// estimate) and the number of characters. For the bars of small cards.
fn entry_bars(entries: &[CardEntry]) -> impl Iterator<Item = (f32, usize)> + '_ {
    entries.iter().flat_map(|entry| {
        let name = entry.name.chars().count() + 2;
        let indent = name as f32 * 6.0;
        entry.value.iter().enumerate().map(move |(part, line)| {
            if part == 0 {
                (0.0, name + line.chars().count())
            } else {
                (indent, line.chars().count())
            }
        })
    })
}

fn palette_key(palette: &CanvasPalette) -> u64 {
    let c = palette.text.to_array();
    u64::from(palette.dark) | (u64::from(u32::from_le_bytes(c)) << 1)
}

fn paint_canvas(
    state: &mut CanvasState,
    ui: &Ui,
    rect: Rect,
    palette: &CanvasPalette,
    has_focus: bool,
) {
    let painter = ui.painter_at(rect);
    let origin = rect.min;
    let zoom = state.viewport.zoom;
    let detail = Detail::for_zoom(zoom);
    let ppp = ui.ctx().pixels_per_point();
    let mut shapes = std::mem::take(&mut state.shapes);
    shapes.clear();

    // Background and dots.
    shapes.push(Shape::rect_filled(
        rect,
        CornerRadius::ZERO,
        palette.background,
    ));
    background_dots(&mut shapes, rect, state.viewport, palette.grid);

    // Margin for markers outside the cards.
    let view = state
        .viewport
        .visible_world(rect)
        .expand(24.0 / zoom.max(viewport::MIN_ZOOM));
    hit::visible_into(&state.layout.rects, view, &mut state.visible);

    // Expanded SubTree regions.
    if detail != Detail::Boxes {
        for (_, bounds) in &state.groups {
            let region = bounds.expand(14.0);
            if !region.intersects(view) {
                continue;
            }
            let screen = state.viewport.rect_to_screen(origin, region);
            let r = radius(10.0 * zoom);
            shapes.push(Shape::rect_filled(screen, r, palette.group_fill));
            shapes.push(Shape::rect_stroke(
                screen,
                r,
                Stroke::new(1.0, palette.group_border),
                StrokeKind::Inside,
            ));
        }
    }

    paint_edges(state, &mut shapes, origin, view, palette, detail);

    // The text size. During a zoom the text keeps its layout and scales
    // with the cards (see [`push_text`]), so that it does not jump from one
    // size to the next; when the zoom stops, it gets a new layout at the
    // new size.
    let now = ui.input(|i| i.time);
    if zoom != state.last_zoom {
        state.last_zoom = zoom;
        state.zoom_changed_at = now;
    }
    let settled = now - state.zoom_changed_at >= TEXT_SETTLE_SECONDS;
    if !settled {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(TEXT_SETTLE_SECONDS));
    }
    let wanted = viewport::zoom_bucket(zoom);
    let bucket = match state.text_bucket {
        // Far from the zoom of the layout, the scaled text would be blurred.
        Some(bucket)
            if !settled && (0.7..=1.4).contains(&(zoom / viewport::bucket_zoom(bucket))) =>
        {
            bucket
        }
        _ => wanted,
    };
    state.text_bucket = Some(bucket);
    let text = TextScale {
        zoom: viewport::bucket_zoom(bucket),
        scale: zoom / viewport::bucket_zoom(bucket),
        font_texture: ui.ctx().fonts(|f| f.font_image_size()),
    };
    state.galleys.prepare(bucket, palette_key(palette));

    let visible = std::mem::take(&mut state.visible);
    for &index in &visible {
        paint_card(
            state,
            ui,
            &mut shapes,
            origin,
            index,
            palette,
            detail,
            &text,
            ppp,
            has_focus,
        );
    }
    if state.editable && detail != Detail::Boxes {
        paint_dots(state, &mut shapes, origin, &visible, palette);
    }
    state.visible = visible;

    paint_drop(state, ui, &mut shapes, origin, palette);
    paint_connection(state, ui, &mut shapes, origin, palette);

    // Rubber band.
    if let (DragMode::Band { start, .. }, Some(end)) = (state.drag, state.band_end) {
        let band = state
            .viewport
            .rect_to_screen(origin, hit::rect_from_corners(start, end));
        shapes.push(Shape::rect_filled(
            band,
            CornerRadius::same(2),
            palette.band_fill,
        ));
        shapes.push(Shape::rect_stroke(
            band,
            CornerRadius::same(2),
            Stroke::new(1.0, palette.accent),
            StrokeKind::Inside,
        ));
    }

    // Focus ring of the canvas (keyboard focus).
    if has_focus && ui.memory(|m| m.focused().is_some()) && state.primary.is_none() {
        shapes.push(Shape::rect_stroke(
            rect.shrink(1.0),
            CornerRadius::ZERO,
            Stroke::new(1.0, palette.accent.gamma_multiply(0.35)),
            StrokeKind::Inside,
        ));
    }

    painter.extend(shapes.drain(..));
    state.shapes = shapes;
    state.stats.visible_nodes = state.visible.len();
    state.stats.detail = detail;
    state.stats.cached_galleys = state.galleys.map.len();
}

/// The feedback of a move or a palette drag: ghosts of the moved cards,
/// the drop marker and a short hint next to the pointer.
fn paint_drop(
    state: &CanvasState,
    ui: &Ui,
    shapes: &mut Vec<Shape>,
    origin: Pos2,
    palette: &CanvasPalette,
) {
    let moving = matches!(state.drag, DragMode::Move { .. });
    if !moving && !state.drop_new {
        return;
    }
    let zoom = state.viewport.zoom;
    let corner = radius(6.0 * zoom);
    if let (DragMode::Move { start }, Some(end)) = (state.drag, state.band_end) {
        let delta = end - start;
        for id in &state.moving {
            if let Some(r) = state.layout.rect(*id) {
                let screen = state.viewport.rect_to_screen(origin, r.translate(delta));
                shapes.push(Shape::rect_filled(screen, corner, palette.band_fill));
                shapes.push(Shape::rect_stroke(
                    screen,
                    corner,
                    Stroke::new(1.5, palette.accent),
                    StrokeKind::Inside,
                ));
            }
        }
    }
    let Some(spot) = state.drop else {
        return;
    };
    let target_label = |id: CanvasNodeId| {
        state
            .tree
            .get(id)
            .map(|n| n.label.clone())
            .unwrap_or_default()
    };
    let (hint, color) = match spot {
        DropSpot::Background if moving => ("Disconnect".to_owned(), palette.accent),
        DropSpot::Background => ("Add as unconnected node".to_owned(), palette.accent),
        DropSpot::Invalid => ("Cannot drop here".to_owned(), palette.error),
        DropSpot::Node { target, placement } => {
            if let Some(r) = state.layout.rect(target) {
                let screen = state.viewport.rect_to_screen(origin, r);
                match drop_bar(screen, placement, state.direction, zoom) {
                    Some(bar) => {
                        shapes.push(Shape::rect_filled(
                            bar,
                            CornerRadius::same(2),
                            palette.accent,
                        ));
                    }
                    None => {
                        shapes.push(Shape::rect_filled(screen, corner, palette.band_fill));
                        shapes.push(Shape::rect_stroke(
                            screen.expand(2.0),
                            corner,
                            Stroke::new(2.5, palette.accent),
                            StrokeKind::Outside,
                        ));
                    }
                }
            }
            let label = target_label(target);
            let text = match placement {
                DropPlacement::Inside => format!("Into {label}"),
                DropPlacement::Before => format!("Before {label}"),
                DropPlacement::After => format!("After {label}"),
            };
            (text, palette.accent)
        }
    };
    if let Some(end) = state.band_end {
        let at = state.viewport.to_screen(origin, end) + vec2(14.0, 14.0);
        hint_bubble(shapes, ui, at, hint, color, palette);
    }
}

/// The bar that marks a place before or after a card (screen rectangle),
/// or `None` for a place inside the card.
fn drop_bar(
    screen: Rect,
    placement: DropPlacement,
    direction: LayoutDirection,
    zoom: f32,
) -> Option<Rect> {
    let gap = 6.0 * zoom.max(0.5);
    let width = 4.0;
    match (placement, direction) {
        (DropPlacement::Inside, _) => None,
        (DropPlacement::Before, LayoutDirection::TopDown) => Some(Rect::from_min_max(
            pos2(screen.min.x - gap - width, screen.min.y),
            pos2(screen.min.x - gap, screen.max.y),
        )),
        (DropPlacement::After, LayoutDirection::TopDown) => Some(Rect::from_min_max(
            pos2(screen.max.x + gap, screen.min.y),
            pos2(screen.max.x + gap + width, screen.max.y),
        )),
        (DropPlacement::Before, LayoutDirection::LeftRight) => Some(Rect::from_min_max(
            pos2(screen.min.x, screen.min.y - gap - width),
            pos2(screen.max.x, screen.min.y - gap),
        )),
        (DropPlacement::After, LayoutDirection::LeftRight) => Some(Rect::from_min_max(
            pos2(screen.min.x, screen.max.y + gap),
            pos2(screen.max.x, screen.max.y + gap + width),
        )),
    }
}

/// A short text in a coloured bubble next to the pointer.
fn hint_bubble(
    shapes: &mut Vec<Shape>,
    ui: &Ui,
    at: Pos2,
    text: String,
    color: Color32,
    palette: &CanvasPalette,
) {
    let galley = ui
        .painter()
        .layout_no_wrap(text, FontId::proportional(12.0), palette.on_color);
    let frame = Rect::from_min_size(at, galley.size()).expand2(vec2(6.0, 3.0));
    shapes.push(Shape::rect_filled(frame, CornerRadius::same(4), color));
    shapes.push(Shape::galley(at, galley, palette.on_color));
}

/// The dots of the visible nodes that can take a child (see [`connect`]).
fn paint_dots(
    state: &CanvasState,
    shapes: &mut Vec<Shape>,
    origin: Pos2,
    visible: &[u32],
    palette: &CanvasPalette,
) {
    let zoom = state.viewport.zoom;
    let active = match state.drag {
        DragMode::Connect { from } => Some(from),
        _ => state.hovered_dot,
    };
    for &index in visible {
        let id = CanvasNodeId(index);
        let (Some(node), Some(rect)) = (state.tree.get(id), state.layout.rect(id)) else {
            continue;
        };
        if !connect::has_dot(node) {
            continue;
        }
        let center = state
            .viewport
            .to_screen(origin, connect::dot_center(rect, state.direction));
        let hot = active == Some(id);
        let r = connect::dot_radius_px(zoom) * if hot { 1.3 } else { 1.0 };
        let color = palette.category(node.category);
        let (fill, stroke, mark) = if hot {
            (palette.accent, palette.accent, palette.on_color)
        } else {
            (palette.card, color, color)
        };
        shapes.push(Shape::circle_filled(center, r, fill));
        shapes.push(Shape::circle_stroke(center, r, Stroke::new(1.5, stroke)));
        // A plus sign: "add a child here".
        let arm = r * 0.5;
        let plus = Stroke::new((r * 0.22).max(1.2), mark);
        shapes.push(Shape::line_segment(
            [center - vec2(arm, 0.0), center + vec2(arm, 0.0)],
            plus,
        ));
        shapes.push(Shape::line_segment(
            [center - vec2(0.0, arm), center + vec2(0.0, arm)],
            plus,
        ));
    }
}

/// The connection from a dot to the pointer during a drag, the place of
/// the new node and a hint.
fn paint_connection(
    state: &CanvasState,
    ui: &Ui,
    shapes: &mut Vec<Shape>,
    origin: Pos2,
    palette: &CanvasPalette,
) {
    let (DragMode::Connect { from }, Some(end)) = (state.drag, state.band_end) else {
        return;
    };
    let Some(rect) = state.layout.rect(from) else {
        return;
    };
    let zoom = state.viewport.zoom;
    let start = state
        .viewport
        .to_screen(origin, connect::dot_center(rect, state.direction));
    let end_screen = state.viewport.to_screen(origin, end);
    // A curve that leaves the dot along the child axis.
    let bend = match state.direction {
        LayoutDirection::TopDown => vec2(0.0, ((end_screen.y - start.y).abs() * 0.5).max(30.0)),
        LayoutDirection::LeftRight => vec2(((end_screen.x - start.x).abs() * 0.5).max(30.0), 0.0),
    };
    shapes.push(Shape::CubicBezier(
        egui::epaint::CubicBezierShape::from_points_stroke(
            [start, start + bend, end_screen - bend, end_screen],
            false,
            Color32::TRANSPARENT,
            Stroke::new(2.0, palette.accent),
        ),
    ));
    shapes.push(Shape::circle_filled(end_screen, 4.0, palette.accent));
    let label = |id: CanvasNodeId| {
        state
            .tree
            .get(id)
            .map(|n| n.label.clone())
            .unwrap_or_default()
    };
    let parent = label(from);
    let hint = match state.connect_end {
        Some(ConnectEnd::Connect(child)) => {
            if let Some(r) = state.layout.rect(child) {
                let screen = state.viewport.rect_to_screen(origin, r);
                shapes.push(Shape::rect_stroke(
                    screen.expand(2.0),
                    radius(6.0 * zoom),
                    Stroke::new(2.5, palette.accent),
                    StrokeKind::Outside,
                ));
            }
            format!("Connect {} to {parent}", label(child))
        }
        Some(ConnectEnd::Add(DropSpot::Node { target, placement })) => {
            if let Some(r) = state.layout.rect(target)
                && let Some(bar) = drop_bar(
                    state.viewport.rect_to_screen(origin, r),
                    placement,
                    state.direction,
                    zoom,
                )
            {
                shapes.push(Shape::rect_filled(
                    bar,
                    CornerRadius::same(2),
                    palette.accent,
                ));
            }
            match placement {
                DropPlacement::Before => {
                    format!("Add a child of {parent} before {}", label(target))
                }
                _ => format!("Add a child of {parent}"),
            }
        }
        _ => format!("Add a child of {parent}"),
    };
    hint_bubble(
        shapes,
        ui,
        end_screen + vec2(14.0, 14.0),
        hint,
        palette.accent,
        palette,
    );
}

/// Dots on a world grid. The spacing grows when the zoom is small, so that
/// the number of dots stays low. All dots are one mesh.
fn background_dots(shapes: &mut Vec<Shape>, rect: Rect, view: Viewport, color: Color32) {
    let mut spacing = 24.0 * view.zoom;
    while spacing < 18.0 {
        spacing *= 4.0;
    }
    if spacing > 400.0 {
        return;
    }
    let offset = rect.min + view.pan;
    let start_x = rect.min.x + (offset.x - rect.min.x).rem_euclid(spacing);
    let start_y = rect.min.y + (offset.y - rect.min.y).rem_euclid(spacing);
    let dot = Vec2::splat(1.6);
    let mut mesh = Mesh::default();
    let mut y = start_y;
    while y < rect.max.y {
        let mut x = start_x;
        while x < rect.max.x {
            mesh.add_colored_rect(Rect::from_center_size(pos2(x, y), dot), color);
            x += spacing;
        }
        y += spacing;
    }
    shapes.push(Shape::mesh(mesh));
}

fn paint_edges(
    state: &mut CanvasState,
    shapes: &mut Vec<Shape>,
    origin: Pos2,
    view: Rect,
    palette: &CanvasPalette,
    detail: Detail,
) {
    let zoom = state.viewport.zoom;
    let vertical = state.direction == LayoutDirection::TopDown;
    let width = (1.6 * zoom).clamp(1.0, 2.6);
    let corner = if detail == Detail::Boxes {
        0.0
    } else {
        8.0 * zoom
    };
    let mut active: Vec<(Shape, u8)> = Vec::new();
    let mut points = std::mem::take(&mut state.points);
    for (index, bounds) in state.edge_bounds.iter().enumerate() {
        let Some(bounds) = bounds else {
            continue;
        };
        if !bounds.intersects(view) {
            continue;
        }
        let parent = &state.tree.nodes()[index];
        let Some(parent_rect) = state.layout.rects.get(index) else {
            continue;
        };
        let start = state
            .viewport
            .to_screen(origin, state.layout.edge_start(*parent_rect));
        let bus_world = state
            .layout
            .bus_of(CanvasNodeId(index as u32), parent.depth);
        for child in &parent.children {
            let Some(child_rect) = state.layout.rect(*child) else {
                continue;
            };
            let end = state
                .viewport
                .to_screen(origin, state.layout.edge_end(child_rect));
            let status = state.tree.nodes()[child.index()].deco.status;
            let (color, rank) = match status {
                NodeStatus::Running => (palette.running, 3),
                NodeStatus::Failure => (palette.failure, 2),
                NodeStatus::Success => (palette.success, 1),
                NodeStatus::Idle | NodeStatus::Skipped => (palette.edge, 0),
            };
            let stroke = Stroke::new(if rank > 0 { width * 1.4 } else { width }, color);
            let shape = match state.edge_style {
                EdgeStyle::Curved if detail != Detail::Boxes => {
                    paint::curved_edge(start, end, vertical, stroke)
                }
                _ => {
                    let bus = bus_world.map_or(
                        if vertical {
                            (start.y + end.y) * 0.5
                        } else {
                            (start.x + end.x) * 0.5
                        },
                        |b| {
                            let p = state.viewport.to_screen(origin, pos2(b, b));
                            if vertical { p.y } else { p.x }
                        },
                    );
                    paint::orthogonal_edge_points(start, end, bus, vertical, corner, &mut points);
                    Shape::line(points.clone(), stroke)
                }
            };
            if rank > 0 {
                active.push((shape, rank));
            } else {
                shapes.push(shape);
            }
        }
    }
    state.points = points;
    // Active edges on top, the most important last.
    active.sort_by_key(|(_, rank)| *rank);
    shapes.extend(active.into_iter().map(|(shape, _)| shape));
}

#[allow(clippy::too_many_arguments)]
fn paint_card(
    state: &mut CanvasState,
    ui: &Ui,
    shapes: &mut Vec<Shape>,
    origin: Pos2,
    index: u32,
    palette: &CanvasPalette,
    detail: Detail,
    text: &TextScale,
    ppp: f32,
    has_focus: bool,
) {
    let text_zoom = text.zoom;
    let id = CanvasNodeId(index);
    let zoom = state.viewport.zoom;
    let Some(world) = state.layout.rect(id) else {
        return;
    };
    let node = &state.tree.nodes()[index as usize];
    let screen = state.viewport.rect_to_screen(origin, world);
    let deco = node.deco;
    let category_color = palette.category(node.category);
    let status_color = palette.status(deco.status);
    let hovered = state.hovered == Some(id);
    let r = radius(6.0 * zoom);
    let skipped = deco.status == NodeStatus::Skipped;

    // Search match halo.
    if deco.search_match {
        shapes.push(Shape::rect_filled(
            screen.expand(5.0 * zoom.max(0.5)),
            radius(9.0 * zoom),
            palette.search,
        ));
    }

    // The executor waits before this node: a halo in the breakpoint
    // colour, and a strong outline after the card (below).
    let stopped = deco.breakpoint == Some(BreakpointMarker::Hit);
    if stopped {
        shapes.push(Shape::rect_filled(
            screen.expand(8.0 * zoom.max(0.5)),
            radius(12.0 * zoom),
            palette.breakpoint.gamma_multiply(0.2),
        ));
    }

    if detail == Detail::Boxes {
        let fill = match status_color {
            Some(color) => mix(palette.card, color, 0.7),
            None => mix(palette.card, category_color, 0.42),
        };
        shapes.push(Shape::rect_filled(screen, r, fill));
        let band_width = card::BAND * zoom;
        if band_width >= 0.75 {
            let band = Rect::from_min_max(
                screen.min,
                pos2(screen.min.x + band_width.max(1.0), screen.max.y),
            );
            shapes.push(Shape::rect_filled(band, CornerRadius::ZERO, category_color));
        }
        if deco.selected {
            shapes.push(Shape::rect_stroke(
                screen.expand(2.0),
                r,
                Stroke::new(2.0, palette.accent),
                StrokeKind::Outside,
            ));
        }
        if stopped {
            shapes.push(Shape::rect_stroke(
                screen.expand(1.0),
                r,
                Stroke::new(2.5, palette.breakpoint),
                StrokeKind::Outside,
            ));
        }
        return;
    }

    let geometry = CardGeometry::of(&node.card);
    let card_fill = if deco.status == NodeStatus::Running {
        mix(
            palette.card,
            palette.running,
            if palette.dark { 0.12 } else { 0.08 },
        )
    } else {
        palette.card
    };
    let alpha = if skipped { 0.55 } else { 1.0 };

    // Collapsed SubTree: two cards behind this one.
    if node.is_collapsed_subtree() {
        for step in [2.0, 1.0] {
            let offset = vec2(4.0, 4.0) * step * zoom;
            let behind = screen.translate(offset);
            shapes.push(Shape::rect_filled(
                behind,
                r,
                palette.card.gamma_multiply(alpha),
            ));
            shapes.push(Shape::rect_stroke(
                behind,
                r,
                Stroke::new(1.0, palette.card_border.gamma_multiply(alpha)),
                StrokeKind::Inside,
            ));
        }
    }

    // Shadow, body, band.
    shapes.push(Shape::rect_filled(
        screen.translate(vec2(0.0, 1.5 * zoom.min(1.5))).expand(0.5),
        r,
        palette.shadow.gamma_multiply(alpha),
    ));
    shapes.push(Shape::rect_filled(
        screen,
        r,
        card_fill.gamma_multiply(alpha),
    ));
    let band = Rect::from_min_max(
        screen.min,
        pos2(screen.min.x + card::BAND * zoom, screen.max.y),
    );
    shapes.push(Shape::rect_filled(
        band,
        CornerRadius {
            nw: r.nw,
            sw: r.sw,
            ne: 0,
            se: 0,
        },
        category_color.gamma_multiply(alpha),
    ));

    // Border.
    let border = match (status_color, deco.status) {
        (Some(color), NodeStatus::Running) => Stroke::new(2.0, color),
        (Some(color), NodeStatus::Skipped) => Stroke::new(1.2, color),
        (Some(color), _) => Stroke::new(1.6, color),
        (None, _) if hovered => Stroke::new(1.0, palette.card_border_hover),
        (None, _) => Stroke::new(1.0, palette.card_border),
    };
    if skipped {
        paint::dashed_rect(shapes, screen, border, 4.0, 3.0);
    } else {
        shapes.push(Shape::rect_stroke(screen, r, border, StrokeKind::Inside));
    }
    if deco.status == NodeStatus::Running {
        shapes.push(Shape::rect_stroke(
            screen.expand(2.0),
            radius(6.0 * zoom + 2.0),
            Stroke::new(3.0, palette.running.gamma_multiply(0.3)),
            StrokeKind::Outside,
        ));
    }
    if stopped {
        shapes.push(Shape::rect_stroke(
            screen.expand(1.0),
            radius(6.0 * zoom + 1.0),
            Stroke::new(3.0, palette.breakpoint),
            StrokeKind::Outside,
        ));
    }

    // Glyph.
    let local = |p: Pos2| card::to_screen(screen, zoom, p);
    let glyph_center = local(geometry.glyph_center());
    let glyph_box = Rect::from_center_size(glyph_center, Vec2::splat(card::GLYPH * zoom));
    shapes.push(Shape::rect_filled(
        glyph_box,
        radius(4.0 * zoom),
        mix(
            palette.card,
            category_color,
            if palette.dark { 0.22 } else { 0.14 },
        )
        .gamma_multiply(alpha),
    ));
    paint::category_glyph(
        shapes,
        glyph_center,
        card::GLYPH * zoom,
        node.category,
        category_color.gamma_multiply(alpha),
    );

    // Text.
    let text_color = palette.text.gamma_multiply(alpha);
    let weak = palette.text_weak.gamma_multiply(alpha);
    let painter = ui.painter();
    let label_line = card::LABEL_LINE * zoom;
    let has_type = node.card.type_line.is_some();
    {
        let label = if detail == Detail::Full {
            state.galleys.get(index, SLOT_LABEL, || {
                painter.layout_job(LayoutJob::simple_singleline(
                    node.card.label.clone(),
                    state
                        .fonts
                        .font(TextKind::Label, card::LABEL_SIZE * text_zoom),
                    text_color,
                ))
            })
        } else {
            // Small cards: a readable minimum size, cut to the free width.
            let size = (card::LABEL_SIZE * text_zoom).max(LABEL_MIN_PX);
            let free =
                (node.card.size.x - card::TEXT_LEFT - card::PAD_X - card::BADGE - 2.0) * zoom;
            state.galleys.get(index, SLOT_LABEL_SMALL, || {
                let mut job = LayoutJob::simple_singleline(
                    node.label.clone(),
                    state.fonts.font(TextKind::Label, size),
                    text_color,
                );
                job.wrap = egui::text::TextWrapping::truncate_at_width(free.max(1.0));
                painter.layout_job(job)
            })
        };
        let pos = if detail == Detail::Full && has_type {
            local(geometry.label_pos(true))
        } else {
            local(geometry.label_pos(false))
        };
        // Centre the text on its line.
        let pos = pos + vec2(0.0, (label_line - label.size().y * text.scale) * 0.5);
        push_text(shapes, label, pos, text, text_color, ppp);
    }
    if detail == Detail::Full {
        if let Some(type_line) = &node.card.type_line {
            let galley = state.galleys.get(index, SLOT_TYPE, || {
                painter.layout_job(LayoutJob::simple_singleline(
                    type_line.clone(),
                    state
                        .fonts
                        .font(TextKind::Type, card::TYPE_SIZE * text_zoom),
                    weak,
                ))
            });
            let mut pos = local(geometry.type_pos());
            pos.y += (card::TYPE_LINE * zoom - galley.size().y * text.scale) * 0.5;
            push_text(shapes, galley, pos, text, weak, ppp);
        }
        let value_color = palette.text_value.gamma_multiply(alpha);
        if geometry.port_lines > 0 {
            let y = local(pos2(0.0, geometry.separator_y())).y;
            shapes.push(Shape::hline(
                (screen.min.x + card::BAND * zoom)..=(screen.max.x - 1.0),
                y,
                Stroke::new(1.0, palette.separator.gamma_multiply(alpha)),
            ));
            let colors = EntryColors {
                name: weak,
                value: value_color,
                reference: palette.text_reference.gamma_multiply(alpha),
            };
            let entries = &node.card.ports;
            let mut line = 0;
            for entry in entries {
                for part in 0..entry.line_count() {
                    let galley = state.galleys.get(index, SLOT_LINES + line as u64, || {
                        entry_line_job(painter, &state.fonts, entry, part, colors, text_zoom)
                    });
                    let mut pos = local(geometry.port_pos(line));
                    pos.y += (card::PORT_LINE * zoom - galley.size().y * text.scale) * 0.5;
                    push_text(shapes, galley, pos, text, value_color, ppp);
                    line += 1;
                }
            }
        }
        if geometry.condition_lines > 0 {
            let boxed = geometry.condition_box(node.card.size.x);
            condition_box(
                shapes,
                Rect::from_min_max(local(boxed.min), local(boxed.max)),
                palette,
                alpha,
                zoom,
            );
            let colors = EntryColors {
                name: palette.condition.gamma_multiply(alpha),
                value: value_color,
                reference: value_color,
            };
            let first_slot = SLOT_LINES + geometry.port_lines as u64;
            let mut line = 0;
            for entry in &node.card.conditions {
                for part in 0..entry.line_count() {
                    let galley = state.galleys.get(index, first_slot + line as u64, || {
                        entry_line_job(painter, &state.fonts, entry, part, colors, text_zoom)
                    });
                    let mut pos = local(geometry.condition_pos(line));
                    pos.y += (card::PORT_LINE * zoom - galley.size().y * text.scale) * 0.5;
                    push_text(shapes, galley, pos, text, value_color, ppp);
                    line += 1;
                }
            }
        }
    }

    // Small cards: grey bars stand for the port lines.
    if detail == Detail::Labels && geometry.port_lines > 0 {
        let bar_color = palette.separator.gamma_multiply(alpha);
        let y = local(pos2(0.0, geometry.separator_y())).y;
        shapes.push(Shape::hline(
            (screen.min.x + card::BAND * zoom)..=(screen.max.x - 1.0),
            y,
            Stroke::new(1.0, bar_color),
        ));
        let max_bar = (node.card.size.x - card::BAND - 2.0 * card::PAD_X) * zoom;
        for (line, (indent, chars)) in entry_bars(&node.card.ports).enumerate() {
            let width = (chars as f32 * 6.0 * zoom).min(max_bar - indent * zoom);
            let top = local(geometry.port_pos(line));
            let bar = Rect::from_min_size(
                pos2(top.x + indent * zoom, top.y + card::PORT_LINE * zoom * 0.3),
                vec2(width, (card::PORT_LINE * zoom * 0.4).max(1.0)),
            );
            shapes.push(Shape::rect_filled(bar, radius(2.0), bar_color));
        }
    }
    // Small cards: coloured bars stand for the condition lines.
    if detail == Detail::Labels && geometry.condition_lines > 0 {
        let boxed = geometry.condition_box(node.card.size.x);
        condition_box(
            shapes,
            Rect::from_min_max(local(boxed.min), local(boxed.max)),
            palette,
            alpha,
            zoom,
        );
        let bar_color = palette.condition.gamma_multiply(alpha * 0.55);
        let max_bar = (node.card.size.x - card::BAND - 2.0 * card::PAD_X) * zoom;
        for (line, (indent, chars)) in entry_bars(&node.card.conditions).enumerate() {
            let width = (chars as f32 * 6.0 * zoom).min(max_bar - indent * zoom);
            let top = local(geometry.condition_pos(line));
            let bar = Rect::from_min_size(
                pos2(top.x + indent * zoom, top.y + card::PORT_LINE * zoom * 0.3),
                vec2(width, (card::PORT_LINE * zoom * 0.4).max(1.0)),
            );
            shapes.push(Shape::rect_filled(bar, radius(2.0), bar_color));
        }
    }

    // Status badge.
    let badge_center = local(geometry.badge_center(node.card.size.x));
    match (deco.status, deco.previous_status) {
        (NodeStatus::Idle, Some(previous)) => {
            paint::status_badge(
                shapes,
                badge_center,
                card::BADGE * zoom,
                previous,
                true,
                palette,
            );
        }
        (status, _) => {
            paint::status_badge(
                shapes,
                badge_center,
                card::BADGE * zoom,
                status,
                false,
                palette,
            );
        }
    }

    // Collapsed SubTree: the number of hidden nodes.
    if let Some(info) = node
        .subtree
        .as_ref()
        .filter(|_| node.is_collapsed_subtree())
    {
        let text = format!("+{}", info.inner_count);
        let galley = state.galleys.get(index, SLOT_CHIP, || {
            painter.layout_job(LayoutJob::simple_singleline(
                text,
                state
                    .fonts
                    .font(TextKind::Note, TextKind::Note.size() * text_zoom),
                palette.on_color,
            ))
        });
        let center = match state.direction {
            LayoutDirection::TopDown => pos2(screen.center().x, screen.max.y + 4.0 * zoom),
            LayoutDirection::LeftRight => pos2(screen.max.x + 6.0 * zoom, screen.center().y),
        };
        paint::pill(
            shapes,
            center,
            galley,
            palette.category(NodeCategory::SubTree),
            palette.on_color,
            vec2(5.0, 1.0) * zoom,
        );
    }

    // Folded node: the number of hidden nodes, below the dot.
    if let Some(hidden) = node.folded {
        let galley = state.galleys.get(index, SLOT_FOLD, || {
            painter.layout_job(LayoutJob::simple_singleline(
                format!("+{hidden}"),
                state
                    .fonts
                    .font(TextKind::Note, TextKind::Note.size() * text_zoom),
                palette.on_color,
            ))
        });
        let center = match state.direction {
            LayoutDirection::TopDown => pos2(screen.center().x, screen.max.y + 17.0 * zoom),
            LayoutDirection::LeftRight => pos2(screen.max.x + 22.0 * zoom, screen.center().y),
        };
        paint::pill(
            shapes,
            center,
            galley,
            category_color.gamma_multiply(alpha),
            palette.on_color,
            vec2(5.0, 1.0) * zoom,
        );
    }

    // Substitution tag.
    if let Some(substitution) = deco.substitution {
        let (text, color, slot) = match substitution {
            Substitution::ForcedSuccess => ("forced SUCCESS", palette.success, SLOT_TAG),
            Substitution::ForcedFailure => ("forced FAILURE", palette.failure, SLOT_TAG_FAILURE),
        };
        let galley = state.galleys.get(index, slot, || {
            painter.layout_job(LayoutJob::simple_singleline(
                text.to_owned(),
                state.fonts.font(TextKind::Note, 9.5 * text_zoom),
                palette.on_color,
            ))
        });
        let size = galley.size();
        let center = pos2(
            screen.min.x + (card::BAND + card::PAD_X) * zoom + size.x * 0.5 + 4.0 * zoom,
            screen.min.y,
        );
        paint::pill(
            shapes,
            center,
            galley,
            color,
            palette.on_color,
            vec2(4.0, 1.0) * zoom,
        );
    }

    // Breakpoint marker: on the category band, at the header, like a
    // breakpoint in the gutter of a code editor.
    if let Some(marker) = deco.breakpoint {
        let radius = (5.5 * zoom).clamp(3.5, 8.0);
        let center = pos2(
            screen.min.x + card::BAND * zoom * 0.5,
            screen.min.y + geometry.header_height * 0.5 * zoom,
        );
        paint::breakpoint_marker(shapes, center, radius, marker, palette);
    }

    // Problem marker (top-right corner).
    if let Some(problem) = deco.problem {
        let radius = (7.0 * zoom).clamp(4.0, 9.0);
        paint::problem_marker(
            shapes,
            pos2(screen.max.x - 1.0, screen.min.y + 1.0),
            radius,
            problem,
            palette,
        );
    }

    // Selection.
    if deco.selected {
        let primary = state.primary == Some(id);
        if primary {
            shapes.push(Shape::rect_stroke(
                screen.expand(3.0),
                radius(6.0 * zoom + 3.0),
                Stroke::new(
                    5.0,
                    palette
                        .accent
                        .gamma_multiply(if has_focus { 0.22 } else { 0.12 }),
                ),
                StrokeKind::Outside,
            ));
        }
        shapes.push(Shape::rect_stroke(
            screen.expand(2.0),
            radius(6.0 * zoom + 2.0),
            Stroke::new(2.0, palette.accent),
            StrokeKind::Outside,
        ));
    }
}

/// The largest size of the minimap.
const MINIMAP_MAX: Vec2 = vec2(220.0, 150.0);

/// The rectangle of the minimap in a canvas, for tree bounds of this
/// shape. `None` if the canvas is too small.
fn minimap_rect(canvas: Rect, bounds: Rect) -> Option<Rect> {
    if !bounds.is_positive() || canvas.width() < 360.0 || canvas.height() < 240.0 {
        return None;
    }
    let scale = (MINIMAP_MAX.x / bounds.width()).min(MINIMAP_MAX.y / bounds.height());
    let size = (bounds.size() * scale).max(vec2(40.0, 24.0)) + Vec2::splat(12.0);
    Some(Rect::from_min_size(
        canvas.max - size - Vec2::splat(12.0),
        size,
    ))
}

/// The minimap: an overview of the whole tree in the bottom-right corner.
/// It shows when the tree does not fit the view. A click or a drag moves
/// the view to that place.
fn minimap(state: &mut CanvasState, ui: &Ui, rect: Rect, palette: &CanvasPalette, canvas: Id) {
    let bounds = state.layout.bounds;
    let visible = state.viewport.visible_world(rect);
    if !bounds.is_positive() || visible.contains_rect(bounds) {
        return;
    }
    let Some(frame) = minimap_rect(rect, bounds) else {
        return;
    };
    let inner = frame.shrink(6.0);
    let scale = (inner.width() / bounds.width()).min(inner.height() / bounds.height());
    let offset = inner.center() - bounds.center().to_vec2() * scale;
    let to_mini = |p: Pos2| offset + p.to_vec2() * scale;
    let from_mini = |p: Pos2| ((p - offset) / scale).to_pos2();

    let response = ui.interact(frame, canvas.with("minimap"), Sense::click_and_drag());
    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let target = Viewport::centered_on(from_mini(pointer), state.size, state.viewport.zoom);
        if response.dragged() {
            state.set_viewport(target);
        } else {
            state.animate_to(target, SHORT_ANIMATION_SECONDS);
        }
    }

    let painter = ui.painter_at(rect);
    painter.rect(
        frame,
        CornerRadius::same(6),
        palette.card.gamma_multiply(0.94),
        Stroke::new(1.0, palette.card_border),
        StrokeKind::Inside,
    );
    // The node mesh changes only with the layout, the statuses, the
    // palette or the place of the minimap. Pan and zoom move only the view
    // rectangle, so the mesh stays in a cache.
    let key = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for value in [inner.min.x, inner.min.y, inner.max.x, inner.max.y, scale] {
            value.to_bits().hash(&mut hasher);
        }
        palette_key(palette).hash(&mut hasher);
        state.tree.len().hash(&mut hasher);
        for node in state.tree.nodes() {
            node.deco.status.hash(&mut hasher);
        }
        hasher.finish()
    };
    let mesh = match &state.minimap_cache {
        Some((cached, mesh)) if *cached == key => mesh.clone(),
        _ => {
            let mut mesh = Mesh::default();
            for (node, world) in state.tree.nodes().iter().zip(&state.layout.rects) {
                let color = palette
                    .status(node.deco.status)
                    .unwrap_or_else(|| palette.category(node.category));
                let r = Rect::from_min_max(to_mini(world.min), to_mini(world.max));
                let r = Rect::from_center_size(r.center(), r.size().max(Vec2::splat(1.0)));
                mesh.add_colored_rect(r, color);
            }
            let mesh = Arc::new(mesh);
            state.minimap_cache = Some((key, mesh.clone()));
            mesh
        }
    };
    painter.add(Shape::Mesh(mesh));
    let view =
        Rect::from_min_max(to_mini(visible.min), to_mini(visible.max)).intersect(inner.expand(2.0));
    if view.is_positive() {
        painter.rect(
            view,
            CornerRadius::same(2),
            palette.accent.gamma_multiply(0.1),
            Stroke::new(1.5, palette.accent),
            StrokeKind::Middle,
        );
    }
    response.on_hover_text("Click or drag to move the view");
}

/// Describes the visible nodes for screen readers (and for UI tests).
fn describe_for_accesskit(state: &CanvasState, ui: &Ui, rect: Rect, canvas: Id) {
    let ctx = ui.ctx();
    let title = state.tree.title.clone();
    let enabled = ctx
        .accesskit_node_builder(canvas, |builder| {
            builder.set_role(egui::accesskit::Role::Tree);
            builder.set_label(format!("Tree canvas: {title}"));
        })
        .is_some();
    if !enabled {
        return;
    }
    for &index in &state.visible {
        let id = CanvasNodeId(index);
        let Some(node) = state.tree.get(id) else {
            continue;
        };
        let Some(screen) = state.node_screen_rect(rect.min, id) else {
            continue;
        };
        let screen = screen.intersect(rect);
        if !screen.is_positive() {
            continue;
        }
        let description = match node.deco.status {
            NodeStatus::Idle => format!("{} ({})", node.type_id, node.category.name()),
            status => format!("{} ({}), {status}", node.type_id, node.category.name()),
        };
        ctx.accesskit_node_builder(node_item_id(canvas, id), |builder| {
            builder.set_role(egui::accesskit::Role::TreeItem);
            builder.set_label(node.label.clone());
            builder.set_description(description);
            builder.set_bounds(egui::accesskit::Rect {
                x0: screen.min.x.into(),
                y0: screen.min.y.into(),
                x1: screen.max.x.into(),
                y1: screen.max.y.into(),
            });
            builder.set_selected(node.deco.selected);
            if let Some(info) = &node.subtree
                && info.can_expand
            {
                builder.set_expanded(info.expanded);
            }
            builder.add_action(egui::accesskit::Action::Click);
        });
    }
}

fn show_tooltip(
    state: &CanvasState,
    ui: &Ui,
    response: &Response,
    canvas: Id,
    now: f64,
    palette: &CanvasPalette,
) {
    let Some(id) = state.hovered else {
        return;
    };
    if state.drag != DragMode::None
        || now - state.hover_start < TOOLTIP_DELAY
        || ui.input(|i| i.pointer.any_down())
        || response.context_menu_opened()
    {
        if now - state.hover_start < TOOLTIP_DELAY {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(TOOLTIP_DELAY));
        }
        return;
    }
    let Some(node) = state.tree.get(id) else {
        return;
    };
    egui::Tooltip::always_open(
        ui.ctx().clone(),
        response.layer_id,
        canvas.with("tooltip"),
        egui::PopupAnchor::Pointer,
    )
    .gap(14.0)
    .show(|ui| {
        ui.set_max_width(360.0);
        ui.label(egui::RichText::new(&node.label).strong().size(14.0));
        ui.label(
            egui::RichText::new(format!("{}  ·  {}", node.type_id, node.category.name())).weak(),
        );
        if let Some(description) = &node.description {
            ui.label(description);
        }
        if node.category_inferred {
            ui.label(
                egui::RichText::new("Category inferred: no model is known for this type.")
                    .small()
                    .weak(),
            );
        }
        egui::Grid::new(canvas.with("tooltip_grid"))
            .num_columns(2)
            .spacing([12.0, 2.0])
            .show(ui, |ui| {
                if !node.path.is_empty() && node.path != node.label {
                    ui.label(egui::RichText::new("Path").weak());
                    ui.label(&node.path);
                    ui.end_row();
                }
                if let Some(uid) = node.uid {
                    ui.label(egui::RichText::new("UID").weak());
                    ui.label(uid.to_string());
                    ui.end_row();
                }
                let status = match (node.deco.status, node.deco.previous_status) {
                    (NodeStatus::Idle, Some(previous)) => format!("IDLE (before: {previous})"),
                    (status, _) => status.to_string(),
                };
                ui.label(egui::RichText::new("Status").weak());
                ui.label(status);
                ui.end_row();
                if let Some(info) = &node.subtree {
                    ui.label(egui::RichText::new("Tree").weak());
                    let state = match (info.can_expand, info.expanded) {
                        (false, _) => "not found".to_owned(),
                        (true, true) => format!("expanded, {} nodes", info.inner_count),
                        (true, false) => format!("collapsed, {} nodes", info.inner_count),
                    };
                    ui.label(format!("{} ({state})", info.tree_id));
                    ui.end_row();
                }
            });
        if !node.ports.is_empty() {
            ui.separator();
            egui::Grid::new(canvas.with("tooltip_ports"))
                .num_columns(2)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    for port in &node.ports {
                        ui.label(egui::RichText::new(&port.name).weak());
                        ui.add(
                            egui::Label::new(egui::RichText::new(&port.value).monospace()).wrap(),
                        );
                        ui.end_row();
                    }
                });
        }
        if !node.conditions.is_empty() {
            ui.add_space(4.0);
            condition_frame(palette).show(ui, |ui| {
                egui::Grid::new(canvas.with("tooltip_conditions"))
                    .num_columns(2)
                    .spacing([12.0, 2.0])
                    .show(ui, |ui| {
                        for line in &node.conditions {
                            ui.label(egui::RichText::new(&line.name).color(palette.condition));
                            ui.add(
                                egui::Label::new(egui::RichText::new(&line.value).monospace())
                                    .wrap(),
                            );
                            ui.end_row();
                        }
                    });
            });
        }
        if let Some(hidden) = node.folded {
            ui.label(
                egui::RichText::new(format!(
                    "{hidden} nodes below are folded. Double-click to unfold."
                ))
                .small()
                .weak(),
            );
        }
        if node.subtree.as_ref().is_some_and(|s| s.can_expand) {
            ui.label(
                egui::RichText::new("Double-click to expand or collapse.")
                    .small()
                    .weak(),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::card::FixedMeasure;
    use crate::canvas::model::{CanvasNode, CanvasTreeBuilder};
    use fasttick_core::runtime::NodeIndex;

    fn chain_tree(n: usize) -> CanvasTree {
        let mut builder = CanvasTreeBuilder::new();
        let root = builder.push(
            None,
            CanvasNode::new(NodeSource::Runtime(NodeIndex(0)), "root", "Sequence"),
        );
        for i in 1..n {
            builder.push(
                Some(root),
                CanvasNode::new(NodeSource::Runtime(NodeIndex(i)), format!("n{i}"), "Action"),
            );
        }
        builder.finish("T")
    }

    #[test]
    fn detail_levels() {
        assert_eq!(Detail::for_zoom(1.0), Detail::Full);
        assert_eq!(Detail::for_zoom(0.5), Detail::Labels);
        assert_eq!(Detail::for_zoom(0.1), Detail::Boxes);
        assert!(Detail::Boxes < Detail::Full);
    }

    #[test]
    fn pending_fit_and_focus() {
        let mut state = CanvasState::new(chain_tree(10));
        state.prepare_with(&mut FixedMeasure(7.0));
        assert!(!state.needs_layout());
        let size = vec2(800.0, 600.0);
        state.size = size;
        state.fit_to_view(false);
        state.apply_pending(size);
        // The fit happened without animation.
        let bounds = state.layout().bounds;
        let screen = state.viewport().rect_to_screen(Pos2::ZERO, bounds);
        assert!(Rect::from_min_size(Pos2::ZERO, size).contains_rect(screen));

        // Focus with animation: the view arrives at the node.
        state.set_viewport(Viewport::new(vec2(5000.0, 0.0), 0.3));
        state.focus_on(CanvasNodeId(5), true);
        assert!(state.tree().is_selected(CanvasNodeId(5)));
        state.apply_pending(size);
        assert!(state.step_animation(0.0));
        assert!(state.step_animation(10.0));
        assert!(!state.step_animation(10.1));
        assert_eq!(state.viewport().zoom, 1.0);
        let center = state.layout().rect(CanvasNodeId(5)).unwrap().center();
        let on_screen = state.viewport().to_screen(Pos2::ZERO, center);
        assert!((on_screen - pos2(400.0, 300.0)).length() < 0.01);
    }

    #[test]
    fn zoom_out_stops_at_the_tree_size() {
        let mut state = CanvasState::new(chain_tree(200));
        state.prepare_with(&mut FixedMeasure(7.0));
        let size = vec2(800.0, 600.0);
        state.size = size;
        let min = state.min_zoom();
        let fit = Viewport::fit(
            state.layout().bounds,
            size,
            viewport::FIT_MARGIN,
            viewport::FIT_MAX_ZOOM,
        );
        assert!((min - fit.zoom).abs() < 1e-6 && min < 1.0);
        let mut now = 0.0;
        for _ in 0..40 {
            state.zoom_by(0.5);
            for _ in 0..2 {
                now += 10.0;
                state.step_animation(now);
            }
        }
        assert!((state.viewport().zoom - min).abs() < 1e-4);
        // A small tree: down to 100 %.
        let mut small = CanvasState::new(chain_tree(2));
        small.prepare_with(&mut FixedMeasure(7.0));
        small.size = size;
        assert_eq!(small.min_zoom(), viewport::FIT_MAX_ZOOM);
    }

    #[test]
    fn initial_view_keeps_cards_readable() {
        let size = vec2(800.0, 600.0);
        // A small tree fits.
        let mut small = CanvasState::new(chain_tree(3));
        small.prepare_with(&mut FixedMeasure(7.0));
        small.apply_pending(size);
        let screen = small
            .viewport()
            .rect_to_screen(Pos2::ZERO, small.layout().bounds);
        assert!(Rect::from_min_size(Pos2::ZERO, size).contains_rect(screen));
        // A wide tree shows the root at a readable zoom, at the top.
        let mut wide = CanvasState::new(chain_tree(40));
        wide.prepare_with(&mut FixedMeasure(7.0));
        wide.apply_pending(size);
        assert_eq!(wide.viewport().zoom, READABLE_ZOOM);
        let root = wide
            .viewport()
            .rect_to_screen(Pos2::ZERO, wide.layout().rects[0]);
        assert!((root.center().x - 400.0).abs() < 0.01);
        assert!((root.min.y - viewport::FIT_MARGIN).abs() < 0.01);
        // Left-right: the root at the left.
        wide.set_direction(LayoutDirection::LeftRight);
        wide.prepare_with(&mut FixedMeasure(7.0));
        wide.apply_pending(size);
        let root = wide
            .viewport()
            .rect_to_screen(Pos2::ZERO, wide.layout().rects[0]);
        if wide.viewport().zoom == READABLE_ZOOM {
            assert!((root.min.x - viewport::FIT_MARGIN).abs() < 0.01);
            assert!((root.center().y - 300.0).abs() < 0.01);
        }
    }

    #[test]
    fn zoom_steps_animate_to_target() {
        // A wide tree, so that it can zoom out below 100 %.
        let mut state = CanvasState::new(chain_tree(60));
        state.prepare_with(&mut FixedMeasure(7.0));
        state.size = vec2(400.0, 300.0);
        state.set_viewport(Viewport::new(Vec2::ZERO, 1.0));
        state.zoom_in();
        state.step_animation(0.0);
        state.step_animation(5.0);
        assert!((state.viewport().zoom - viewport::ZOOM_STEP).abs() < 1e-4);
        state.zoom_reset();
        state.step_animation(5.0);
        state.step_animation(10.0);
        assert!((state.viewport().zoom - 1.0).abs() < 1e-4);
        state.zoom_out();
        state.step_animation(10.0);
        state.step_animation(20.0);
        assert!(state.viewport().zoom < 1.0);
        // An invalid viewport is ignored.
        let before = state.viewport();
        state.set_viewport(Viewport {
            pan: Vec2::ZERO,
            zoom: f32::NAN,
        });
        assert_eq!(state.viewport(), before);
    }

    #[test]
    fn replace_tree_keeps_anchor_and_selection() {
        let mut state = CanvasState::new(chain_tree(4));
        state.prepare_with(&mut FixedMeasure(7.0));
        state.size = vec2(800.0, 600.0);
        state.apply_pending(state.size);
        assert!(click_select(
            &mut state,
            Some(CanvasNodeId(2)),
            SelectMode::Replace
        ));
        let anchor = state.tree().get(CanvasNodeId(2)).unwrap().source;
        let before = state.viewport().to_screen(
            Pos2::ZERO,
            state.layout().rect(CanvasNodeId(2)).unwrap().min,
        );

        // A new tree with one more node: the layout moves the anchor.
        state.replace_tree(chain_tree(9), Some(anchor));
        assert!(state.needs_layout());
        state.prepare_with(&mut FixedMeasure(7.0));
        state.apply_pending(state.size);
        let after = state.viewport().to_screen(
            Pos2::ZERO,
            state.layout().rect(CanvasNodeId(2)).unwrap().min,
        );
        assert!((after - before).length() < 0.01);
        assert!(state.tree().is_selected(CanvasNodeId(2)));
        assert_eq!(state.primary(), Some(CanvasNodeId(2)));
    }

    #[test]
    fn replace_tree_keeps_new_problem_markers_and_old_measurements() {
        let mut state = CanvasState::new(chain_tree(4));
        state.prepare_with(&mut FixedMeasure(7.0));
        state
            .tree_mut()
            .get_mut(CanvasNodeId(1))
            .unwrap()
            .deco
            .problem = Some(crate::canvas::ProblemMarker::Error);
        let measured = state.tree().get(CanvasNodeId(0)).unwrap().card.clone();
        assert_ne!(measured.size, Vec2::ZERO);

        // The new tree has no problem at node 1 and a new label at node 2.
        let mut tree = chain_tree(4);
        tree.get_mut(CanvasNodeId(2)).unwrap().label = "renamed".into();
        state.replace_tree(tree, None);
        let tree = state.tree();
        assert_eq!(tree.get(CanvasNodeId(1)).unwrap().deco.problem, None);
        assert_eq!(tree.get(CanvasNodeId(0)).unwrap().card, measured);
        assert_eq!(tree.get(CanvasNodeId(2)).unwrap().card.size, Vec2::ZERO);
        // Only the changed node waits for a measurement.
        let missing = tree
            .nodes()
            .iter()
            .filter(|n| n.card.size == Vec2::ZERO)
            .count();
        assert_eq!(missing, 1);
    }

    #[test]
    fn click_selection_modes() {
        let mut state = CanvasState::new(chain_tree(4));
        assert!(click_select(
            &mut state,
            Some(CanvasNodeId(1)),
            SelectMode::Replace
        ));
        assert!(click_select(
            &mut state,
            Some(CanvasNodeId(2)),
            SelectMode::Add
        ));
        assert_eq!(state.tree().selected_count(), 2);
        assert!(click_select(
            &mut state,
            Some(CanvasNodeId(2)),
            SelectMode::Toggle
        ));
        assert_eq!(state.tree().selected_count(), 1);
        // The primary node was deselected.
        assert_eq!(state.primary(), None);
        assert!(!click_select(&mut state, None, SelectMode::Add));
        assert!(click_select(&mut state, None, SelectMode::Replace));
        assert_eq!(state.tree().selected_count(), 0);
        assert_eq!(state.primary(), None);
        assert_eq!(select_mode(Modifiers::COMMAND), SelectMode::Toggle);
        assert_eq!(select_mode(Modifiers::SHIFT), SelectMode::Add);
        assert_eq!(select_mode(Modifiers::NONE), SelectMode::Replace);
    }

    #[test]
    fn minimap_rect_keeps_the_aspect_ratio() {
        let canvas = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
        let wide = Rect::from_min_size(Pos2::ZERO, vec2(10000.0, 500.0));
        let mini = minimap_rect(canvas, wide).unwrap();
        assert!(canvas.contains_rect(mini));
        assert!((mini.width() - 12.0 - MINIMAP_MAX.x).abs() < 0.01);
        assert!(mini.height() < mini.width());
        let tall = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 10000.0));
        let mini = minimap_rect(canvas, tall).unwrap();
        assert!((mini.height() - 12.0 - MINIMAP_MAX.y).abs() < 0.01);
        assert!(minimap_rect(Rect::from_min_size(Pos2::ZERO, vec2(200.0, 100.0)), wide).is_none());
        assert!(minimap_rect(canvas, Rect::NOTHING).is_none());
    }

    #[test]
    fn helpers() {
        assert!(is_reference("{goal}"));
        assert!(!is_reference("3.0"));
        assert_eq!(snap(pos2(1.26, 2.74), 2.0), pos2(1.5, 2.5));
        assert_eq!(radius(300.0), CornerRadius::same(255));
        assert_eq!(radius(-1.0), CornerRadius::same(0));
    }
}
