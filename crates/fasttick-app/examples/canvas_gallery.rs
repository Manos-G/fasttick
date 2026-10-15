//! A gallery of all node decorations on the canvas: each status, the
//! previous status, breakpoint, substitution and problem markers, search
//! matches, selection and a collapsed SubTree.
//!
//! Run: `cargo run -p fasttick-app --example canvas_gallery [-- dark]`

use fasttick_app::canvas::{
    BreakpointMarker, CanvasNode, CanvasState, CanvasTreeBuilder, NodeCategory, NodeSource,
    PortLine, ProblemMarker, Substitution, SubtreeInfo, TreeCanvas,
};
use fasttick_app::theme::{self, CanvasPalette, ThemeChoice};
use fasttick_core::runtime::NodeIndex;
use fasttick_core::status::NodeStatus;

fn node(index: usize, label: &str, type_id: &str, category: NodeCategory) -> CanvasNode {
    let mut node = CanvasNode::new(NodeSource::Runtime(NodeIndex(index)), label, type_id);
    node.category = category;
    node
}

fn gallery() -> CanvasState {
    let mut b = CanvasTreeBuilder::new();
    let mut root = node(0, "gallery", "Sequence", NodeCategory::Control);
    root.deco.status = NodeStatus::Running;
    let root = b.push(None, root);

    let mut statuses = node(1, "statuses", "Fallback", NodeCategory::Control);
    statuses.deco.status = NodeStatus::Running;
    let statuses = b.push(Some(root), statuses);
    let cases = [
        ("idle", NodeStatus::Idle, None),
        ("running", NodeStatus::Running, None),
        ("success", NodeStatus::Success, None),
        ("failure", NodeStatus::Failure, None),
        ("skipped", NodeStatus::Skipped, None),
        ("was_success", NodeStatus::Idle, Some(NodeStatus::Success)),
        ("was_failure", NodeStatus::Idle, Some(NodeStatus::Failure)),
    ];
    for (i, (label, status, previous)) in cases.into_iter().enumerate() {
        let mut n = node(10 + i, label, "MoveBase", NodeCategory::Action);
        n.deco.status = status;
        n.deco.previous_status = previous;
        n.ports = vec![PortLine {
            name: "goal".into(),
            value: "{target}".into(),
        }];
        b.push(Some(statuses), n);
    }

    let markers = b.push(Some(root), node(2, "markers", "Sequence", NodeCategory::Control));
    let mut bp = node(20, "breakpoint", "CheckBattery", NodeCategory::Condition);
    bp.deco.breakpoint = Some(BreakpointMarker::Enabled);
    b.push(Some(markers), bp);
    let mut off = node(21, "disabled_bp", "CheckBattery", NodeCategory::Condition);
    off.deco.breakpoint = Some(BreakpointMarker::Disabled);
    b.push(Some(markers), off);
    let mut hit = node(22, "stopped_here", "MoveBase", NodeCategory::Action);
    hit.deco.breakpoint = Some(BreakpointMarker::Hit);
    hit.deco.status = NodeStatus::Running;
    b.push(Some(markers), hit);
    let mut forced = node(23, "forced", "MoveBase", NodeCategory::Action);
    forced.deco.substitution = Some(Substitution::ForcedSuccess);
    b.push(Some(markers), forced);
    let mut failing = node(24, "forced_fail", "MoveBase", NodeCategory::Action);
    failing.deco.substitution = Some(Substitution::ForcedFailure);
    b.push(Some(markers), failing);

    let others = b.push(Some(root), node(3, "others", "Sequence", NodeCategory::Control));
    let mut error = node(30, "error", "UnknownThing", NodeCategory::Unknown);
    error.deco.problem = Some(ProblemMarker::Error);
    b.push(Some(others), error);
    let mut warning = node(31, "warning", "Inverter", NodeCategory::Decorator);
    warning.deco.problem = Some(ProblemMarker::Warning);
    let warning = b.push(Some(others), warning);
    b.push(Some(warning), node(32, "AlwaysSuccess", "AlwaysSuccess", NodeCategory::Action));
    let mut found = node(33, "search_match", "SaySomething", NodeCategory::Action);
    found.deco.search_match = true;
    b.push(Some(others), found);
    let mut chosen = node(34, "selected", "SaySomething", NodeCategory::Action);
    chosen.deco.selected = true;
    b.push(Some(others), chosen);
    let mut sub = node(35, "collapsed", "SubTree", NodeCategory::SubTree);
    sub.subtree = Some(SubtreeInfo {
        tree_id: "Recover".into(),
        expanded: false,
        can_expand: true,
        inner_count: 12,
    });
    b.push(Some(others), sub);

    let mut state = CanvasState::new(b.finish("Gallery"));
    state.set_direction(fasttick_app::canvas::LayoutDirection::LeftRight);
    state
}

struct Gallery {
    state: CanvasState,
}

impl eframe::App for Gallery {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let palette = CanvasPalette::for_theme(ui.ctx().theme());
        egui::CentralPanel::no_frame().show(ui, |ui| {
            TreeCanvas::new(&mut self.state, &palette).show(ui);
        });
    }
}

fn main() -> eframe::Result {
    let theme = match std::env::args().nth(1).as_deref() {
        Some("dark") => ThemeChoice::Dark,
        _ => ThemeChoice::Light,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FastTick canvas gallery")
            .with_inner_size([1100.0, 980.0]),
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "fasttick-canvas-gallery",
        options,
        Box::new(move |cc| {
            fasttick_app::fonts::install(&cc.egui_ctx);
            theme::install(&cc.egui_ctx, theme);
            Ok(Box::new(Gallery { state: gallery() }))
        }),
    )
}
