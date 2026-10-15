//! End-to-end UI test of live mode with the real BT.CPP 4.9.0 executor of
//! the test harness (`fasttick-testkit`). The test is skipped when BT.CPP
//! is not installed (see `require_harness!`).

use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use fasttick_app::app::{FastTickApp, Mode, StartOptions};
use fasttick_app::canvas::BreakpointMarker;
use fasttick_app::theme::ThemeChoice;
use fasttick_core::status::NodeStatus;
use fasttick_testkit::{Executor, ExecutorOptions, free_port_pair, require_harness};

const TIMEOUT: Duration = Duration::from_secs(20);

fn wait(
    harness: &mut Harness<'static, FastTickApp>,
    what: &str,
    mut check: impl FnMut(&Harness<'static, FastTickApp>) -> bool,
) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        harness.step();
        if check(harness) {
            harness.step();
            return;
        }
        assert!(Instant::now() < deadline, "timeout: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The node of the active view with this label.
fn node<'a>(
    harness: &'a Harness<'static, FastTickApp>,
    label: &str,
) -> Option<&'a fasttick_app::canvas::CanvasNode> {
    let view = harness.state().workspace().active()?;
    view.canvas.tree().nodes().iter().find(|n| n.label == label)
}

#[test]
fn live_mode_with_the_real_executor() {
    let kit = require_harness!();
    let mut options = ExecutorOptions::new(kit.tree("long_running.xml"));
    options.port = free_port_pair();
    options.tick_ms = 20;
    let executor = Executor::spawn(options).expect("the executor starts");
    let port = executor.port();

    let start = StartOptions {
        open: Vec::new(),
        theme: Some(ThemeChoice::Dark),
        mode: Some(Mode::Live),
        connect: Some(("127.0.0.1".to_owned(), port)),
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1500.0, 950.0))
        .with_theme(egui::Theme::Dark)
        .with_max_steps(400)
        .build_eframe(move |cc| FastTickApp::new(cc, start));
    wait(&mut harness, "the live tree", |h| {
        h.state().live().is_connected() && node(h, "say_cycle").is_some()
    });

    // The root runs for ever: RUNNING.
    wait(&mut harness, "a running root", |h| {
        node(h, "long_sequence").is_some_and(|n| n.deco.status == NodeStatus::Running)
    });

    // Select a node and press F9: a breakpoint before it.
    harness
        .get_by_role_and_label(Role::TreeItem, "say_cycle")
        .click();
    harness.run_ok();
    harness.key_press(egui::Key::F9);
    harness.run_ok();
    let uid = node(&harness, "say_cycle").unwrap().uid.unwrap();
    assert!(harness.state().live().hook(uid).is_some());

    // The executor stops there: the banner shows.
    wait(&mut harness, "the stop at the breakpoint", |h| {
        h.state().live().is_hit(uid) && h.query_by_label("Stopped before").is_some()
    });
    assert_eq!(
        node(&harness, "say_cycle").unwrap().deco.breakpoint,
        Some(BreakpointMarker::Hit)
    );

    // F9 again removes the breakpoint, and the node continues.
    harness.key_press(egui::Key::F9);
    harness.run_ok();
    wait(&mut harness, "the node continues", |h| {
        h.state().live().hits().is_empty()
            && h.state().live().hooks().is_empty()
            && node(h, "say_cycle").is_some_and(|n| {
                n.deco.status == NodeStatus::Success
                    || n.deco.previous_status == Some(NodeStatus::Success)
            })
    });
    assert!(harness.query_by_label("Stopped before").is_none());

    // A short recording of the running tree.
    harness.get_by_label("Start recording").click();
    harness.run_ok();
    wait(&mut harness, "recorded transitions", |h| {
        h.state().live().recording().len() >= 20
    });
    harness.get_by_label("Stop recording").click();
    harness.run_ok();
    wait(&mut harness, "the stopped recording", |h| {
        !h.state().live().recording().is_active()
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("live.btlog");
    let ctx = harness.ctx.clone();
    assert!(harness.state_mut().save_recording_to(&ctx, path.clone()));
    wait(&mut harness, "the saved file", |h| {
        h.query_by_label("Open in replay").is_some()
    });
    let log = fasttick_log::btlog::BtLog::open(&path).unwrap();
    assert!(log.transitions.len() >= 20);
    assert!(log.unknown_uids().is_empty());

    // Disconnect.
    harness
        .get_by_role_and_label(Role::Button, "Disconnect")
        .click();
    harness.run_ok();
    wait(&mut harness, "the disconnect", |h| !h.state().live().is_active());
    drop(executor);
}
