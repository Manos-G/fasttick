//! Headless UI tests of unconnected nodes: "Delete, keep children" and the
//! dot on the parent side of an unconnected node, which connects it to a
//! new parent.

use std::path::Path;

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use fasttick_app::app::{FastTickApp, Mode, StartOptions};
use fasttick_app::shortcuts::Command;
use fasttick_app::theme::ThemeChoice;

const TREE: &str = r#"<root BTCPP_format="4" main_tree_to_execute="Main">
  <BehaviorTree ID="Main">
    <Sequence name="seq">
      <AlwaysSuccess name="first"/>
      <Fallback name="fb">
        <AlwaysFailure name="a"/>
        <AlwaysSuccess name="b"/>
      </Fallback>
      <AlwaysSuccess name="last"/>
    </Sequence>
  </BehaviorTree>
</root>
"#;

fn harness(path: &Path) -> Harness<'static, FastTickApp> {
    std::fs::write(path, TREE).unwrap();
    let options = StartOptions {
        open: vec![path.to_path_buf()],
        theme: Some(ThemeChoice::Light),
        mode: Some(Mode::Edit),
        connect: None,
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
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

/// The labels of the children of a node.
fn children(harness: &Harness<'static, FastTickApp>, parent: &str) -> Vec<String> {
    let app = harness.state();
    let tree = app.workspace().active().unwrap().canvas.tree();
    let id = tree
        .ids()
        .find(|id| tree.get(*id).unwrap().label == parent)
        .expect("the parent");
    tree.get(id)
        .unwrap()
        .children
        .iter()
        .map(|c| tree.get(*c).unwrap().label.clone())
        .collect()
}

/// The labels of the unconnected nodes (detached roots), in order.
fn unconnected(harness: &Harness<'static, FastTickApp>) -> Vec<String> {
    let app = harness.state();
    let tree = app.workspace().active().unwrap().canvas.tree();
    tree.nodes()
        .iter()
        .filter(|n| n.detached)
        .map(|n| n.label.clone())
        .collect()
}

fn selected(harness: &Harness<'static, FastTickApp>) -> Vec<String> {
    let app = harness.state();
    let tree = app.workspace().active().unwrap().canvas.tree();
    tree.selected()
        .map(|id| tree.get(id).unwrap().label.clone())
        .collect()
}

fn xml(harness: &Harness<'static, FastTickApp>) -> String {
    let app = harness.state();
    let view = app.workspace().active().unwrap();
    let doc = app.workspace().doc(view.doc).unwrap();
    doc.editor.as_ref().unwrap().file().unwrap().to_xml_string()
}

/// The parent dot of a node: the middle of the top edge of its card.
fn parent_dot_of(harness: &Harness<'static, FastTickApp>, label: &str) -> Pos2 {
    let rect = item_rect(harness, label);
    pos2(rect.center().x, rect.min.y)
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

/// The world rectangle of a card.
fn world_rect(harness: &Harness<'static, FastTickApp>, label: &str) -> egui::Rect {
    let app = harness.state();
    let canvas = &app.workspace().active().unwrap().canvas;
    let id = canvas
        .tree()
        .ids()
        .find(|id| canvas.tree().get(*id).unwrap().label == label)
        .expect("the node");
    canvas.layout().rects[id.index()]
}

#[test]
fn delete_keeps_the_children_where_they_were() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    let before = [world_rect(&harness, "a"), world_rect(&harness, "b")];
    harness.get_by_role_and_label(Role::TreeItem, "fb").click();
    harness.run();
    // The Delete key keeps the children.
    harness.key_press(egui::Key::Delete);
    harness.run();
    assert!(!xml(&harness).contains("Fallback"));
    assert_eq!(children(&harness, "seq"), ["first", "last"]);
    assert_eq!(unconnected(&harness), ["a", "b"]);
    // The kept children are selected, and they stay where they were.
    assert_eq!(selected(&harness), ["a", "b"]);
    assert_eq!(world_rect(&harness, "a").min, before[0].min);
    assert_eq!(world_rect(&harness, "b").min, before[1].min);
    // Undo brings the Fallback back with its children.
    command(&mut harness, Command::Undo);
    assert_eq!(children(&harness, "seq"), ["first", "fb", "last"]);
    assert_eq!(children(&harness, "fb"), ["a", "b"]);
    assert!(unconnected(&harness).is_empty());
}

#[test]
fn ctrl_delete_deletes_the_children_too() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    harness.get_by_role_and_label(Role::TreeItem, "fb").click();
    harness.run();
    assert!(harness.state().is_enabled(Command::DeleteWithChildren));
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::Delete);
    harness.run();
    assert_eq!(children(&harness, "seq"), ["first", "last"]);
    assert!(unconnected(&harness).is_empty());
    assert!(!xml(&harness).contains("name=\"a\""));
}

