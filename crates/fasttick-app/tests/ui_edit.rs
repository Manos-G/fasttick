//! Headless UI tests of edit mode: drag and drop, the context menu, the
//! name prompt and undo. The tests open the test tree of the harness but
//! never save it.

use std::path::PathBuf;

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use fasttick_app::app::{FastTickApp, Mode, StartOptions};
use fasttick_app::dock::Tab;
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
    harness.get_by_label("Add child…").click();
    harness.run();
    // The search popup opens; a fuzzy search finds the type and Enter adds it.
    assert!(harness.state().add_popup().is_some());
    type_into(
        &mut harness,
        Field::Hint("Search node types"),
        "alwsu",
        false,
    );
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert!(harness.state().add_popup().is_none());
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

fn is_text_field(node: &egui_kittest::Node<'_>) -> bool {
    matches!(
        node.accesskit_node().role(),
        Role::TextInput | Role::MultilineTextInput
    )
}

/// The text field (one line or more) with an accessible label.
fn input<'a>(harness: &'a Harness<'static, FastTickApp>, label: &'a str) -> egui_kittest::Node<'a> {
    let mut found = harness.query_all_by_label(label).filter(is_text_field);
    let node = found
        .next()
        .unwrap_or_else(|| panic!("a text field with the label {label:?}"));
    assert!(found.next().is_none(), "more than one field {label:?}");
    node
}

/// The text field with this hint text.
fn input_with_hint<'a>(
    harness: &'a Harness<'static, FastTickApp>,
    hint: &str,
) -> egui_kittest::Node<'a> {
    harness
        .query_all_by_role(Role::TextInput)
        .chain(harness.query_all_by_role(Role::MultilineTextInput))
        .find(|n| n.accesskit_node().placeholder() == Some(hint))
        .unwrap_or_else(|| panic!("a text field with the hint {hint:?}"))
}

