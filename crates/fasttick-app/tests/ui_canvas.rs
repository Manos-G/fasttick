//! Headless tests of the canvas widget alone, with `egui_kittest`.

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, Vec2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use fasttick_app::canvas::{
    BuiltinCategories, CanvasEvent, CanvasState, CanvasTree, Expansion, RuntimeBuildOptions,
    TreeCanvas, Viewport,
};
use fasttick_app::theme::CanvasPalette;
use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;

const XML: &str = r#"<root BTCPP_format="4">
  <BehaviorTree ID="Main" _fullpath="">
    <Sequence name="root" _uid="1">
      <CheckBattery name="battery" _uid="2" level="{level}"/>
      <Fallback name="choose" _uid="3">
        <MoveBase name="move" _uid="4" goal="{goal}"/>
        <SubTree ID="Recover" _fullpath="recover" _uid="5"/>
      </Fallback>
      <SaySomething name="say" _uid="6" message="done"/>
    </Sequence>
  </BehaviorTree>
  <BehaviorTree ID="Recover" _fullpath="recover">
    <Sequence name="recover_steps" _uid="7">
      <Sleep name="wait" _uid="8" msec="100"/>
      <AlwaysSuccess name="ok" _uid="9"/>
    </Sequence>
  </BehaviorTree>
</root>"#;

/// The state of the test UI: the canvas and the events it sent.
struct TestState {
    canvas: CanvasState,
    events: Vec<CanvasEvent>,
    runtime: RuntimeTree,
    expansion: Expansion,
}

fn build(runtime: &RuntimeTree, expansion: &Expansion) -> CanvasTree {
    CanvasTree::from_runtime(
        runtime,
        RuntimeBuildOptions::for_tree(runtime),
        expansion,
        &BuiltinCategories,
    )
}

fn harness() -> Harness<'static, TestState> {
    let runtime = RuntimeTree::parse(XML).unwrap();
    let expansion =
        CanvasTree::default_expansion(&runtime, RuntimeBuildOptions::for_tree(&runtime));
    let canvas = CanvasState::new(build(&runtime, &expansion));
    let state = TestState {
        canvas,
        events: Vec::new(),
        runtime,
        expansion,
    };
    let palette = CanvasPalette::light();
    let mut harness = Harness::builder()
        .with_size(vec2(1000.0, 700.0))
        .with_max_steps(64)
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(
            move |ui, state: &mut TestState| {
                let output = TreeCanvas::new(&mut state.canvas, &palette).show(ui);
                for event in output.events {
                    // Rebuild on expand or collapse, as the app does.
                    if let CanvasEvent::ToggleSubtree(source) = event {
                        state.expansion.toggle(source);
                        let tree = build(&state.runtime, &state.expansion);
                        state.canvas.replace_tree(tree, Some(source));
                    }
                    state.events.push(event);
                }
            },
            state,
        );
    fasttick_app::fonts::install(&harness.ctx);
    harness.run();
    harness
}

fn item_center(harness: &Harness<'static, TestState>, label: &'static str) -> Pos2 {
    harness
        .get_by_role_and_label(Role::TreeItem, label)
        .rect()
        .center()
}

fn click_at(harness: &Harness<'static, TestState>, pos: Pos2, modifiers: Modifiers) {
    harness.event(Event::PointerMoved(pos));
    for pressed in [true, false] {
        harness.event_modifiers(
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            },
            modifiers,
        );
    }
}

fn selected_labels(harness: &Harness<'static, TestState>) -> Vec<String> {
    let tree = harness.state().canvas.tree();
    tree.selected()
        .map(|id| tree.get(id).unwrap().label.clone())
        .collect()
}

