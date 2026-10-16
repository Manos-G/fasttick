//! Headless UI tests of node notes, groups and canvas notes.

use std::path::Path;

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use fasttick_app::app::{FastTickApp, Mode, StartOptions};
use fasttick_app::canvas::EditTarget;
use fasttick_app::canvas::annotate::MARGIN;
use fasttick_app::shortcuts::Command;
use fasttick_app::theme::ThemeChoice;

const TREE: &str = r#"<root BTCPP_format="4" main_tree_to_execute="Main">
  <BehaviorTree ID="Main" _group_1="Pick" _group_1_description="Moves the arm." _note_1="Read me" _note_1_box="-400 -200 220 90">
    <Sequence name="seq">
      <AlwaysSuccess name="first"/>
      <Fallback name="fb">
        <AlwaysFailure name="a" _group="1"/>
        <AlwaysSuccess name="b" _group="1"/>
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

fn xml(harness: &Harness<'static, FastTickApp>) -> String {
    let app = harness.state();
    let view = app.workspace().active().unwrap();
    let doc = app.workspace().doc(view.doc).unwrap();
    doc.editor.as_ref().unwrap().file().unwrap().to_xml_string()
}

fn canvas<'a>(harness: &'a Harness<'static, FastTickApp>) -> &'a fasttick_app::canvas::CanvasState {
    &harness.state().workspace().active().unwrap().canvas
}

fn item_rect(harness: &Harness<'static, FastTickApp>, label: &str) -> Rect {
    harness.get_by_role_and_label(Role::TreeItem, label).rect()
}

/// The world rectangle of a card.
fn world_rect(harness: &Harness<'static, FastTickApp>, label: &str) -> Rect {
    let canvas = canvas(harness);
    let id = canvas
        .tree()
        .ids()
        .find(|id| canvas.tree().get(*id).unwrap().label == label)
        .expect("the node");
    canvas.layout().rects[id.index()]
}

/// The screen position of a world position (from a card that is on the
/// screen and in the world).
fn to_screen(harness: &Harness<'static, FastTickApp>, world: Pos2) -> Pos2 {
    let screen = item_rect(harness, "seq");
    let world_seq = world_rect(harness, "seq");
    let zoom = canvas(harness).viewport().zoom;
    screen.min + (world - world_seq.min) * zoom
}

/// The world position of a screen position.
fn to_world(harness: &Harness<'static, FastTickApp>, screen: Pos2) -> Pos2 {
    let screen_seq = item_rect(harness, "seq");
    let world_seq = world_rect(harness, "seq");
    let zoom = canvas(harness).viewport().zoom;
    world_seq.min + (screen - screen_seq.min) / zoom
}