/// How a test finds a text field.
#[derive(Clone, Copy)]
enum Field<'s> {
    /// By its accessible label.
    Label(&'s str),
    /// By its hint text.
    Hint(&'s str),
}

impl<'s> Field<'s> {
    fn find<'a>(self, harness: &'a Harness<'static, FastTickApp>) -> egui_kittest::Node<'a>
    where
        's: 'a,
    {
        match self {
            Field::Label(label) => input(harness, label),
            Field::Hint(hint) => input_with_hint(harness, hint),
        }
    }
}

/// Focuses a text field, optionally selects all its text, and types.
fn type_into(
    harness: &mut Harness<'static, FastTickApp>,
    field: Field<'_>,
    text: &str,
    replace: bool,
) {
    field.find(harness).focus();
    harness.run();
    if replace {
        harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
        harness.run();
    }
    field.find(harness).type_text(text);
    harness.run();
}

/// Selects a node by a click on its card.
fn select(harness: &mut Harness<'static, FastTickApp>, label: &str) {
    harness.get_by_role_and_label(Role::TreeItem, label).click();
    harness.run();
}

#[test]
fn create_subtree_from_selection_asks_for_a_name() {
    let mut harness = harness();
    right_click(&mut harness, "fallback");
    harness
        .get_by_label("Create SubTree from selection…")
        .click();
    harness.run();
    // The prompt has the focus. A taken ID cannot be used.
    assert!(input(&harness, "Tree ID").is_focused());
    input(&harness, "Tree ID").type_text("Navigate");
    harness.run();
    harness.get_by_label_contains("A tree with the ID Navigate exists.");
    type_into(&mut harness, Field::Label("Tree ID"), "Checks", true);
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

#[test]
fn inspector_edits_a_port_and_undo_joins_the_keystrokes() {
    let mut harness = harness();
    select(&mut harness, "say_text");
    assert_eq!(
        input(&harness, "message").value().as_deref(),
        Some("Hello & welcome")
    );
    type_into(&mut harness, Field::Label("message"), "B", true);
    for text in ["y", "e"] {
        input(&harness, "message").type_text(text);
        harness.run();
    }
    let text = xml(&harness);
    assert!(text.contains("message=\"Bye\""), "{text}");
    // The card shows the new value.
    assert_eq!(input(&harness, "message").value().as_deref(), Some("Bye"));
    // One undo step for the whole text.
    command(&mut harness, Command::Undo);
    let text = xml(&harness);
    assert!(text.contains("message=\"Hello &amp; welcome\""), "{text}");
}

#[test]
fn inspector_sets_the_name_and_a_condition() {
    let mut harness = harness();
    select(&mut harness, "say_default");
    type_into(&mut harness, Field::Label("Name"), "greet", true);
    assert!(xml(&harness).contains("name=\"greet\""));
    // The card shows the new name.
    harness.get_by_role_and_label(Role::TreeItem, "greet");

    harness
        .get_by_label_contains("Pre- and post-conditions")
        .click();
    harness.run();
    type_into(&mut harness, Field::Label("_skipIf"), "x == 1", false);
    let text = xml(&harness);
    assert!(
        text.contains("<SayMessage name=\"greet\" _skipIf=\"x == 1\"/>"),
        "{text}"
    );
    // The card shows the condition.
    {
        let app = harness.state();
        let tree = app.workspace().active().unwrap().canvas.tree();
        let node = tree.nodes().iter().find(|n| n.label == "greet").unwrap();
        assert_eq!(node.conditions.len(), 1);
        assert_eq!(node.card.conditions.len(), 1);
        assert_eq!(node.card.conditions[0].name, "_skipIf");
        assert_eq!(node.card.conditions[0].value, ["x == 1"]);
    }
    // An empty script removes the attribute.
    type_into(&mut harness, Field::Label("_skipIf"), "", true);
    harness.key_press(egui::Key::Backspace);
    harness.run();
    assert!(
        xml(&harness).contains("<SayMessage name=\"greet\"/>"),
        "{}",
        xml(&harness)
    );
}

#[test]
fn palette_drag_adds_a_node_and_double_click_adds_to_the_selection() {
    let mut harness = harness();
    harness.state_mut().show_panel(Tab::Palette);
    harness.run();
    type_into(
        &mut harness,
        Field::Hint("Search node types"),
        "AlwaysSucc",
        false,
    );
    // The search hides other types.
    assert!(harness.query_by_label("Sequence").is_none());

    let from = harness.get_by_label("AlwaysSuccess").rect().center();
    let to = item_rect(&harness, "fallback").center();
    drag(&mut harness, from, to);
    assert_eq!(
        children(&harness, "fallback"),
        ["check_blocked", "check_flag", "AlwaysSuccess"]
    );

    select(&mut harness, "reactive");
    // The palette entry (the canvas has a node with the same label now).
    let item = harness
        .query_all_by_label("AlwaysSuccess")
        .find(|n| n.accesskit_node().role() != Role::TreeItem)
        .expect("the palette entry")
        .rect()
        .center();
    harness.event(Event::PointerMoved(item));
    for _ in 0..2 {
        for pressed in [true, false] {
            harness.event(Event::PointerButton {
                pos: item,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
    }
    harness.run();
    assert_eq!(
        children(&harness, "reactive"),
        ["CheckFlag", "wait_three", "AlwaysSuccess"]
    );
}

#[test]
fn model_dialog_adds_a_model_to_the_file() {
    let mut harness = harness();
    harness.state_mut().show_panel(Tab::Palette);
    harness.run();
    harness.get_by_role_and_label(Role::Button, "New…").click();
    harness.run();
    type_into(&mut harness, Field::Label("ID"), "MoveArm", false);
    harness
        .get_by_role_and_label(Role::Button, "Add port")
        .click();
    harness.run();
    // The new port has no name yet, so OK is disabled.
    assert!(
        harness
            .get_by_role_and_label(Role::Button, "OK")
            .accesskit_node()
            .is_disabled()
    );
    type_into(&mut harness, Field::Hint("port name"), "speed", false);
    harness.get_by_role_and_label(Role::Button, "OK").click();
    harness.run();

    let text = xml(&harness);
    assert!(text.contains("<Action ID=\"MoveArm\""), "{text}");
    assert!(text.contains("<input_port name=\"speed\""), "{text}");
    // The palette lists the new type.
    harness.get_by_label("MoveArm");
}

fn primary_label(harness: &Harness<'static, FastTickApp>) -> Option<String> {
    let app = harness.state();
    let view = app.workspace().active().unwrap();
    let id = view.canvas.primary()?;
    Some(view.canvas.tree().get(id)?.label.clone())
}

#[test]
fn problems_list_shows_new_problems_and_reveals_the_node() {
    let mut harness = harness();
    harness.state_mut().show_panel(Tab::Problems);
    harness.run();
    // An empty Fallback is an error.
    for label in ["check_blocked", "check_flag"] {
        right_click(&mut harness, label);
        harness
            .get_by_role_and_label(Role::Button, "Delete")
            .click();
        harness.run();
    }
    let message = {
        let app = harness.state();
        let view = app.workspace().active().unwrap();
        let doc = app.workspace().doc(view.doc).unwrap();
        let editor = doc.editor.as_ref().unwrap();
        let tree = view.canvas.tree();
        let fallback = tree
            .nodes()
            .iter()
            .find(|n| n.label == "fallback")
            .map(|n| n.source)
            .unwrap();
        editor
            .problems()
            .iter()
            .find(|p| {
                p.node
                    .map(|n| fasttick_app::canvas::NodeSource::Xml(p.file, n))
                    == Some(fallback)
            })
            .expect("a problem of the Fallback")
            .message
            .clone()
    };
    let row = harness
        .query_all(egui_kittest::kittest::by().label(&message))
        .find(|n| n.accesskit_node().role() == Role::Label)
        .expect("the problem row")
        .rect()
        .center();
    // Select another node first, then click the problem.
    select(&mut harness, "init");
    harness.event(Event::PointerMoved(row));
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos: row,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run();
    assert_eq!(primary_label(&harness).as_deref(), Some("fallback"));
}

#[test]
fn xml_panel_follows_the_document() {
    let mut harness = harness();
    harness.state_mut().show_panel(Tab::Xml);
    harness.run();
    harness.get_by_label_contains("<SayMessage name=\"say_text\"");
    select(&mut harness, "say_text");
    type_into(&mut harness, Field::Label("Name"), "said", true);
    harness.get_by_label_contains("<SayMessage name=\"said\"");
}

#[test]
fn search_finds_nodes_and_reveals_them() {
    let mut harness = harness();
    command(&mut harness, Command::Find);
    harness.run();
    assert!(input(&harness, "Find").is_focused());
    input(&harness, "Find").type_text("count_t");
    harness.run();
    harness.get_by_label("1 matches in 1 nodes");
    {
        let app = harness.state();
        let tree = app.workspace().active().unwrap().canvas.tree();
        let marked: Vec<String> = tree
            .nodes()
            .iter()
            .filter(|n| n.deco.search_match)
            .map(|n| n.label.clone())
            .collect();
        assert_eq!(marked, ["count_twice"]);
    }
    harness
        .query_all(egui_kittest::kittest::by().label_contains("count_twice"))
        .find(|n| n.accesskit_node().role() == Role::Button)
        .expect("the result row")
        .click();
    harness.run();
    assert_eq!(primary_label(&harness).as_deref(), Some("count_twice"));

    // Wildcards match the whole text.
    harness.get_by_label("* ?").click();
    type_into(&mut harness, Field::Label("Find"), "count_*", true);
    // All trees of the file: count_once, count_twice and count_in_subtree.
    harness.get_by_label("3 matches in 3 nodes");
}

#[test]
fn tree_list_adds_renames_and_sets_the_main_tree() {
    let mut harness = harness();
    harness
        .get_by_role_and_label(Role::Button, "Add tree…")
        .click();
    harness.run();
    input(&harness, "Tree ID").type_text("Extra");
    harness.run();
    harness.get_by_role_and_label(Role::Button, "Add").click();
    harness.run();
    let trees = |h: &Harness<'static, FastTickApp>| {
        let app = h.state();
        let view = app.workspace().active().unwrap();
        let doc = app.workspace().doc(view.doc).unwrap();
        doc.editor
            .as_ref()
            .unwrap()
            .trees()
            .into_iter()
            .map(|(_, id)| id)
            .collect::<Vec<_>>()
    };
    assert!(trees(&harness).contains(&"Extra".to_owned()));
    // The view shows the new tree.
    assert_eq!(
        harness
            .state()
            .workspace()
            .active()
            .unwrap()
            .canvas
            .tree()
            .title,
        "Extra"
    );

    // Rename it with the context menu of the tree list.
    harness.get_by_label("Extra").click_secondary();
    harness.run();
    harness.get_by_label("Rename…").click();
    harness.run();
    type_into(&mut harness, Field::Label("Tree ID"), "Spare", true);
    harness
        .get_by_role_and_label(Role::Button, "Rename")
        .click();
    harness.run();
    assert!(trees(&harness).contains(&"Spare".to_owned()));
    assert!(!trees(&harness).contains(&"Extra".to_owned()));

    harness.get_by_label("Spare").click_secondary();
    harness.run();
    harness.get_by_label("Set as main tree").click();
    harness.run();
    assert!(xml(&harness).contains("main_tree_to_execute=\"Spare\""));
}

#[test]
fn project_with_several_files() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.xml");
    let b = dir.path().join("b.xml");
    let a_text = r#"<root BTCPP_format="4" main_tree_to_execute="Main">
  <BehaviorTree ID="Main">
    <Sequence name="seq">
      <SubTree ID="Other" name="other"/>
    </Sequence>
  </BehaviorTree>
</root>
"#;
    std::fs::write(&a, a_text).unwrap();
    std::fs::write(
        &b,
        r#"<root BTCPP_format="4">
  <BehaviorTree ID="Other">
    <AlwaysSuccess name="leaf"/>
  </BehaviorTree>
</root>
"#,
    )
    .unwrap();
    let project_path = dir.path().join("p.ftproj");
    let mut project = fasttick_core::project::Project::open_xml(&a).unwrap();
    project.add_file(&b).unwrap();
    project.save_as(&project_path).unwrap();

    let options = StartOptions {
        open: vec![project_path.clone()],
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
    // The tree list has a header for each file.
    harness.get_by_label("a.xml");
    harness.get_by_label("b.xml");
    let title = |h: &Harness<'static, FastTickApp>| {
        h.state()
            .workspace()
            .active()
            .unwrap()
            .canvas
            .tree()
            .title
            .clone()
    };
    assert_eq!(title(&harness), "Main");

    // Edit the tree of the second file.
    harness.get_by_role_and_label(Role::Button, "Other").click();
    harness.run();
    assert_eq!(title(&harness), "Other");
    select(&mut harness, "leaf");
    type_into(&mut harness, Field::Label("Name"), "leaf2", true);
    command(&mut harness, Command::Save);
    let b_text = std::fs::read_to_string(&b).unwrap();
    assert!(b_text.contains("name=\"leaf2\""), "{b_text}");
    assert_eq!(
        std::fs::read_to_string(&a).unwrap(),
        a_text,
        "a.xml did not change"
    );

    // A new file in the project.
    let c = dir.path().join("c.xml");
    let view = harness.state().workspace().active_view.unwrap();
    let ctx = harness.ctx.clone();
    harness.state_mut().project_file_picked(
        &ctx,
        view,
        fasttick_app::jobs::DialogPurpose::NewTreeFile,
        c.clone(),
    );
    harness.run();
    assert!(c.exists());
    harness.get_by_label("c.xml");
    // The view shows the tree of the new file; a node goes into that file.
    let shown = title(&harness);
    command(&mut harness, Command::Save);
    harness.state_mut().workspace_mut();
    let app = harness.state();
    let view = app.workspace().active().unwrap();
    let doc = app.workspace().doc(view.doc).unwrap();
    let editor = doc.editor.as_ref().unwrap();
    let file = editor.tree_file(view.root_instance.0).unwrap();
    assert_eq!(editor.file_name(file), "c.xml");
    assert_eq!(
        editor.tree_id(view.root_instance.0).as_deref(),
        Some(shown.as_str())
    );
}

#[test]
fn a_model_file_removes_the_unknown_type_problems() {
    let mut harness = harness();
    let unknown = |h: &Harness<'static, FastTickApp>| {
        let app = h.state();
        let view = app.workspace().active().unwrap();
        let doc = app.workspace().doc(view.doc).unwrap();
        doc.editor
            .as_ref()
            .unwrap()
            .problems()
            .iter()
            .filter(|p| p.message.contains("is not known"))
            .count()
    };
    assert!(unknown(&harness) > 0, "the test nodes have no models");
    let models = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/harness/custom_models.xml");
    let view = harness.state().workspace().active_view.unwrap();
    let ctx = harness.ctx.clone();
    harness.state_mut().project_file_picked(
        &ctx,
        view,
        fasttick_app::jobs::DialogPurpose::AddModelFile,
        models,
    );
    harness.run();
    assert_eq!(unknown(&harness), 0);
    // The palette lists the new types.
    harness.state_mut().show_panel(Tab::Palette);
    harness.run();
    type_into(
        &mut harness,
        Field::Hint("Search node types"),
        "WriteValues",
        false,
    );
    assert!(
        harness
            .query_all_by_label("WriteValues")
            .any(|n| n.accesskit_node().role() != Role::TreeItem)
    );
}

#[test]
fn a_condition_script_with_an_error_shows_the_bt_cpp_message() {
    let mut harness = harness();
    select(&mut harness, "say_default");
    harness
        .get_by_label_contains("Pre- and post-conditions")
        .click();
    harness.run();
    type_into(&mut harness, Field::Label("_skipIf"), "x == (1", false);
    // The message of BT.CPP shows below the field and in the problems list.
    assert_eq!(
        harness
            .query_all_by_label_contains("expected ')' (got '')")
            .count(),
        2
    );
    let script_errors = |h: &Harness<'static, FastTickApp>| {
        let app = h.state();
        let view = app.workspace().active().unwrap();
        let doc = app.workspace().doc(view.doc).unwrap();
        doc.editor
            .as_ref()
            .unwrap()
            .problems()
            .iter()
            .filter(|p| p.code == fasttick_core::validate::ProblemCode::ScriptSyntax)
            .count()
    };
    assert_eq!(script_errors(&harness), 1);
    // The card has an error marker.
    {
        let app = harness.state();
        let tree = app.workspace().active().unwrap().canvas.tree();
        let node = tree
            .nodes()
            .iter()
            .find(|n| n.label == "say_default")
            .unwrap();
        assert_eq!(
            node.deco.problem,
            Some(fasttick_app::canvas::ProblemMarker::Error)
        );
    }
    // Correct the script: the problem goes away.
    input(&harness, "_skipIf").type_text(")");
    harness.run();
    assert_eq!(script_errors(&harness), 0);
    assert!(harness.query_by_label_contains("expected ')'").is_none());
}

#[test]
fn opening_a_file_offers_the_files_with_the_missing_subtrees() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main.xml");
    std::fs::write(
        &main,
        r#"<root BTCPP_format="4" main_tree_to_execute="Main">
  <BehaviorTree ID="Main">
    <Sequence>
      <SubTree ID="Get"/>
    </Sequence>
  </BehaviorTree>
</root>
"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("library.xml"),
        r#"<root BTCPP_format="4">
  <BehaviorTree ID="Get">
    <SubTree ID="Reset"/>
  </BehaviorTree>
</root>
"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("automation.xml"),
        r#"<root BTCPP_format="4">
  <BehaviorTree ID="Reset">
    <AlwaysSuccess/>
  </BehaviorTree>
</root>
"#,
    )
    .unwrap();
    let options = StartOptions {
        open: vec![main],
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
    let missing = |h: &Harness<'static, FastTickApp>| {
        let app = h.state();
        let view = app.workspace().active().unwrap();
        let doc = app.workspace().doc(view.doc).unwrap();
        doc.editor
            .as_ref()
            .unwrap()
            .problems()
            .iter()
            .filter(|p| p.code == fasttick_core::validate::ProblemCode::MissingSubTree)
            .count()
    };
    assert_eq!(missing(&harness), 1);
    let notice = harness
        .state()
        .notices()
        .last()
        .expect("a notice")
        .text
        .clone();
    assert!(
        notice.contains("main.xml uses 1 tree")
            && notice.contains("2 XML files")
            && notice.contains("library.xml"),
        "{notice}"
    );
    harness
        .get_by_role_and_label(Role::Button, "Add the files")
        .click();
    harness.run();
    assert_eq!(missing(&harness), 0);
    // The tree list has a header for each new file.
    harness.get_by_label("library.xml");
    harness.get_by_label("automation.xml");
}

