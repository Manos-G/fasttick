//! Headless UI tests of the whole app with `egui_kittest`.
//!
//! The tests find widgets by their accessibility labels. The canvas
//! describes each visible node as a tree item with the instance name as
//! its label.

use std::path::PathBuf;

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use fasttick_app::app::{FastTickApp, Mode, StartOptions};
use fasttick_app::dock::Tab;
use fasttick_app::shortcuts::Command;
use fasttick_app::theme::ThemeChoice;

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

fn main_xml() -> PathBuf {
    fixture("test_harness/trees/main.xml")
}

fn harness(options: StartOptions) -> Harness<'static, FastTickApp> {
    // The harness sets its own theme on the context after the app starts;
    // give it the theme of the options.
    let theme = match options.theme {
        Some(ThemeChoice::Dark) => egui::Theme::Dark,
        _ => egui::Theme::Light,
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .with_theme(theme)
        .with_max_steps(64)
        .with_step_dt(1.0 / 30.0)
        .build_eframe(move |cc| FastTickApp::new(cc, options));
    harness.run();
    harness
}

fn light(open: Vec<PathBuf>) -> StartOptions {
    StartOptions {
        open,
        theme: Some(ThemeChoice::Light),
        mode: Some(Mode::Edit),
        connect: None,
    }
}

/// Runs a command as the menu does.
fn command(harness: &mut Harness<'static, FastTickApp>, command: Command) {
    let ctx = harness.ctx.clone();
    harness.state_mut().run_command(&ctx, command);
    harness.run();
}

fn tree_item<'a>(
    harness: &'a Harness<'static, FastTickApp>,
    label: &'a str,
) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::TreeItem, label)
}

fn mode_button<'a>(
    harness: &'a Harness<'static, FastTickApp>,
    mode: Mode,
) -> egui_kittest::Node<'a> {
    let label = match mode {
        Mode::Edit => "Edit mode",
        Mode::Live => "Live mode",
        Mode::Replay => "Replay mode",
    };
    // The status bar also shows "Edit mode" as text; the switcher is a
    // button.
    harness.get_by_role_and_label(Role::Button, label)
}

#[test]
fn main_window_renders() {
    let harness = harness(light(Vec::new()));
    for menu in ["File", "View", "Live", "Help"] {
        assert!(
            harness
                .query_all_by_role_and_label(Role::Button, menu)
                .count()
                >= 1,
            "menu {menu}"
        );
    }
    for mode in Mode::ALL {
        mode_button(&harness, mode);
    }
    // The start page of edit mode.
    harness.get_by_label("FastTick");
    harness.get_by_label_contains("Open tree XML");
    // The status bar.
    harness.get_by_role_and_label(Role::Label, "Edit mode");
    harness.get_by_label("Not connected");
    harness.get_by_label("No document");
    // The default panels.
    let app = harness.state();
    for tab in [Tab::Start, Tab::Trees, Tab::Inspector, Tab::Problems] {
        assert!(
            fasttick_app::dock::contains(app.dock(Mode::Edit), tab),
            "{tab:?}"
        );
    }
}

#[test]
fn mode_switching() {
    let mut harness = harness(light(Vec::new()));
    assert_eq!(harness.state().mode(), Mode::Edit);
    mode_button(&harness, Mode::Live).click();
    harness.run();
    assert_eq!(harness.state().mode(), Mode::Live);
    // The live bar and the live start page have a connect form.
    assert_eq!(
        harness
            .query_all_by_role_and_label(Role::Button, "Connect")
            .count(),
        2
    );
    assert_eq!(harness.query_all_by_label("Host").count(), 2);

    mode_button(&harness, Mode::Replay).click();
    harness.run();
    assert_eq!(harness.state().mode(), Mode::Replay);
    harness.get_by_label_contains("Open log");

    // Keyboard shortcut Ctrl+1 goes back to edit mode.
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num1);
    harness.run();
    assert_eq!(harness.state().mode(), Mode::Edit);
}

#[test]
fn opening_the_sample_tree_shows_its_nodes() {
    let mut harness = harness(light(vec![main_xml()]));
    {
        let app = harness.state();
        assert_eq!(app.workspace().docs().len(), 1);
        let view = app.workspace().active().expect("an active view");
        assert_eq!(view.canvas.tree().title, "MainTree");
        // MainTree with the two SubTrees expanded.
        assert_eq!(view.canvas.tree().len(), 43);
    }
    // The root is visible in the first view.
    tree_item(&harness, "main_sequence");
    // The tree list shows the trees of the file.
    for tree in ["MainTree", "Navigate", "Counting", "SecondTree"] {
        harness.get_by_label(tree);
    }

    // After "Fit to view" all nodes are on screen.
    command(&mut harness, Command::FitToView);
    for label in [
        "say_text",
        "navigate",
        "count_in_subtree",
        "read_text_pose",
        "long_wait",
    ] {
        tree_item(&harness, label);
    }
    let visible = harness.query_all_by_role(Role::TreeItem).count();
    assert_eq!(visible, 43);
}

