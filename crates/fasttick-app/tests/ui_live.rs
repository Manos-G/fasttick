//! Headless UI tests of live mode against a fake executor
//! (`tests/support/fake_publisher.rs`): connect, status colours,
//! breakpoints with the banner, forced results, the blackboard view, the
//! recording, breakpoint files and the disconnect cleanup.

mod support;

use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use fasttick_app::app::{FastTickApp, Mode, StartOptions};
use fasttick_app::canvas::{BreakpointMarker, Decorations, Substitution};
use fasttick_app::dock::Tab;
use fasttick_app::shortcuts::Command;
use fasttick_app::theme::ThemeChoice;
use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_log::btlog::BtLog;
use fasttick_protocol::breakpoints::BreakpointSet;
use fasttick_protocol::messages::HookMode;
use fasttick_protocol::wire::RequestType;
use support::fake_publisher::FakePublisher;

const TIMEOUT: Duration = Duration::from_secs(10);

fn harness(port: u16) -> Harness<'static, FastTickApp> {
    let options = StartOptions {
        open: Vec::new(),
        theme: Some(ThemeChoice::Light),
        mode: Some(Mode::Live),
        connect: Some(("127.0.0.1".to_owned(), port)),
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1500.0, 950.0))
        .with_theme(egui::Theme::Light)
        .with_max_steps(400)
        .with_step_dt(1.0 / 30.0)
        .build_eframe(move |cc| FastTickApp::new(cc, options));
    harness.step();
    harness
}

