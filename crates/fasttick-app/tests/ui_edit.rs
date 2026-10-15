//! Headless UI tests of edit mode: drag and drop, the context menu, the
//! name prompt and undo. The tests open the test tree of the harness but
//! never save it.

use std::path::PathBuf;

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use fasttick_app::app::{FastTickApp, Mode, StartOptions};
use fasttick_app::shortcuts::Command;
use fasttick_app::theme::ThemeChoice;

fn main_xml() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test_harness/trees/main.xml")
}

fn harness() -> Harness<'static, FastTickApp> {
    let options = StartOptions {
        open: vec![main_xml()],
        theme: Some(ThemeChoice::Light),
        mode: Some(Mode::Edit),
        connect: None,
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .with_theme(egui::Theme::Light)
        .with_max_steps(64)
        .with_step_dt(1.0 / 30.0)
        .build_eframe(move |cc| FastTickApp::new(cc, options));
    harness.run();
    command(&mut harness, Command::FitToView);
    harness
}

fn command(harness: &mut Harness<'static, FastTickApp>, command: Command) {
    let ctx = harness.ctx.clone();
    harness.state_mut().run_command(&ctx, command);
    harness.run();
}

fn item_rect(harness: &Harness<'static, FastTickApp>, label: &str) -> egui::Rect {
    harness.get_by_role_and_label(Role::TreeItem, label).rect()
}

/// The labels of the children of the node with label `parent`.
fn children(harness: &Harness<'static, FastTickApp>, parent: &str) -> Vec<String> {
    let app = harness.state();
    let tree = app.workspace().active().unwrap().canvas.tree();
    let id = tree
        .ids()
        .find(|id| tree.get(*id).unwrap().label == parent)
        .expect("parent node");
    tree.get(id)
        .unwrap()
        .children
        .iter()
        .map(|c| tree.get(*c).unwrap().label.clone())
        .collect()
}

/// The XML text of the open document.
fn xml(harness: &Harness<'static, FastTickApp>) -> String {
    let app = harness.state();
    let view = app.workspace().active().unwrap();
    let doc = app.workspace().doc(view.doc).unwrap();
    doc.editor.as_ref().unwrap().file().unwrap().to_xml_string()
}

fn is_dirty(harness: &Harness<'static, FastTickApp>) -> bool {
    let app = harness.state();
    let view = app.workspace().active().unwrap();
    let doc = app.workspace().doc(view.doc).unwrap();
    doc.editor.as_ref().unwrap().is_dirty()
}