#[test]
fn zoom_and_fit_change_the_viewport() {
    let mut harness = harness(light(vec![main_xml()]));
    let zoom = |h: &Harness<'static, FastTickApp>| {
        h.state()
            .workspace()
            .active()
            .unwrap()
            .canvas
            .viewport()
            .zoom
    };
    let start = zoom(&harness);
    harness.get_by_label("Zoom in").click();
    harness.run();
    let zoomed_in = zoom(&harness);
    assert!(zoomed_in > start * 1.2, "{start} -> {zoomed_in}");

    harness.get_by_label("Zoom out").click();
    harness.run();
    harness.get_by_label("Zoom out").click();
    harness.run();
    assert!(zoom(&harness) < start);

    harness.get_by_label("Fit to view").click();
    harness.run();
    let fitted = harness
        .state()
        .workspace()
        .active()
        .unwrap()
        .canvas
        .viewport();
    let bounds = harness
        .state()
        .workspace()
        .active()
        .unwrap()
        .canvas
        .layout()
        .bounds;
    assert!(fitted.zoom < start, "the main tree is wider than the view");
    // The whole tree is inside the canvas.
    let canvas = harness.get_by_role(Role::Tree).rect();
    let screen = fitted.rect_to_screen(canvas.min, bounds);
    assert!(
        canvas.expand(1.0).contains_rect(screen),
        "{canvas:?} {screen:?}"
    );

    // Ctrl+0: 100 %.
    tree_item(&harness, "main_sequence").click();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num0);
    harness.run();
    assert!((zoom(&harness) - 1.0).abs() < 1e-3);
}

#[test]
fn selection_and_keyboard_navigation() {
    let mut harness = harness(light(vec![main_xml()]));
    tree_item(&harness, "main_sequence").click();
    harness.run();
    {
        let view = harness.state().workspace().active().unwrap();
        let selected: Vec<_> = view.canvas.tree().selected().collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(
            view.canvas.tree().get(selected[0]).unwrap().label,
            "main_sequence"
        );
    }
    // The inspector shows the selected node.
    harness.get_by_label("Read-only. Editing comes with the tree editor.");

    // Arrow down: the first child.
    harness.key_press(egui::Key::ArrowDown);
    harness.run();
    let selected_label = |h: &Harness<'static, FastTickApp>| {
        let view = h.state().workspace().active().unwrap();
        let id = view.canvas.primary().unwrap();
        view.canvas.tree().get(id).unwrap().label.clone()
    };
    assert_eq!(selected_label(&harness), "init");
    harness.key_press(egui::Key::ArrowRight);
    harness.run();
    assert_eq!(selected_label(&harness), "write_values");
    harness.key_press(egui::Key::ArrowUp);
    harness.run();
    assert_eq!(selected_label(&harness), "main_sequence");
    harness.key_press(egui::Key::Escape);
    harness.run();
    assert_eq!(
        harness
            .state()
            .workspace()
            .active()
            .unwrap()
            .canvas
            .tree()
            .selected_count(),
        0
    );

    // Select all.
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.run();
    assert_eq!(
        harness
            .state()
            .workspace()
            .active()
            .unwrap()
            .canvas
            .tree()
            .selected_count(),
        43
    );
}

#[test]
fn double_click_collapses_a_subtree() {
    let mut harness = harness(light(vec![main_xml()]));
    command(&mut harness, Command::FitToView);
    command(&mut harness, Command::ZoomReset);
    let count = |h: &Harness<'static, FastTickApp>| {
        h.state().workspace().active().unwrap().canvas.tree().len()
    };
    assert_eq!(count(&harness), 43);
    let center = tree_item(&harness, "navigate").rect().center();
    for _ in 0..2 {
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: center,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
    }
    harness.run();
    // "navigate" hides its 4 inner nodes.
    assert_eq!(count(&harness), 39);
    // The SubTree node stays where it was.
    let after = tree_item(&harness, "navigate").rect().center();
    assert!((after - center).length() < 1.0, "{center:?} -> {after:?}");
}

#[test]
fn split_view_and_close() {
    let mut harness = harness(light(vec![main_xml()]));
    command(&mut harness, Command::SplitView);
    assert_eq!(harness.state().workspace().views().len(), 2);
    let docs = fasttick_app::dock::document_views(harness.state().dock(Mode::Edit));
    assert_eq!(docs.len(), 2);
    command(&mut harness, Command::CloseTab);
    assert_eq!(harness.state().workspace().views().len(), 1);
    command(&mut harness, Command::CloseTab);
    assert!(harness.state().workspace().docs().is_empty());
    harness.get_by_label("No document");
}