#[test]
fn the_context_menu_deletes_with_or_without_the_children() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    harness
        .get_by_role_and_label(Role::TreeItem, "fb")
        .click_secondary();
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Delete")
        .click();
    harness.run();
    assert_eq!(unconnected(&harness), ["a", "b"]);
    command(&mut harness, Command::Undo);
    harness
        .get_by_role_and_label(Role::TreeItem, "fb")
        .click_secondary();
    harness.run();
    harness.get_by_label("Delete with children").click();
    harness.run();
    assert!(unconnected(&harness).is_empty());
    assert_eq!(children(&harness, "seq"), ["first", "last"]);
    // A leaf has no such entry: it has no children.
    harness
        .get_by_role_and_label(Role::TreeItem, "first")
        .click_secondary();
    harness.run();
    assert!(harness.query_by_label("Delete with children").is_none());
}

#[test]
fn a_drag_to_an_empty_place_moves_or_disconnects_a_node_there() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    let zoom = harness
        .state()
        .workspace()
        .active()
        .unwrap()
        .canvas
        .viewport()
        .zoom;
    // A connected leaf to an empty place: it is unconnected there.
    let old = world_rect(&harness, "last");
    let from = item_rect(&harness, "last").center();
    let to = from + egui::vec2(0.0, 250.0);
    drag(&mut harness, from, to);
    assert_eq!(unconnected(&harness), ["last"]);
    let moved = world_rect(&harness, "last").min - old.min;
    assert!((moved.y - 250.0 / zoom).abs() < 2.0, "{moved:?}");
    assert!(moved.x.abs() < 2.0, "{moved:?}");
    // An unconnected node: it only moves.
    let old = world_rect(&harness, "last");
    let from = item_rect(&harness, "last").center();
    drag(&mut harness, from, from + egui::vec2(-150.0, 30.0));
    assert_eq!(unconnected(&harness), ["last"]);
    let moved = world_rect(&harness, "last").min - old.min;
    assert!((moved.x + 150.0 / zoom).abs() < 2.0, "{moved:?}");
    assert!((moved.y - 30.0 / zoom).abs() < 2.0, "{moved:?}");
}

#[test]
fn the_parent_dot_connects_an_unconnected_node() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    harness.get_by_role_and_label(Role::TreeItem, "fb").click();
    harness.run();
    command(&mut harness, Command::Delete);
    command(&mut harness, Command::FitToView);

    // From the dot of "a" to the Sequence: the last child.
    let from = parent_dot_of(&harness, "a");
    let to = item_rect(&harness, "seq").center();
    drag(&mut harness, from, to);
    assert_eq!(children(&harness, "seq"), ["first", "last", "a"]);
    assert_eq!(unconnected(&harness), ["b"]);
    assert!(xml(&harness).contains(r#"<AlwaysFailure name="a"/>"#));

    // From the dot of "b" to the left half of "last": before it.
    command(&mut harness, Command::FitToView);
    let from = parent_dot_of(&harness, "b");
    let last = item_rect(&harness, "last");
    drag(&mut harness, from, pos2(last.min.x + 4.0, last.center().y));
    assert_eq!(children(&harness, "seq"), ["first", "b", "last", "a"]);
    assert!(unconnected(&harness).is_empty());

    // One undo step for each connection.
    command(&mut harness, Command::Undo);
    assert_eq!(unconnected(&harness), ["b"]);
}

#[test]
fn a_release_on_a_leaf_without_room_or_on_nothing_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.xml");
    std::fs::write(
        &path,
        r#"<root BTCPP_format="4" main_tree_to_execute="Main">
  <BehaviorTree ID="Main">
    <Inverter name="inv">
      <AlwaysSuccess name="only"/>
    </Inverter>
  </BehaviorTree>
</root>
"#,
    )
    .unwrap();
    let options = StartOptions {
        open: vec![path],
        theme: Some(ThemeChoice::Light),
        mode: Some(Mode::Edit),
        connect: None,
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .with_max_steps(64)
        .with_step_dt(1.0 / 30.0)
        .build_eframe(move |cc| FastTickApp::new(cc, options));
    harness.run();
    let view = harness.state().workspace().active_view.unwrap();
    let ctx = harness.ctx.clone();
    harness.state_mut().run_edit(
        &ctx,
        view,
        fasttick_app::edit_ops::EditOp::AddUnconnected {
            type_id: "Sleep".into(),
            position: None,
        },
    );
    harness.run();
    command(&mut harness, Command::FitToView);
    let before = xml(&harness);
    // On the full Inverter and on its only child: no connection.
    for target in ["inv", "only"] {
        let from = parent_dot_of(&harness, "Sleep");
        let to = item_rect(&harness, target).center();
        drag(&mut harness, from, to);
        assert_eq!(xml(&harness), before, "{target}");
        assert_eq!(unconnected(&harness), ["Sleep"]);
    }
    // On an empty place.
    let from = parent_dot_of(&harness, "Sleep");
    drag(&mut harness, from, pos2(1300.0, 850.0));
    assert_eq!(unconnected(&harness), ["Sleep"]);
}
