//! The tree canvas: one drawing surface for the three modes (edit, live,
//! replay). See `docs/DESIGN.md` section 3.1.
//!
//! - [`model`]: the view model ([`CanvasTree`]) and its builders.
//! - [`card`]: the geometry and the text of a node card.
//! - [`layout`]: the layout boundary (`layout(&CanvasTree, direction)`).
//! - [`viewport`]: pan and zoom maths.
//! - [`hit`]: hit testing and culling.
//! - [`paint`]: glyphs, badges, markers and edges.
//! - [`svg`]: the export of a tree as an SVG image.
//! - [`widget`]: the [`TreeCanvas`] widget and its [`CanvasState`].
//!
//! Typical use:
//!
//! ```no_run
//! use fasttick_app::canvas::{
//!     BuiltinCategories, CanvasEvent, CanvasState, CanvasTree, RuntimeBuildOptions, TreeCanvas,
//! };
//! use fasttick_app::theme::CanvasPalette;
//! use fasttick_core::runtime::RuntimeTree;
//!
//! fn show(ui: &mut egui::Ui, state: &mut CanvasState, runtime: &RuntimeTree) {
//!     let palette = CanvasPalette::for_theme(ui.ctx().theme());
//!     let output = TreeCanvas::new(state, &palette).show(ui);
//!     for event in output.events {
//!         if let CanvasEvent::ToggleSubtree(_source) = event {
//!             // Change the expansion and rebuild: see `Workspace::toggle_subtree`.
//!         }
//!     }
//! }
//!
//! let xml = r#"<root><BehaviorTree ID="T"><Sequence><AlwaysSuccess/></Sequence></BehaviorTree></root>"#;
//! let runtime = RuntimeTree::parse(xml).unwrap();
//! let options = RuntimeBuildOptions::for_tree(&runtime);
//! let expansion = CanvasTree::default_expansion(&runtime, options);
//! let tree = CanvasTree::from_runtime(&runtime, options, &expansion, &BuiltinCategories);
//! let mut state = CanvasState::new(tree);
//! // Each frame: show(ui, &mut state, &runtime);
//! ```

pub mod card;
pub mod categories;
pub mod connect;
pub mod drop;
pub mod hit;
pub mod layout;
pub mod model;
pub mod paint;
pub mod svg;
pub mod synthetic;
pub mod viewport;
pub mod widget;
pub mod xml_build;

pub use categories::{BuiltinCategories, CategoryLookup, TableCategories};
pub use drop::{DropPlacement, DropSpot, NewNodePayload};
pub use layout::{LayoutDirection, TreeLayout, layout};
pub use model::{
    BreakpointMarker, CanvasNode, CanvasNodeId, CanvasTree, CanvasTreeBuilder, Decorations,
    Expansion, NavStep, NodeCategory, NodeSource, PortLine, ProblemMarker, RuntimeBuildOptions,
    Substitution, SubtreeInfo, SubtreeLinking,
};
pub use paint::EdgeStyle;
pub use viewport::Viewport;
pub use widget::{
    CanvasEvent, CanvasOutput, CanvasState, CanvasStats, Detail, NodeMenu, TreeCanvas,
    condition_frame,
};
pub use xml_build::{ProjectCategories, category_of_kind};