/// Shows a node at 100 % in the middle of the view.
fn focus(harness: &mut Harness<'static, FastTickApp>, label: &str) {
    let view = harness.state_mut().workspace_mut().active_mut().unwrap();
    let id = view
        .canvas
        .tree()
        .ids()
        .find(|id| view.canvas.tree().get(*id).unwrap().label == label)
        .expect("the node");
    view.canvas.focus_on(id, false);
    harness.run();
}

/// The dot of a node: the middle of the bottom edge of its card.
fn dot_of(harness: &Harness<'static, FastTickApp>, label: &str) -> Pos2 {
    let rect = item_rect(harness, label);
    pos2(rect.center().x, rect.max.y)
}

/// A primary click at a screen position.
fn click_at(harness: &mut Harness<'static, FastTickApp>, pos: Pos2) {
    harness.event(Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run();
}

#[test]
fn a_drag_from_the_dot_asks_for_a_type_and_adds_the_node_there() {
    let mut harness = harness();
    focus(&mut harness, "fallback");
    let first = item_rect(&harness, "check_blocked");
    let from = dot_of(&harness, "fallback");
    // A release left of the first child puts the new node before it.
    drag(
        &mut harness,
        from,
        pos2(first.min.x - 20.0, first.max.y + 40.0),
    );
    let popup = harness.state().add_popup().expect("the popup opens");
    assert_eq!(popup.title, "Add a node before check_blocked");
    type_into(
        &mut harness,
        Field::Hint("Search node types"),
        "alwfail",
        false,
    );
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert!(harness.state().add_popup().is_none());
    assert_eq!(
        children(&harness, "fallback"),
        ["AlwaysFailure", "check_blocked", "check_flag"]
    );
    // Undo removes the node.
    command(&mut harness, Command::Undo);
    assert_eq!(
        children(&harness, "fallback"),
        ["check_blocked", "check_flag"]
    );
}

#[test]
fn a_click_on_the_dot_asks_for_a_last_child_and_escape_closes() {
    let mut harness = harness();
    focus(&mut harness, "fallback");
    let from = dot_of(&harness, "fallback");
    click_at(&mut harness, from);
    let popup = harness.state().add_popup().expect("the popup opens");
    assert_eq!(popup.title, "Add a child of fallback");
    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(harness.state().add_popup().is_none());
    assert_eq!(
        children(&harness, "fallback"),
        ["check_blocked", "check_flag"]
    );
    // A click on a type in the list adds it as the last child.
    click_at(&mut harness, from);
    type_into(
        &mut harness,
        Field::Hint("Search node types"),
        "sleep",
        false,
    );
    harness.get_by_role_and_label(Role::Button, "Sleep").click();
    harness.run();
    assert_eq!(
        children(&harness, "fallback"),
        ["check_blocked", "check_flag", "Sleep"]
    );
    // A leaf has no dot: a click there selects the leaf and opens nothing.
    let leaf = dot_of(&harness, "check_flag");
    click_at(&mut harness, leaf);
    assert!(harness.state().add_popup().is_none());
}

#[test]
fn a_drag_from_the_dot_to_an_unconnected_node_connects_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("small.xml");
    std::fs::write(
        &path,
        r#"<root BTCPP_format="4" main_tree_to_execute="Main">
  <BehaviorTree ID="Main">
    <Sequence name="seq">
      <AlwaysSuccess name="first"/>
    </Sequence>
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
    // Make an unconnected node.
    let view = harness.state().workspace().active_view.unwrap();
    let ctx = harness.ctx.clone();
    harness.state_mut().run_edit(
        &ctx,
        view,
        fasttick_app::edit_ops::EditOp::AddUnconnected {
            type_id: "Sleep".into(),
        },
    );
    harness.run();
    command(&mut harness, Command::FitToView);
    let target = item_rect(&harness, "Sleep").center();
    let from = dot_of(&harness, "seq");
    drag(&mut harness, from, target);
    assert!(harness.state().add_popup().is_none());
    assert_eq!(children(&harness, "seq"), ["first", "Sleep"]);
}

#[test]
fn shift_a_opens_the_search_for_the_selected_node() {
    let mut harness = harness();
    select(&mut harness, "fallback");
    harness.key_press_modifiers(Modifiers::SHIFT, egui::Key::A);
    harness.run();
    assert_eq!(
        harness
            .state()
            .add_popup()
            .map(|p| p.title.clone())
            .as_deref(),
        Some("Add a child of fallback")
    );
    type_into(
        &mut harness,
        Field::Hint("Search node types"),
        "inverter",
        false,
    );
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(
        children(&harness, "fallback"),
        ["check_blocked", "check_flag", "Inverter"]
    );
    // A leaf gets a sibling after it.
    select(&mut harness, "check_blocked");
    harness.key_press_modifiers(Modifiers::SHIFT, egui::Key::A);
    harness.run();
    assert_eq!(
        harness
            .state()
            .add_popup()
            .map(|p| p.title.clone())
            .as_deref(),
        Some("Add a node after check_blocked")
    );
    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(harness.state().add_popup().is_none());
}
