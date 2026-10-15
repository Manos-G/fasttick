//! FastTick desktop application: the GUI shell and the tree canvas.
//!
//! The binary (`src/main.rs`) only starts [`app::FastTickApp`]. The library
//! form lets the UI tests drive the app headless with `egui_kittest`.

pub mod canvas;
pub mod fonts;
pub mod theme;
