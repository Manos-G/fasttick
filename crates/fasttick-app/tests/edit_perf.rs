//! Time of one edit in a file with 5,000 nodes: the edit command, the
//! validation of the project, the new canvas tree and the next frame of the
//! canvas. The app does all of this after each change (also for each
//! keystroke in the inspector).
//!
//! Run with `--release -- --nocapture` to see the times.

use std::fmt::Write;
use std::time::{Duration, Instant};

use egui::{Pos2, Rect, vec2};
use fasttick_app::canvas::{CanvasState, Expansion, TreeCanvas};
use fasttick_app::editor::EditorDoc;
use fasttick_app::theme::CanvasPalette;
use fasttick_core::bt::BtFile;
use fasttick_core::edit::CoalesceKey;
use fasttick_core::project::Project;

/// A tree with `groups` Sequences of `leaves` named actions each, with
/// ports and a few conditions.
fn big_tree(groups: usize, leaves: usize) -> String {
    let mut xml = String::from(
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\">\n<BehaviorTree ID=\"Main\">\n<Sequence name=\"root\">\n",
    );
    for g in 0..groups {
        let _ = writeln!(xml, "<Sequence name=\"group_{g}\">");
        for l in 0..leaves {
            let _ = writeln!(
                xml,
                "<SetBlackboard name=\"set_{g}_{l}\" value=\"{l}\" output_key=\"key_{l}\" _skipIf=\"flag == {g}\"/>"
            );
        }
        xml.push_str("</Sequence>\n");
    }
    xml.push_str("</Sequence>\n</BehaviorTree>\n</root>\n");
    xml
}

/// Runs one frame of the canvas in a headless context and returns its
/// time (UI pass and tessellation).
fn frame(
    ctx: &egui::Context,
    state: &mut CanvasState,
    palette: &CanvasPalette,
    t: f64,
) -> Duration {
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))),
        time: Some(t),
        ..Default::default()
    };
    let start = Instant::now();
    let mut output = ctx.run_ui(input, |ui| {
        egui::CentralPanel::no_frame().show(ui, |ui| {
            TreeCanvas::new(state, palette).show(ui);
        });
    });
    output.textures_delta.clear();
    std::hint::black_box(ctx.tessellate(output.shapes, output.pixels_per_point));
    start.elapsed()
}

fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed())
}

#[test]
fn one_edit_with_5000_nodes() {
    let xml = big_tree(50, 99);
    let (mut editor, open) = time(|| {
        let (project, file) =
            Project::with_file("big", BtFile::parse(&xml, None).expect("valid XML"));
        EditorDoc::new(project, file)
    });
    let (tree, build) = time(|| editor.build_canvas(0, &Expansion::none()));
    assert_eq!(tree.len(), 1 + 50 + 50 * 99);
    let ctx = egui::Context::default();
    fasttick_app::fonts::install(&ctx);
    let palette = CanvasPalette::light();
    let mut state = CanvasState::new(tree);
    let first_frames =
        frame(&ctx, &mut state, &palette, 0.0) + frame(&ctx, &mut state, &palette, 0.1);

    // A rename of one node, as the inspector sends it for one keystroke.
    let file = editor.file_id();
    let node = editor
        .file()
        .and_then(|f| {
            let doc = f.doc();
            doc.descendants(doc.root())
                .find(|n| doc.element(*n).and_then(|e| e.attribute("name")) == Some("set_25_50"))
        })
        .expect("the node");
    let mut edits = Duration::ZERO;
    let mut rebuilds = Duration::ZERO;
    let mut frames = Duration::ZERO;
    const ROUNDS: u32 = 10;
    for round in 0..ROUNDS {
        let name = format!("renamed_{round}");
        let ((), edit) = time(|| {
            editor
                .edit_coalesced(CoalesceKey::attribute(file, node, "name"), |ed| {
                    ed.set_instance_name(file, node, Some(&name))
                })
                .expect("the rename");
        });
        let (tree, rebuild) = time(|| editor.build_canvas(0, &Expansion::none()));
        state.replace_tree(tree, None);
        let next = frame(&ctx, &mut state, &palette, 0.2 + f64::from(round) * 0.1);
        edits += edit;
        rebuilds += rebuild;
        frames += next;
    }
    let edit = edits / ROUNDS;
    let rebuild = rebuilds / ROUNDS;
    let next_frame = frames / ROUNDS;
    println!(
        "5,000 nodes: open + validate {open:?}, first canvas {build:?}, \
         first two frames {first_frames:?}; for each keystroke: edit + validate {edit:?}, \
         canvas rebuild {rebuild:?}, next frame {next_frame:?}"
    );
    // One undo step for all keystrokes.
    assert!(editor.undo().is_ok());
    assert!(
        editor
            .file()
            .unwrap()
            .to_xml_string()
            .contains("name=\"set_25_50\"")
    );

    // A keystroke must not block the UI. Debug builds are much slower, so
    // the limit is only strict in release builds.
    let limit = if cfg!(debug_assertions) {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(50)
    };
    assert!(
        edit + rebuild + next_frame < limit,
        "{edit:?} + {rebuild:?} + {next_frame:?}"
    );
}
