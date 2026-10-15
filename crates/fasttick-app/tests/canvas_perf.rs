//! Frame time of the canvas with a synthetic tree of 5,000 nodes.
//!
//! Each case runs the canvas in a headless egui context and measures the
//! UI pass (culling, text cache, shape building) and the tessellation.
//! The rendering on the GPU is not part of the measurement.
//!
//! Run with `--nocapture` to see the times. In a release build, all
//! cases stay far below 16 ms (60 frames a second).

use std::time::{Duration, Instant};

use egui::{Pos2, Rect, Vec2, pos2, vec2};
use fasttick_app::canvas::synthetic::synthetic_tree;
use fasttick_app::canvas::{CanvasState, Detail, TreeCanvas, Viewport};
use fasttick_app::theme::CanvasPalette;

const SIZE: Vec2 = vec2(1600.0, 1000.0);

fn input(time: f64) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
        time: Some(time),
        ..Default::default()
    }
}

struct Measurement {
    ui: Duration,
    tessellate: Duration,
    visible: usize,
    detail: Detail,
}

fn run_frames(
    ctx: &egui::Context,
    state: &mut CanvasState,
    palette: &CanvasPalette,
    frames: usize,
    time: &mut f64,
    mut change: impl FnMut(&mut CanvasState, usize),
) -> Measurement {
    let mut ui_total = Duration::ZERO;
    let mut tess_total = Duration::ZERO;
    for frame in 0..frames {
        change(state, frame);
        *time += 1.0 / 60.0;
        let start = Instant::now();
        let mut output = ctx.run_ui(input(*time), |ui| {
            egui::CentralPanel::no_frame().show(ui, |ui| {
                TreeCanvas::new(state, palette).show(ui);
            });
        });
        let after_ui = Instant::now();
        // A real renderer would upload the font atlas changes.
        output.textures_delta.clear();
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        std::hint::black_box(primitives);
        ui_total += after_ui - start;
        tess_total += after_ui.elapsed();
    }
    Measurement {
        ui: ui_total / frames as u32,
        tessellate: tess_total / frames as u32,
        visible: state.stats().visible_nodes,
        detail: state.stats().detail,
    }
}

#[test]
fn frame_time_with_5000_nodes() {
    let ctx = egui::Context::default();
    fasttick_app::fonts::install(&ctx);
    let palette = CanvasPalette::light();
    let mut state = CanvasState::new(synthetic_tree(5000, 6));
    let mut time = 0.0;

    // Warm up: fonts, measuring and layout (counted separately).
    let start = Instant::now();
    run_frames(&ctx, &mut state, &palette, 2, &mut time, |_, _| {});
    let first = start.elapsed();
    assert_eq!(state.tree().len(), 5000);
    assert!(!state.needs_layout());

    let bounds = state.layout().bounds;
    let canvas = vec2(SIZE.x, SIZE.y);
    let cases: Vec<(&str, Viewport)> = vec![
        (
            "fit (all nodes)",
            Viewport::fit(bounds, canvas, 32.0, 1.0),
        ),
        (
            "zoom 0.01 (all, boxes)",
            Viewport::centered_on(bounds.center(), canvas, 0.01),
        ),
        (
            "zoom 0.08 (boxes)",
            Viewport::centered_on(bounds.center(), canvas, 0.08),
        ),
        (
            "zoom 0.5 (labels)",
            Viewport::centered_on(bounds.center(), canvas, 0.5),
        ),
        (
            "zoom 1.0 (full cards)",
            Viewport::centered_on(bounds.center(), canvas, 1.0),
        ),
    ];
    println!("first two frames (measure + layout): {first:?}");
    for (name, viewport) in cases {
        state.set_viewport(viewport);
        run_frames(&ctx, &mut state, &palette, 2, &mut time, |_, _| {});
        let m = run_frames(&ctx, &mut state, &palette, 20, &mut time, |_, _| {});
        println!(
            "{name:24} visible {:5}  detail {:?}  ui {:?}  tessellate {:?}",
            m.visible, m.detail, m.ui, m.tessellate
        );
        // Generous limits: debug builds and shared CI machines are slow.
        assert!(m.ui < Duration::from_millis(400), "{name}: {:?}", m.ui);
    }

    // Panning at zoom 1: the text cache keeps the cost low.
    state.set_viewport(Viewport::centered_on(bounds.center(), canvas, 1.0));
    let m = run_frames(&ctx, &mut state, &palette, 30, &mut time, |state, frame| {
        let mut v = state.viewport();
        v.pan_by(vec2(if frame % 2 == 0 { 7.0 } else { -5.0 }, 3.0));
        state.set_viewport(v);
    });
    println!(
        "{:24} visible {:5}  detail {:?}  ui {:?}  tessellate {:?}",
        "pan at zoom 1.0", m.visible, m.detail, m.ui, m.tessellate
    );

    // Zooming through all levels of detail.
    let m = run_frames(&ctx, &mut state, &palette, 40, &mut time, |state, frame| {
        let mut v = state.viewport();
        let factor = if frame < 20 { 0.88 } else { 1.0 / 0.88 };
        v.zoom_around(Pos2::ZERO, pos2(800.0, 500.0), factor);
        state.set_viewport(v);
    });
    println!(
        "{:24} visible {:5}  detail {:?}  ui {:?}  tessellate {:?}",
        "zoom gesture", m.visible, m.detail, m.ui, m.tessellate
    );
    assert!(m.ui < Duration::from_millis(400));
}
