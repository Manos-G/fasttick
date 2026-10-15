//! Live and replay with a tree of 5,000 nodes.
//!
//! The test measures the work of one frame outside the canvas painting
//! (decoding a status reply of the executor and putting the statuses on
//! the canvas tree; finding the replay state of a time and putting it on
//! the canvas tree), the build of the replay index of a long log, and the
//! canvas frame while the statuses change on each frame.
//!
//! Run with `--release -- --nocapture` to see the times. The limits are
//! generous, because debug builds and shared CI machines are slow.

use std::time::{Duration, Instant};

use egui::{Pos2, Rect, Vec2, vec2};
use fasttick_app::canvas::categories::BuiltinCategories;
use fasttick_app::canvas::model::{Expansion, RuntimeBuildOptions};
use fasttick_app::canvas::{CanvasState, CanvasTree, TreeCanvas, Viewport};
use fasttick_app::documents::apply_replay_state;
use fasttick_app::theme::CanvasPalette;
use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_core::status::{NodeStatus, RawStatus};
use fasttick_log::{ReplayIndex, Transition};
use fasttick_protocol::messages::StatusSnapshot;

const NODES: usize = 5000;
const SIZE: Vec2 = vec2(1600.0, 1000.0);

/// A tree XML of the executor (with `_uid` attributes): controls with
/// `fanout` children, leaves at the bottom, `nodes` nodes in all.
fn runtime_xml(nodes: usize, fanout: usize) -> String {
    // Breadth-first: node i has the children fanout*i+1 ..= fanout*i+fanout.
    fn write(out: &mut String, index: usize, nodes: usize, fanout: usize) {
        let uid = index + 1;
        let first = fanout * index + 1;
        if first >= nodes {
            out.push_str(&format!(
                "<AlwaysSuccess name=\"leaf_{uid}\" _uid=\"{uid}\"/>"
            ));
            return;
        }
        let tag = if index.is_multiple_of(2) {
            "Sequence"
        } else {
            "Fallback"
        };
        out.push_str(&format!("<{tag} name=\"node_{uid}\" _uid=\"{uid}\">"));
        for child in first..(first + fanout).min(nodes) {
            write(out, child, nodes, fanout);
        }
        out.push_str(&format!("</{tag}>"));
    }
    let mut out = String::from(r#"<root BTCPP_format="4"><BehaviorTree ID="MainTree">"#);
    write(&mut out, 0, nodes, fanout);
    out.push_str("</BehaviorTree></root>");
    out
}

/// A small deterministic random number generator (xorshift).
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn status_of(n: u64) -> NodeStatus {
    match n % 4 {
        0 => NodeStatus::Idle,
        1 => NodeStatus::Running,
        2 => NodeStatus::Success,
        _ => NodeStatus::Failure,
    }
}

fn input(time: f64) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
        time: Some(time),
        ..Default::default()
    }
}

/// The average time of `f` over `runs` runs.
fn average(runs: u32, mut f: impl FnMut(u32)) -> Duration {
    let start = Instant::now();
    for run in 0..runs {
        f(run);
    }
    start.elapsed() / runs
}

#[test]
fn live_and_replay_with_5000_nodes() {
    let runtime = RuntimeTree::parse(&runtime_xml(NODES, 6)).unwrap();
    assert_eq!(runtime.nodes.len(), NODES);
    let tree = CanvasTree::from_runtime(
        &runtime,
        RuntimeBuildOptions::for_tree(&runtime),
        &Expansion::none(),
        &BuiltinCategories,
    );
    assert_eq!(tree.len(), NODES);
    let mut random = Random(0x9E37_79B9_7F4A_7C15);

    // Live: a status reply of the executor (3 bytes for each node), as the
    // live controller decodes it and puts it on the canvas tree.
    let replies: Vec<Vec<u8>> = (0..8)
        .map(|_| {
            let mut bytes = Vec::with_capacity(NODES * 3);
            for uid in 1..=NODES as u16 {
                bytes.extend_from_slice(&uid.to_le_bytes());
                bytes.push(RawStatus::encode(status_of(random.next()), NodeStatus::Idle).0);
            }
            bytes
        })
        .collect();
    let mut live_tree = tree.clone();
    let live = average(50, |run| {
        let snapshot = StatusSnapshot::decode(&replies[run as usize % replies.len()]).unwrap();
        live_tree.reset_status();
        for entry in &snapshot.entries {
            live_tree.set_status(entry.uid, entry.current, entry.previous);
        }
    });
    println!("live: decode and apply a status reply of {NODES} nodes: {live:?}");
    assert!(live < Duration::from_millis(50), "{live:?}");

    // Replay: a log of one million transitions.
    let mut time = 0u64;
    let transitions: Vec<Transition> = (0..1_000_000)
        .map(|_| {
            time += 1 + random.below(200);
            Transition::new(
                time,
                Uid(1 + random.below(NODES as u64) as u16),
                status_of(random.next()),
            )
        })
        .collect();
    let start = Instant::now();
    let index = ReplayIndex::new(&transitions);
    let build = start.elapsed();
    println!(
        "replay: index of {} transitions: {build:?}",
        transitions.len()
    );
    assert!(build < Duration::from_secs(20), "{build:?}");

    let last = index.last_time().unwrap();
    let mut replay_tree = tree.clone();
    let seek = average(200, |_| {
        let state = index.state_at(random.below(last));
        apply_replay_state(&mut replay_tree, &state);
    });
    println!("replay: seek to a random time and apply the state: {seek:?}");
    assert!(seek < Duration::from_millis(50), "{seek:?}");

    // The canvas while the statuses change on each frame (playback).
    let ctx = egui::Context::default();
    fasttick_app::fonts::install(&ctx);
    let palette = CanvasPalette::dark();
    let mut state = CanvasState::new(replay_tree);
    let mut now = 0.0;
    let frame = |state: &mut CanvasState, now: &mut f64| {
        *now += 1.0 / 60.0;
        let mut output = ctx.run_ui(input(*now), |ui| {
            egui::CentralPanel::no_frame().show(ui, |ui| {
                TreeCanvas::new(state, &palette).show(ui);
            });
        });
        // A real renderer would upload the font atlas changes.
        output.textures_delta.clear();
        std::hint::black_box(ctx.tessellate(output.shapes, output.pixels_per_point));
    };
    for _ in 0..3 {
        frame(&mut state, &mut now);
    }
    let bounds = state.layout().bounds;
    for (name, viewport) in [
        ("fit", Viewport::fit(bounds, SIZE, 32.0, 1.0)),
        (
            "zoom 1.0",
            Viewport::centered_on(bounds.center(), SIZE, 1.0),
        ),
    ] {
        state.set_viewport(viewport);
        frame(&mut state, &mut now);
        let mut play_time = 0;
        let per_frame = average(30, |_| {
            play_time = (play_time + last / 30).min(last);
            let replay = index.state_at(play_time);
            apply_replay_state(state.tree_mut(), &replay);
            frame(&mut state, &mut now);
        });
        println!("replay playback, {name:9}: apply and paint one frame: {per_frame:?}");
        assert!(
            per_frame < Duration::from_millis(500),
            "{name}: {per_frame:?}"
        );
    }
}