#[test]
fn tree_list_switches_trees() {
    let mut harness = harness(light(vec![main_xml()]));
    harness.get_by_label("SecondTree").click();
    harness.run();
    let view = harness.state().workspace().active().unwrap();
    assert_eq!(view.canvas.tree().title, "SecondTree");
    assert_eq!(view.canvas.tree().len(), 3);
}

#[test]
fn dialogs_and_theme() {
    let mut harness = harness(light(Vec::new()));
    harness.key_press(egui::Key::F1);
    harness.run();
    // The window title and the entry of the Help group.
    assert_eq!(harness.query_all_by_label("Keyboard shortcuts").count(), 2);
    harness.get_by_role_and_label(Role::Label, "Fit to view");
    harness.get_by_role_and_label(Role::Label, "Ctrl+Shift+O");
    harness.key_press(egui::Key::Escape);
    harness.run();

    command(&mut harness, Command::About);
    harness.get_by_label("THIRD_PARTY_LICENSES.md");
    harness.get_by_role_and_label(Role::Button, "Close").click();
    harness.run();
    assert!(harness.query_by_label("THIRD_PARTY_LICENSES.md").is_none());

    assert_eq!(harness.ctx.theme(), egui::Theme::Light);
    command(&mut harness, Command::ThemeDark);
    assert_eq!(harness.ctx.theme(), egui::Theme::Dark);
    assert_eq!(harness.state().settings().theme, ThemeChoice::Dark);

    command(&mut harness, Command::Connect);
    harness.get_by_label("Connect to an executor");
    harness
        .get_by_role_and_label(Role::Button, "Cancel")
        .click();
    harness.run();
    assert!(harness.query_by_label("Connect to an executor").is_none());
}

#[test]
fn replay_log_shows_statuses() {
    let options = StartOptions {
        open: vec![fixture("tests/fixtures/harness/main.btlog")],
        theme: Some(ThemeChoice::Dark),
        mode: None,
        connect: None,
    };
    let mut harness = harness(options);
    assert_eq!(harness.state().mode(), Mode::Replay);
    let doc = &harness.state().workspace().docs()[0];
    let replay = doc.replay.as_ref().unwrap();
    assert_eq!(replay.cursor().applied(), replay.index().len());
    // At the end of the log, the root finished with SUCCESS and is IDLE.
    {
        let view = harness.state().workspace().active().unwrap();
        let root = view
            .canvas
            .tree()
            .get(view.canvas.tree().root().unwrap())
            .unwrap();
        assert_eq!(root.deco.status, fasttick_core::status::NodeStatus::Idle);
        assert_eq!(
            root.deco.previous_status,
            Some(fasttick_core::status::NodeStatus::Success)
        );
    }
    // Go to the start: all nodes are IDLE.
    harness.get_by_label("Go to the start").click();
    harness.run();
    let view = harness.state().workspace().active().unwrap();
    assert!(
        view.canvas
            .tree()
            .nodes()
            .iter()
            .all(|n| n.deco.status == fasttick_core::status::NodeStatus::Idle
                && n.deco.previous_status.is_none())
    );
    harness.get_by_label("One transition forward").click();
    harness.run();
    let view = harness.state().workspace().active().unwrap();
    assert!(
        view.canvas
            .tree()
            .nodes()
            .iter()
            .any(|n| n.deco.status == fasttick_core::status::NodeStatus::Running)
    );
}

#[test]
fn bad_file_shows_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.xml");
    std::fs::write(&bad, "<root>").unwrap();
    let harness = harness(light(vec![bad]));
    assert_eq!(harness.state().notices().len(), 1);
    harness.get_by_label_contains("Cannot open");
    assert!(harness.state().workspace().docs().is_empty());
}

#[test]
fn notice_close_button_removes_the_notice() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.xml");
    std::fs::write(&bad, "<root>").unwrap();
    let mut harness = harness(light(vec![bad]));
    harness.run();
    assert_eq!(harness.state().notices().len(), 1);
    harness.get_by_role_and_label(Role::Button, "Close").click();
    harness.run();
    assert!(harness.state().notices().is_empty());
    assert!(harness.query_by_label_contains("Cannot open").is_none());
}

#[test]
fn main_tree_to_execute_is_shown_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two.xml");
    std::fs::write(
        &path,
        r#"<root BTCPP_format="4" main_tree_to_execute="Second">
            <BehaviorTree ID="First"><AlwaysSuccess name="first_leaf"/></BehaviorTree>
            <BehaviorTree ID="Second"><AlwaysFailure name="second_leaf"/></BehaviorTree>
        </root>"#,
    )
    .unwrap();
    let harness = harness(light(vec![path]));
    let view = harness.state().workspace().active().unwrap();
    assert_eq!(view.canvas.tree().title, "Second");
    tree_item(&harness, "second_leaf");
    assert!(harness.query_by_label("first_leaf").is_none());
}
