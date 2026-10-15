//! The tree canvas: one drawing surface for the three modes (edit, live,
//! replay). See `docs/DESIGN.md` section 3.1.
//!
//! - [`model`]: the view model ([`CanvasTree`]) and its builders.
//! - [`card`]: the geometry and the text of a node card.
//! - [`layout`]: the layout boundary (`layout(&CanvasTree, direction)`).
//! - [`viewport`]: pan and zoom maths.
//! - [`hit`]: hit testing and culling.
//! - [`paint`]: glyphs, badges, markers and edges.
//! - [`widget`]: the [`TreeCanvas`] widget and its [`CanvasState`].
//!
//! Typical use:
//!
//! ```ignore
//! let tree = CanvasTree::from_runtime(&runtime, options, &expansion, &BuiltinCategories);
//! let mut state = CanvasState::new(tree);
//! // Each frame:
//! let output = TreeCanvas::new(&mut state, &palette).show(ui);
//! for event in output.events { /* ... */ }
//! ```

pub mod card;
pub mod categories;
pub mod hit;
pub mod layout;
pub mod model;
pub mod paint;
pub mod viewport;
pub mod widget;

pub use categories::{BuiltinCategories, CategoryLookup, TableCategories};
pub use layout::{LayoutDirection, TreeLayout, layout};
pub use model::{
    BreakpointMarker, CanvasNode, CanvasNodeId, CanvasTree, CanvasTreeBuilder, Decorations,
    Expansion, NavStep, NodeCategory, NodeSource, PortLine, ProblemMarker, RuntimeBuildOptions,
    Substitution, SubtreeInfo, SubtreeLinking,
};
pub use paint::EdgeStyle;
pub use viewport::Viewport;
pub use widget::{CanvasEvent, CanvasOutput, CanvasState, CanvasStats, Detail, TreeCanvas};