/// Runs frames until `check` is true.
fn wait(
    harness: &mut Harness<'static, FastTickApp>,
    what: &str,
    mut check: impl FnMut(&Harness<'static, FastTickApp>) -> bool,
) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        harness.step();
        if check(harness) {
            // Let the UI show the change.
            harness.step();
            return;
        }
        assert!(Instant::now() < deadline, "timeout: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Waits until the fake executor state passes `check`, while the UI runs.
fn wait_fake(
    harness: &mut Harness<'static, FastTickApp>,
    fake: &FakePublisher,
    what: &str,
    mut check: impl FnMut(&support::fake_publisher::FakeState) -> bool,
) {
    wait(harness, what, |_| check(&fake.state()));
}

/// The decorations of the node with a UID in the active live view.
fn deco(harness: &Harness<'static, FastTickApp>, uid: u16) -> Decorations {
    let view = harness.state().workspace().active().expect("a live view");
    let tree = view.canvas.tree();
    tree.get(tree.find_uid(Uid(uid)).expect("node on the canvas"))
        .unwrap()
        .deco
}

/// Opens the context menu of a canvas node (right-click).
fn right_click(harness: &mut Harness<'static, FastTickApp>, label: &str) {
    let center = harness
        .get_by_role_and_label(Role::TreeItem, label)
        .rect()
        .center();
    harness.event(Event::PointerMoved(center));
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos: center,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run_ok();
}

fn click_label(harness: &mut Harness<'static, FastTickApp>, label: &str) {
    harness.get_by_label(label).click();
    harness.run_ok();
}

fn connected(harness: &mut Harness<'static, FastTickApp>) {
    wait(harness, "the live tree", |h| {
        h.state().live().is_connected()
            && h.state().live_doc().is_some()
            && h.state().workspace().active().is_some()
    });
    harness.run_ok();
}

#[test]
fn connect_breakpoint_banner_and_continue() {
    let fake = FakePublisher::start();
    fake.set_status(1, 1); // main: RUNNING
    fake.set_status(2, 2); // first: SUCCESS
    fake.set_status(5, 13); // fb: IDLE, was FAILURE
    let mut harness = harness(fake.port);
    connected(&mut harness);

    // The status bar and the live bar show the connection.
    assert_eq!(
        harness
            .query_all_by_label(&format!("Connected to 127.0.0.1:{}", fake.port))
            .count(),
        2,
        "the live bar and the status bar"
    );
    harness.get_by_role_and_label(Role::Button, "Disconnect");

    // Status colours: current and faded previous.
    wait(&mut harness, "statuses", |h| {
        deco(h, 1).status == NodeStatus::Running
    });
    assert_eq!(deco(&harness, 2).status, NodeStatus::Success);
    assert_eq!(deco(&harness, 5).status, NodeStatus::Idle);
    assert_eq!(deco(&harness, 5).previous_status, Some(NodeStatus::Failure));
    // The canvas shows the nodes as accessible tree items.
    harness.get_by_role_and_label(Role::TreeItem, "first");

    // "After" breakpoints are not available (plan decision D4).
    right_click(&mut harness, "first");
    assert!(
        harness
            .get_by_label("Add breakpoint after the node")
            .accesskit_node()
            .is_disabled()
    );
    click_label(&mut harness, "Add breakpoint before the node");
    wait_fake(&mut harness, &fake, "the hook in the executor", |s| {
        s.hooks
            .get(&2)
            .is_some_and(|h| h.hook.mode == HookMode::Breakpoint && h.hook.enabled)
    });
    wait(&mut harness, "the breakpoint marker", |h| {
        deco(h, 2).breakpoint == Some(BreakpointMarker::Enabled)
    });

    // The executor stops before the node: banner, marker, focus.
    assert_eq!(fake.hit(2), Some(HookMode::Breakpoint));
    wait(&mut harness, "the banner", |h| {
        h.query_by_label("Stopped before").is_some()
    });
    assert_eq!(deco(&harness, 2).breakpoint, Some(BreakpointMarker::Hit));
    assert_eq!(harness.state().live().hits(), &[Uid(2)]);
    {
        let view = harness.state().workspace().active().unwrap();
        let primary = view.canvas.primary().expect("the node is selected");
        assert_eq!(view.canvas.tree().get(primary).unwrap().uid, Some(Uid(2)));
        assert!(view.canvas.viewport().zoom >= 1.0 - 1e-3);
    }
    harness.get_by_label("Force SUCCESS");
    harness.get_by_label("Force FAILURE");

    // Continue releases the node.
    harness
        .get_by_role_and_label(Role::Button, "Continue")
        .click();
    harness.run_ok();
    wait_fake(&mut harness, &fake, "the unlock", |s| {
        s.hooks.get(&2).is_some_and(|h| !h.blocked)
    });
    assert!(fake.count(RequestType::BreakpointUnlock) >= 1);
    wait(&mut harness, "no banner", |h| {
        h.query_by_label("Stopped before").is_none()
    });
    assert_eq!(
        deco(&harness, 2).breakpoint,
        Some(BreakpointMarker::Enabled)
    );

    // A second stop, released with a forced result (F8 is Continue; here
    // the banner button "Force FAILURE").
    fake.hit(2);
    wait(&mut harness, "the second stop", |h| {
        !h.state().live().hits().is_empty()
    });
    harness.get_by_label("Force FAILURE").click();
    harness.run_ok();
    wait_fake(&mut harness, &fake, "the forced unlock", |s| {
        s.hooks
            .get(&2)
            .is_some_and(|h| !h.blocked && h.hook.desired_status == NodeStatus::Failure)
    });

    // Fault injection on another node, then clear it.
    right_click(&mut harness, "fail");
    click_label(&mut harness, "Force SUCCESS");
    wait_fake(&mut harness, &fake, "the replace hook", |s| {
        s.hooks.get(&6).is_some_and(|h| {
            h.hook.mode == HookMode::Replace && h.hook.desired_status == NodeStatus::Success
        })
    });
    wait(&mut harness, "the forced marker", |h| {
        deco(h, 6).substitution == Some(Substitution::ForcedSuccess)
    });
    // A replace hook also sends a "breakpoint reached": no banner.
    assert_eq!(fake.hit(6), Some(HookMode::Replace));
    for _ in 0..20 {
        harness.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(harness.query_by_label("Stopped before").is_none());

    // The breakpoints panel lists both kinds.
    harness.state_mut().show_panel(Tab::Breakpoints);
    harness.run_ok();
    harness.get_by_label("Breakpoints (1)");
    harness.get_by_label("Forced results (1)");
    harness.get_by_role_and_label(Role::Button, "Clear").click();
    harness.run_ok();
    wait_fake(&mut harness, &fake, "the removed replace hook", |s| {
        !s.hooks.contains_key(&6)
    });
    wait(&mut harness, "no forced marker", |h| {
        deco(h, 6).substitution.is_none()
    });

    // Disable the breakpoint in the panel.
    harness.get_by_label("Enabled").click();
    harness.run_ok();
    wait_fake(&mut harness, &fake, "the disabled hook", |s| {
        s.hooks.get(&2).is_some_and(|h| !h.hook.enabled)
    });
    wait(&mut harness, "the disabled marker", |h| {
        deco(h, 2).breakpoint == Some(BreakpointMarker::Disabled)
    });

    // Disconnect: the session removes all hooks of the executor.
    harness
        .get_by_role_and_label(Role::Button, "Disconnect")
        .click();
    harness.run_ok();
    wait_fake(&mut harness, &fake, "the hook cleanup", |s| {
        s.hooks.is_empty()
    });
    wait(&mut harness, "the disconnected state", |h| {
        !h.state().live().is_active() && h.state().live().hooks().is_empty()
    });
    assert!(deco(&harness, 2).breakpoint.is_none());
    fake.assert_not_crashed();
}

#[test]
fn blackboard_recording_and_breakpoint_files() {
    let fake = FakePublisher::start();
    let mut harness = harness(fake.port);
    connected(&mut harness);

    // The blackboard of the root tree is watched by default.
    harness.state_mut().show_panel(Tab::Blackboard);
    harness.run_ok();
    wait(&mut harness, "blackboard values", |h| {
        h.state().live().blackboard().value_of("MainTree").is_some()
    });
    harness.get_by_label("\"fast\"");
    harness.get_by_label("0.5");
    // Watch the SubTree blackboard too: arrays show indexed entries.
    harness.get_by_label("sub  (Sub)").click();
    harness.run_ok();
    wait(&mut harness, "the SubTree blackboard", |h| {
        h.state().live().blackboard().value_of("sub").is_some()
    });
    harness.get_by_label("list").click();
    harness.run_ok();
    harness.get_by_label("[1]");
    harness.get_by_label("2.25");
    // The filter hides entries that do not match.
    harness.get_by_role(Role::TextInput).focus();
    harness.run_ok();
    harness.get_by_role(Role::TextInput).type_text("flag");
    harness.run_ok();
    harness.get_by_label("flag");
    assert!(harness.query_by_label("\"fast\"").is_none());

    // Recording.
    harness.get_by_label("Start recording").click();
    harness.run_ok();
    wait_fake(&mut harness, &fake, "the recording start", |s| s.recording);
    wait(&mut harness, "the recording in the UI", |h| {
        h.state().live().recording().is_active()
    });
    fake.push_transitions(7);
    wait(&mut harness, "the recorded transitions", |h| {
        h.state().live().recording().len() == 7
    });
    // The live bar and the transitions panel show the count.
    assert_eq!(
        harness
            .query_all_by_label_contains("Recording: 7 transitions")
            .count(),
        2
    );
    harness.get_by_label("Stop recording").click();
    harness.run_ok();
    wait(&mut harness, "the recording stop", |h| {
        !h.state().live().recording().is_active()
    });
    harness.get_by_label("Recorded 7 transitions");
    harness.get_by_label_contains("Recorded 7 transitions. Save the recording");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("live.btlog");
    let ctx = harness.ctx.clone();
    assert!(harness.state_mut().save_recording_to(&ctx, path.clone()));
    wait(&mut harness, "the saved recording", |h| {
        h.query_by_label("Open in replay").is_some()
    });
    let log = BtLog::open(&path).unwrap();
    assert_eq!(log.transitions.len(), 7);
    assert_eq!(log.tree.len(), 6);
    // "Open in replay" opens the file in replay mode.
    harness.get_by_label("Open in replay").click();
    harness.run_ok();
    wait(&mut harness, "the replay document", |h| {
        h.state().mode() == Mode::Replay && h.state().active_replay().is_some()
    });
    assert_eq!(harness.state().active_replay().unwrap().index().len(), 7);
    harness.state_mut().set_mode(Mode::Live);
    harness.run_ok();

    // Breakpoint files: save two hooks, remove them, load the file.
    harness.state_mut().live_mut().add_breakpoint(Uid(2));
    harness
        .state_mut()
        .live_mut()
        .force(Uid(6), NodeStatus::Failure);
    wait_fake(&mut harness, &fake, "two hooks", |s| s.hooks.len() == 2);
    let file = dir.path().join("set.ftbp.json");
    let ctx = harness.ctx.clone();
    harness.state_mut().save_breakpoints_to(&ctx, file.clone());
    wait(&mut harness, "the saved breakpoints", |_| file.exists());
    let set = BreakpointSet::load(&file).unwrap();
    let paths: Vec<&str> = set
        .breakpoints
        .iter()
        .map(|b| b.node_path.as_str())
        .collect();
    assert_eq!(paths, ["first", "sub/fail"]);
    harness.state_mut().live_mut().remove_all_hooks();
    wait_fake(&mut harness, &fake, "no hooks", |s| s.hooks.is_empty());
    harness
        .state_mut()
        .prepare_breakpoints(&ctx, file.clone(), set);
    harness.run_ok();
    harness.get_by_label("Load breakpoints");
    assert_eq!(harness.query_all_by_label("Found by path").count(), 2);
    harness.get_by_label("Insert 2 hooks").click();
    harness.run_ok();
    wait_fake(&mut harness, &fake, "the loaded hooks", |s| {
        s.hooks
            .get(&2)
            .is_some_and(|h| h.hook.mode == HookMode::Breakpoint)
            && s.hooks
                .get(&6)
                .is_some_and(|h| h.hook.mode == HookMode::Replace)
    });
    assert!(harness.state().pending_breakpoints().is_none());

    // The commands of the Live menu.
    assert!(harness.state().is_enabled(Command::SaveBreakpoints));
    assert!(harness.state().is_enabled(Command::ToggleRecording));
    assert!(!harness.state().is_enabled(Command::Continue));
    fake.assert_not_crashed();
}