/// The labels of the members of a group (on the canvas).
fn members(harness: &Harness<'static, FastTickApp>, group: u32) -> Vec<String> {
    let canvas = canvas(harness);
    canvas
        .group_boxes()
        .iter()
        .find(|g| g.id == group)
        .map(|g| {
            g.members
                .iter()
                .map(|m| canvas.tree().get(*m).unwrap().label.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn group_rect(harness: &Harness<'static, FastTickApp>, group: u32) -> Rect {
    canvas(harness)
        .group_boxes()
        .iter()
        .find(|g| g.id == group)
        .expect("the group")
        .rect
}

/// A primary-button drag from `from` to `to` in small steps. With
/// `escape`, Escape is pressed before the release.
fn drag(harness: &mut Harness<'static, FastTickApp>, from: Pos2, to: Pos2, escape: bool) {
    harness.event(Event::PointerMoved(from));
    harness.step();
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
    if escape {
        harness.key_press(egui::Key::Escape);
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

/// Checks that no card of another node is in a group box.
fn assert_boxes_are_free(harness: &Harness<'static, FastTickApp>) {
    let canvas = canvas(harness);
    for group in canvas.group_boxes() {
        for (index, rect) in canvas.layout().rects.iter().enumerate() {
            if group.members.iter().any(|m| m.index() == index) {
                continue;
            }
            let label = &canvas.tree().nodes()[index].label;
            assert!(
                !rect.intersects(group.rect),
                "{label} is in the box of group {}",
                group.id
            );
        }
        // The box is around the member cards, with the margin.
        let content = group
            .members
            .iter()
            .map(|m| canvas.layout().rects[m.index()])
            .fold(Rect::NOTHING, |b, r| b.union(r));
        assert_eq!(group.rect.max, content.max + vec2(MARGIN, MARGIN));
        assert_eq!(group.rect.min.x, content.min.x - MARGIN);
        assert!(
            group.rect.min.y < content.min.y - MARGIN,
            "the header is above"
        );
    }
}

#[test]
fn groups_show_with_their_text_and_keep_other_cards_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    assert_eq!(members(&harness, 1), ["a", "b"]);
    let group = &canvas(&harness).group_boxes()[0];
    assert_eq!(group.title, "Pick");
    assert_eq!(group.lines, ["Moves the arm."]);
    assert_boxes_are_free(&harness);
    // The same in the left-right layout.
    command(&mut harness, Command::ToggleLayout);
    assert_boxes_are_free(&harness);
}

#[test]
fn a_new_group_from_the_selection_with_ctrl_g() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    harness
        .get_by_role_and_label(Role::TreeItem, "first")
        .click();
    harness.run();
    harness
        .get_by_role_and_label(Role::TreeItem, "last")
        .click_modifiers(Modifiers::SHIFT);
    harness.run();
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::G);
    harness.run();
    let text = xml(&harness);
    assert!(text.contains(r#"_group_2="Group""#), "{text}");
    assert!(text.contains(r#"<AlwaysSuccess name="first" _group="2"/>"#));
    assert!(text.contains(r#"<AlwaysSuccess name="last" _group="2"/>"#));
    // The title opens for editing, with the old title selected: typing
    // replaces it.
    assert_eq!(canvas(&harness).editing(), Some(EditTarget::GroupTitle(2)));
    assert_eq!(canvas(&harness).selected_group(), Some(2));
    harness.event(Event::Text("Start and end".into()));
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert!(xml(&harness).contains(r#"_group_2="Start and end""#));
    assert_eq!(canvas(&harness).editing(), None);
    // The box shows the new title. "fb" is between the members: the box
    // goes around it (it is not taken out of the box).
    assert_eq!(members(&harness, 2), ["first", "last"]);
    assert_eq!(
        canvas(&harness)
            .group_boxes()
            .iter()
            .find(|g| g.id == 2)
            .unwrap()
            .title,
        "Start and end"
    );
    // Undo: the title, then the group.
    command(&mut harness, Command::Undo);
    assert!(xml(&harness).contains(r#"_group_2="Group""#));
    command(&mut harness, Command::Undo);
    assert_eq!(xml(&harness), TREE);
}

#[test]
fn a_resized_group_takes_the_cards_inside_and_snaps() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    // A drag of the left side alone finds no more cards: the group is
    // below "first" and "fb". Nothing changes.
    let rect = group_rect(&harness, 1);
    let from = to_screen(&harness, pos2(rect.min.x, rect.center().y));
    let first = item_rect(&harness, "first");
    drag(
        &mut harness,
        from,
        pos2(first.center().x - 20.0, from.y),
        false,
    );
    assert_eq!(members(&harness, 1), ["a", "b"]);
    // The top-left corner over the centre of "first" (and "fb" on the way).
    let rect = group_rect(&harness, 1);
    let from = to_screen(&harness, rect.min);
    let to = first.center() - vec2(20.0, 20.0);
    // The cards whose centre is in the dragged area join. "last" is in the
    // area if the box of the group reaches below its centre.
    let area = Rect::from_min_max(to_world(&harness, to), rect.max);
    let last_inside = area.contains(world_rect(&harness, "last").center());
    drag(&mut harness, from, to, false);
    let text = xml(&harness);
    assert!(
        text.contains(r#"<AlwaysSuccess name="first" _group="1"/>"#),
        "{text}"
    );
    assert!(
        text.contains(r#"<Fallback name="fb" _group="1">"#),
        "{text}"
    );
    let mut expected = vec!["first", "fb", "a", "b"];
    if last_inside {
        expected.push("last");
    }
    assert_eq!(members(&harness, 1), expected);
    assert_boxes_are_free(&harness);

    // Escape during a drag cancels it.
    let rect = group_rect(&harness, 1);
    let from = to_screen(&harness, pos2(rect.max.x, rect.center().y));
    drag(&mut harness, from, from + vec2(400.0, 0.0), true);
    assert_eq!(members(&harness, 1), expected);

    // Undo: the group as before.
    command(&mut harness, Command::Undo);
    assert_eq!(members(&harness, 1), ["a", "b"]);
}

#[test]
fn a_smaller_group_lets_cards_go_and_a_drag_to_nothing_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    // The right side to the left of the centre of "b".
    let rect = group_rect(&harness, 1);
    let from = to_screen(&harness, pos2(rect.max.x, rect.center().y));
    let b = item_rect(&harness, "b");
    drag(&mut harness, from, pos2(b.center().x - 10.0, from.y), false);
    assert_eq!(members(&harness, 1), ["a"]);
    assert!(xml(&harness).contains(r#"<AlwaysSuccess name="b"/>"#));
    assert_boxes_are_free(&harness);
    // A box without cards: nothing changes (a group cannot be empty).
    let rect = group_rect(&harness, 1);
    let from = to_screen(&harness, pos2(rect.max.x, rect.max.y));
    let corner = to_screen(&harness, rect.min);
    drag(&mut harness, from, corner + vec2(10.0, 10.0), false);
    assert_eq!(members(&harness, 1), ["a"]);
}

#[test]
fn the_group_menu_edits_and_deletes_a_group() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    let rect = group_rect(&harness, 1);
    let header = to_screen(&harness, rect.min + vec2(rect.width() * 0.5, 12.0));
    harness.event(Event::PointerMoved(header));
    harness.event(Event::PointerButton {
        pos: header,
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.event(Event::PointerButton {
        pos: header,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.run();
    assert_eq!(canvas(&harness).selected_group(), Some(1));
    // The inspector shows the group.
    assert!(harness.query_by_label("Group  ·  2 nodes").is_some());
    // The menu and the inspector both have "Delete group".
    assert_eq!(harness.query_all_by_label("Delete group").count(), 2);
    harness
        .query_all_by_label("Delete group")
        .next()
        .unwrap()
        .click();
    harness.run();
    let text = xml(&harness);
    assert!(!text.contains("_group"), "{text}");
    assert!(canvas(&harness).group_boxes().is_empty());
    command(&mut harness, Command::Undo);
    assert_eq!(xml(&harness), TREE);
}

#[test]
fn a_member_that_moves_away_leaves_its_group() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    // Drag "b" onto the right half of "last": after "last", far from "a".
    let from = item_rect(&harness, "b").center();
    let last = item_rect(&harness, "last");
    drag(
        &mut harness,
        from,
        pos2(last.max.x - 8.0, last.center().y),
        false,
    );
    let text = xml(&harness);
    assert!(text.contains(r#"<AlwaysSuccess name="b"/>"#), "{text}");
    assert_eq!(members(&harness, 1), ["a"]);
    // The last member leaves with "Remove from group": the group goes away.
    harness
        .get_by_role_and_label(Role::TreeItem, "a")
        .click_secondary();
    harness.run();
    harness.get_by_label("Remove from group").click();
    harness.run();
    assert!(!xml(&harness).contains("_group"));
}

#[test]
fn a_node_note_in_the_inspector() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    harness
        .get_by_role_and_label(Role::TreeItem, "first")
        .click();
    harness.run();
    let field = harness
        .query_all_by_label("Note")
        .find(|n| n.accesskit_node().role() == Role::MultilineTextInput)
        .expect("the note field");
    field.focus();
    harness.run();
    harness.event(Event::Text("Starts the run".into()));
    harness.run();
    assert!(xml(&harness).contains(r#"<AlwaysSuccess name="first" _note="Starts the run"/>"#));
    let canvas = canvas(&harness);
    let id = canvas
        .tree()
        .ids()
        .find(|id| canvas.tree().get(*id).unwrap().label == "first")
        .unwrap();
    assert_eq!(
        canvas.tree().get(id).unwrap().note.as_deref(),
        Some("Starts the run")
    );
}

#[test]
fn canvas_notes_are_added_moved_resized_and_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    // The note of the file.
    let notes = canvas(&harness).note_boxes().to_vec();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].lines, ["Read me"]);

    // Right-click an empty place (left of the tree): New note.
    let seq = item_rect(&harness, "seq");
    let first = item_rect(&harness, "first");
    let place = pos2(first.min.x - 250.0, seq.center().y);
    harness.event(Event::PointerMoved(place));
    harness.event(Event::PointerButton {
        pos: place,
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.event(Event::PointerButton {
        pos: place,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "New note")
        .click();
    harness.run();
    assert_eq!(canvas(&harness).editing(), Some(EditTarget::Note(2)));
    harness.event(Event::Text("Check the gripper".into()));
    harness.run();
    harness.key_press(egui::Key::Escape);
    harness.run();
    assert_eq!(canvas(&harness).editing(), None);
    let text = xml(&harness);
    assert!(text.contains(r#"_note_2="Check the gripper""#), "{text}");
    let zoom = canvas(&harness).viewport().zoom;
    let stored = |harness: &Harness<'static, FastTickApp>| {
        canvas(harness)
            .note_boxes()
            .iter()
            .find(|n| n.id == 2)
            .unwrap()
            .stored
    };
    let before = stored(&harness);
    assert_eq!(before.size(), vec2(220.0, 90.0));

    // A drag inside moves the note; the size stays.
    let from = to_screen(&harness, before.center());
    drag(&mut harness, from, from + vec2(60.0, 30.0), false);
    let moved = stored(&harness);
    assert!(
        ((moved.min.x - before.min.x) - 60.0 / zoom).abs() < 1.5,
        "{moved:?}"
    );
    assert!(
        ((moved.min.y - before.min.y) - 30.0 / zoom).abs() < 1.5,
        "{moved:?}"
    );
    assert_eq!(moved.size(), before.size());

    // A drag of the bottom side makes it higher.
    let from = to_screen(&harness, pos2(moved.center().x, moved.max.y));
    drag(&mut harness, from, from + vec2(0.0, 80.0), false);
    let higher = stored(&harness);
    assert!(
        ((higher.height() - moved.height()) - 80.0 / zoom).abs() < 1.5,
        "{higher:?}"
    );
    assert_eq!(higher.min, moved.min);
    assert_eq!(higher.width(), moved.width());

    // Select it and press Delete.
    assert_eq!(canvas(&harness).selected_note(), Some(2));
    harness.key_press(egui::Key::Delete);
    harness.run();
    assert!(!xml(&harness).contains("_note_2"));
    assert!(xml(&harness).contains("_note_1"));
}

#[test]
fn the_svg_export_shows_groups_and_notes() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(&dir.path().join("t.xml"));
    let ctx = harness.ctx.clone();
    let palette = fasttick_app::theme::CanvasPalette::light();
    let svg = harness
        .state_mut()
        .workspace_mut()
        .active_mut()
        .unwrap()
        .canvas
        .to_svg(&ctx, &palette);
    assert!(svg.contains(">Pick</tspan>"), "{svg}");
    assert!(svg.contains(">Moves the arm.</tspan>"));
    assert!(svg.contains(">Read me</tspan>"));
}