#[test]
fn shows_all_nodes_with_categories() {
    let harness = harness();
    // A small tree fits the view; all nodes are visible.
    for label in [
        "root",
        "battery",
        "choose",
        "move",
        "recover",
        "recover_steps",
        "wait",
        "ok",
        "say",
    ] {
        harness.get_by_role_and_label(Role::TreeItem, label);
    }
    assert_eq!(harness.query_all_by_role(Role::TreeItem).count(), 9);
    let canvas = harness.get_by_role(Role::Tree);
    assert_eq!(
        canvas.accesskit_node().label().as_deref(),
        Some("Tree canvas: Main")
    );
    let choose = harness.get_by_role_and_label(Role::TreeItem, "choose");
    assert_eq!(
        choose.accesskit_node().description().as_deref(),
        Some("Fallback (Control)")
    );
    // The node items are inside the canvas.
    let rect = canvas.rect();
    for node in harness.query_all_by_role(Role::TreeItem) {
        assert!(rect.contains_rect(node.rect()));
    }
}

#[test]
fn click_shift_click_ctrl_click() {
    let mut harness = harness();
    click_at(&harness, item_center(&harness, "battery"), Modifiers::NONE);
    harness.run();
    assert_eq!(selected_labels(&harness), ["battery"]);
    click_at(&harness, item_center(&harness, "say"), Modifiers::SHIFT);
    harness.run();
    assert_eq!(selected_labels(&harness), ["battery", "say"]);
    click_at(
        &harness,
        item_center(&harness, "battery"),
        Modifiers::COMMAND,
    );
    harness.run();
    assert_eq!(selected_labels(&harness), ["say"]);
    // A click on the background clears the selection.
    let canvas = harness.get_by_role(Role::Tree).rect();
    click_at(
        &harness,
        canvas.left_bottom() + vec2(5.0, -5.0),
        Modifiers::NONE,
    );
    harness.run();
    assert!(selected_labels(&harness).is_empty());
    assert!(
        harness
            .state()
            .events
            .iter()
            .any(|e| *e == CanvasEvent::SelectionChanged)
    );
    // The accessibility tree reports the selection.
    click_at(&harness, item_center(&harness, "move"), Modifiers::NONE);
    harness.run();
    let node = harness.get_by_role_and_label(Role::TreeItem, "move");
    assert_eq!(node.accesskit_node().is_selected(), Some(true));
}