/// A primary-button drag from `from` to `to` in small steps.
fn drag(harness: &mut Harness<'static, FastTickApp>, from: Pos2, to: Pos2) {
    harness.event(Event::PointerMoved(from));
    harness.event(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.step();
    for i in 1..=8 {
        let t = i as f32 / 8.0;
        harness.event(Event::PointerMoved(from + (to - from) * t));
        harness.step();
    }
    harness.event(Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.run();
}

/// A right-click on the node with a label.
fn right_click(harness: &mut Harness<'static, FastTickApp>, label: &str) {
    let pos = item_rect(harness, label).center();
    harness.event(Event::PointerMoved(pos));
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run();
}

#[test]
fn drag_moves_a_node_after_a_sibling_and_undo_restores_it() {
    let mut harness = harness();
    let before = children(&harness, "main_sequence");
    let a = before.iter().position(|l| l == "say_default").unwrap();
    assert_eq!(before[a + 1], "say_text");

    let from = item_rect(&harness, "say_default").center();
    let target = item_rect(&harness, "say_text");
    // The right end of the card: after the target.
    let to = pos2(target.max.x - target.width() * 0.1, target.center().y);
    drag(&mut harness, from, to);

    let after = children(&harness, "main_sequence");
    assert_eq!(after.len(), before.len());
    let b = after.iter().position(|l| l == "say_text").unwrap();
    assert_eq!(after[b + 1], "say_default", "{after:?}");
    assert!(is_dirty(&harness));
    let text = xml(&harness);
    assert!(text.find("name=\"say_text\"").unwrap() < text.find("name=\"say_default\"").unwrap());

    command(&mut harness, Command::Undo);
    assert_eq!(children(&harness, "main_sequence"), before);
    assert!(!is_dirty(&harness));
    command(&mut harness, Command::Redo);
    assert_eq!(children(&harness, "main_sequence"), after);
}

#[test]
fn drag_into_a_control_node_and_onto_an_empty_place() {
    let mut harness = harness();
    let from = item_rect(&harness, "say_default").center();
    // The middle of the Fallback card: inside, as the last child.
    let fallback = item_rect(&harness, "fallback").center();
    drag(&mut harness, from, fallback);
    assert_eq!(
        children(&harness, "fallback"),
        ["check_blocked", "check_flag", "say_default"]
    );

    // An empty place below the tree: the node becomes unconnected.
    let from = item_rect(&harness, "say_default").center();
    let canvas = harness.get_by_role(Role::Tree).rect();
    let empty = pos2(canvas.center().x, canvas.max.y - 30.0);
    drag(&mut harness, from, empty);
    assert_eq!(
        children(&harness, "fallback"),
        ["check_blocked", "check_flag"]
    );
    let app = harness.state();
    let tree = app.workspace().active().unwrap().canvas.tree();
    let node = tree
        .ids()
        .find(|id| tree.get(*id).unwrap().label == "say_default")
        .map(|id| tree.get(id).unwrap())
        .expect("the node stays in the view");
    assert!(node.parent.is_none(), "unconnected nodes are roots");
}

#[test]
fn escape_cancels_a_move() {
    let mut harness = harness();
    let before = children(&harness, "main_sequence");
    let from = item_rect(&harness, "say_default").center();
    let to = item_rect(&harness, "fallback").center();
    harness.event(Event::PointerMoved(from));
    harness.event(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.step();
    for i in 1..=4 {
        harness.event(Event::PointerMoved(from + (to - from) * (i as f32 / 4.0)));
        harness.step();
    }
    harness.key_press(egui::Key::Escape);
    harness.event(Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.run();
    assert_eq!(children(&harness, "main_sequence"), before);
    assert!(!is_dirty(&harness));
}

#[test]
fn context_menu_deletes_and_pastes() {
    let mut harness = harness();
    right_click(&mut harness, "check_flag");
    harness.get_by_role_and_label(Role::Button, "Copy").click();
    harness.run();
    right_click(&mut harness, "check_flag");
    harness
        .get_by_role_and_label(Role::Button, "Delete")
        .click();
    harness.run();
    assert_eq!(children(&harness, "fallback"), ["check_blocked"]);

    // Paste the copy into the Fallback.
    right_click(&mut harness, "fallback");
    harness
        .get_by_role_and_label(Role::Button, "Paste as child")
        .click();
    harness.run();
    assert_eq!(
        children(&harness, "fallback"),
        ["check_blocked", "check_flag"]
    );
}

#[test]
fn context_menu_adds_a_child_node() {
    let mut harness = harness();
    right_click(&mut harness, "fallback");
    harness.get_by_label_contains("Add child").hover();
    harness.run();
    harness.get_by_label_contains("Actions").hover();
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "AlwaysSuccess")
        .click();
    harness.run();
    assert_eq!(
        children(&harness, "fallback"),
        ["check_blocked", "check_flag", "AlwaysSuccess"]
    );
    // The new node is selected.
    let app = harness.state();
    let view = app.workspace().active().unwrap();
    let primary = view.canvas.primary().unwrap();
    assert_eq!(
        view.canvas.tree().get(primary).unwrap().label,
        "AlwaysSuccess"
    );
}

#[test]
fn create_subtree_from_selection_asks_for_a_name() {
    let mut harness = harness();
    right_click(&mut harness, "fallback");
    harness
        .get_by_label("Create SubTree from selection…")
        .click();
    harness.run();
    // A taken ID cannot be used.
    harness.get_by_role(Role::TextInput).type_text("Navigate");
    harness.run();
    harness.get_by_label_contains("A tree with the ID Navigate exists.");
    harness.get_by_role(Role::TextInput).focus();
    for _ in 0.."Navigate".len() {
        harness.key_press(egui::Key::Backspace);
    }
    harness.run();
    harness.get_by_role(Role::TextInput).type_text("Checks");
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Create")
        .click();
    harness.run();

    let app = harness.state();
    let view = app.workspace().active().unwrap();
    let doc = app.workspace().doc(view.doc).unwrap();
    let trees: Vec<String> = doc
        .editor
        .as_ref()
        .unwrap()
        .trees()
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    assert!(trees.contains(&"Checks".to_owned()), "{trees:?}");
    let text = doc.editor.as_ref().unwrap().file().unwrap().to_xml_string();
    assert!(text.contains("<SubTree ID=\"Checks\""), "{text}");
    assert!(text.contains("<BehaviorTree ID=\"Checks\">"), "{text}");
}
