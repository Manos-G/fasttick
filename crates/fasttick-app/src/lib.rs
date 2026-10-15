//! FastTick desktop application: the GUI shell and the tree canvas.
//!
//! The binary (`src/main.rs`) only parses the command line and starts
//! [`app::FastTickApp`]. The library form lets the UI tests drive the app
//! headless with `egui_kittest`.
//!
//! Modules:
//!
//! - [`app`]: the shell (menus, mode switcher, dock, status bar, dialogs).
//! - [`canvas`]: the tree canvas (view model, layout boundary, widget).
//! - [`dock`]: dock tabs and default arrangements.
//! - [`documents`]: open documents and their views.
//! - [`panels`]: side panels (start page, tree list, inspector, placeholders).
//! - [`shortcuts`], [`settings`], [`theme`], [`fonts`]: key map, persisted
//!   settings, light and dark themes, bundled fonts.
//! - [`jobs`], [`live`]: background work (file dialogs, loading) and the
//!   live connection.
//!
//! The GUI does not parse XML, logs or protocol messages: it calls
//! `fasttick-core`, `fasttick-log` and `fasttick-protocol`.

pub mod actions;
pub mod add_popup;
pub mod app;
pub mod canvas;
pub mod command_search;
pub mod dialogs;
pub mod dock;
pub mod documents;
pub mod edit_menu;
pub mod edit_ops;
pub mod editor;
pub mod fonts;
pub mod fuzzy;
pub mod jobs;
pub mod live;
pub mod model_dialog;
pub mod panels;
pub mod replay;
pub mod search_list;
pub mod settings;
pub mod shortcuts;
pub mod theme;
pub mod widgets;