#[test]
fn rubber_band_selects_nodes() {
    let mut harness = harness();
    let wait = harness.get_by_role_and_label(Role::TreeItem, "wait").rect();
    let ok = harness.get_by_role_and_label(Role::TreeItem, "ok").rect();
    let start = wait.left_bottom() + vec2(-6.0, 8.0);
    let end = ok.right_top() + vec2(6.0, 4.0);
    harness.event(Event::PointerMoved(start));
    harness.event(Event::PointerButton {
        pos: start,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.step();
    for t in [0.25, 0.5, 0.75, 1.0] {
        harness.event(Event::PointerMoved(start + (end - start) * t));
        harness.step();
    }
    harness.event(Event::PointerButton {
        pos: end,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.run();
    assert_eq!(selected_labels(&harness), ["wait", "ok"]);
}

#[test]
fn zoom_with_ctrl_wheel_and_pan_with_middle_drag() {
    let mut harness = harness();
    let before = harness.state().canvas.viewport();
    let anchor = item_center(&harness, "choose");
    harness.event(Event::PointerMoved(anchor));
    harness.event(Event::Zoom(1.5));
    harness.run();
    let after = harness.state().canvas.viewport();
    assert!(
        (after.zoom - before.zoom * 1.5).abs() < 1e-3,
        "{before:?} {after:?}"
    );
    // The node under the pointer stays in place.
    let moved = item_center(&harness, "choose");
    assert!((moved - anchor).length() < 1.0, "{anchor:?} -> {moved:?}");

    // Middle drag pans.
    let start = harness.get_by_role(Role::Tree).rect().left_top() + vec2(20.0, 20.0);
    harness.event(Event::PointerMoved(start));
    harness.event(Event::PointerButton {
        pos: start,
        button: PointerButton::Middle,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.step();
    for i in 1..=5 {
        harness.event(Event::PointerMoved(start + vec2(20.0, 10.0) * i as f32));
        harness.step();
    }
    harness.event(Event::PointerButton {
        pos: start + vec2(100.0, 50.0),
        button: PointerButton::Middle,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.run();
    let panned = harness.state().canvas.viewport();
    let delta = panned.pan - after.pan;
    assert!(delta.x > 50.0 && delta.y > 20.0, "{delta:?}");
    assert_eq!(panned.zoom, after.zoom);
}

#[test]
fn keyboard_navigation() {
    let mut harness = harness();
    click_at(&harness, item_center(&harness, "choose"), Modifiers::NONE);
    harness.run();
    let primary = |h: &Harness<'static, TestState>| {
        let c = &h.state().canvas;
        c.tree().get(c.primary().unwrap()).unwrap().label.clone()
    };
    harness.key_press(egui::Key::ArrowDown);
    harness.run();
    assert_eq!(primary(&harness), "move");
    harness.key_press(egui::Key::ArrowRight);
    harness.run();
    assert_eq!(primary(&harness), "recover");
    harness.key_press(egui::Key::ArrowRight);
    harness.run();
    assert_eq!(primary(&harness), "recover", "no sibling after the last");
    harness.key_press(egui::Key::ArrowUp);
    harness.key_press(egui::Key::ArrowUp);
    harness.run();
    assert_eq!(primary(&harness), "root");
    harness.key_press(egui::Key::ArrowLeft);
    harness.run();
    assert_eq!(primary(&harness), "root", "the root has no siblings");
    assert_eq!(selected_labels(&harness), ["root"]);
}

#[test]
fn enter_and_double_click_toggle_the_subtree() {
    let mut harness = harness();
    assert_eq!(harness.state().canvas.tree().len(), 9);
    let center = item_center(&harness, "recover");
    click_at(&harness, center, Modifiers::NONE);
    click_at(&harness, center, Modifiers::NONE);
    harness.run();
    assert_eq!(harness.state().canvas.tree().len(), 6);
    assert!(harness.query_by_label("wait").is_none());
    let collapsed = harness.get_by_role_and_label(Role::TreeItem, "recover");
    assert_eq!(collapsed.accesskit_node().data().is_expanded(), Some(false));
    // The node did not move.
    assert!((item_center(&harness, "recover") - center).length() < 1.0);
    // Enter expands it again.
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state().canvas.tree().len(), 9);
    harness.get_by_role_and_label(Role::TreeItem, "wait");
}

#[test]
fn hover_tooltip_shows_details() {
    let mut harness = harness();
    let center = item_center(&harness, "move");
    harness.hover_at(center);
    harness.run();
    // The tooltip appears after a short delay.
    for _ in 0..30 {
        harness.step();
    }
    harness.get_by_label("MoveBase  ·  Action");
    harness.get_by_label("{goal}");
    harness.get_by_label("IDLE");
}

#[test]
fn status_and_focus_on() {
    let mut harness = harness();
    {
        let tree = harness.state_mut().canvas.tree_mut();
        tree.set_status(Uid(4), NodeStatus::Running, None);
        tree.set_status(Uid(2), NodeStatus::Idle, Some(NodeStatus::Success));
    }
    harness.run();
    let node = harness.get_by_role_and_label(Role::TreeItem, "move");
    assert_eq!(
        node.accesskit_node().description().as_deref(),
        Some("MoveBase (Action), RUNNING")
    );

    // focus_on centres the node and selects it, with an animation.
    let state = harness.state_mut();
    state
        .canvas
        .set_viewport(Viewport::new(Vec2::new(4000.0, 0.0), 0.5));
    let id = state.canvas.tree().find_uid(Uid(8)).unwrap();
    state.canvas.focus_on(id, true);
    harness.run();
    let canvas = harness.get_by_role(Role::Tree).rect();
    let wait = item_center(&harness, "wait");
    assert!(
        (wait - canvas.center()).length() < 1.0,
        "{wait:?} {canvas:?}"
    );
    assert_eq!(selected_labels(&harness), ["wait"]);
    assert_eq!(harness.state().canvas.viewport().zoom, 1.0);
}
