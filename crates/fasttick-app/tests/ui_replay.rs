//! Headless UI tests of replay mode with the harness log
//! (`tests/fixtures/harness/main.btlog`): playback and steps, the time bar
//! (scrub and range zoom), the transitions table (filter, row click,
//! export) and the statistics table.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use fasttick_app::app::{FastTickApp, Mode, StartOptions};
use fasttick_app::dock::Tab;
use fasttick_app::jobs::ExportFormat;
use fasttick_app::replay::StatsColumn;
use fasttick_app::shortcuts::Command;
use fasttick_app::theme::ThemeChoice;
use fasttick_core::status::NodeStatus;

fn main_log() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/harness/main.btlog")
}

fn harness() -> Harness<'static, FastTickApp> {
    let options = StartOptions {
        open: vec![main_log()],
        theme: Some(ThemeChoice::Light),
        mode: Some(Mode::Replay),
        connect: None,
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1500.0, 1000.0))
        .with_theme(egui::Theme::Light)
        .with_max_steps(400)
        .with_step_dt(1.0 / 30.0)
        .build_eframe(move |cc| FastTickApp::new(cc, options));
    harness.run();
    harness
}

fn applied(harness: &Harness<'static, FastTickApp>) -> usize {
    harness.state().active_replay().unwrap().cursor().applied()
}

fn total(harness: &Harness<'static, FastTickApp>) -> usize {
    harness.state().active_replay().unwrap().index().len()
}

fn drag(harness: &mut Harness<'static, FastTickApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    harness.event(Event::PointerMoved(from));
    harness.event(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers,
    });
    harness.run_ok();
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        harness.event(Event::PointerMoved(from + (to - from) * t));
        harness.run_ok();
    }
    harness.event(Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers,
    });
    harness.run_ok();
}

#[test]
fn playback_steps_and_keys() {
    let mut harness = harness();
    assert_eq!(harness.state().mode(), Mode::Replay);
    let count = total(&harness);
    assert!(count > 50, "{count}");
    // The log opens at its end.
    assert_eq!(applied(&harness), count);
    harness.get_by_label(&format!("transition {count} of {count}"));

    harness.get_by_label("Go to the start").click();
    harness.run();
    assert_eq!(applied(&harness), 0);
    harness.get_by_label("One transition forward").click();
    harness.run();
    assert_eq!(applied(&harness), 1);
    // Keys: "." forward, "," back.
    harness.key_press(egui::Key::Period);
    harness.run();
    assert_eq!(applied(&harness), 2);
    harness.key_press(egui::Key::Comma);
    harness.run();
    assert_eq!(applied(&harness), 1);
    harness.get_by_label("One transition back").click();
    harness.run();
    assert_eq!(applied(&harness), 0);

    // Play at 1x: the log is about 0.2 s long; each frame is 1/30 s.
    harness.get_by_label("Play").click();
    harness.step();
    assert!(
        harness
            .state()
            .active_replay()
            .unwrap()
            .cursor()
            .is_playing()
    );
    harness.step();
    harness.step();
    let during = applied(&harness);
    assert!(during > 0 && during < count, "{during}");
    // The canvas shows a running node during playback.
    {
        let view = harness.state().workspace().active().unwrap();
        assert!(
            view.canvas
                .tree()
                .nodes()
                .iter()
                .any(|n| n.deco.status != NodeStatus::Idle || n.deco.previous_status.is_some())
        );
    }
    harness.run();
    assert_eq!(applied(&harness), count);
    assert!(
        !harness
            .state()
            .active_replay()
            .unwrap()
            .cursor()
            .is_playing()
    );

    // P toggles playback; at the end it starts again from the start.
    harness.key_press(egui::Key::P);
    harness.step();
    assert!(
        harness
            .state()
            .active_replay()
            .unwrap()
            .cursor()
            .is_playing()
    );
    let ctx = harness.ctx.clone();
    harness.state_mut().run_command(&ctx, Command::PlayPause);
    harness.run();
    assert!(
        !harness
            .state()
            .active_replay()
            .unwrap()
            .cursor()
            .is_playing()
    );

    // The speed selector.
    harness.get_by_role(Role::ComboBox).click();
    harness.run();
    harness.get_by_label("10\u{00D7}").click();
    harness.run();
    let speed = harness
        .state()
        .active_replay()
        .unwrap()
        .cursor()
        .speed()
        .factor();
    assert!((speed - 10.0).abs() < 1e-9, "{speed}");
}

#[test]
fn time_bar_scrub_and_range_zoom() {
    let mut harness = harness();
    let end = harness.state().active_replay().unwrap().end_us();
    // A click in the middle of the bar moves the position there.
    let bar = harness
        .get_by_role_and_label(Role::Slider, "Timeline position")
        .rect();
    harness
        .get_by_role_and_label(Role::Slider, "Timeline position")
        .click();
    harness.run();
    let time = harness.state().active_replay().unwrap().cursor().time_us();
    let middle = end / 2;
    assert!(time.abs_diff(middle) < end / 50 + 2, "{time} vs {middle}");

    // A drag on the bar scrubs.
    let lane_y = bar.bottom() - 8.0;
    drag(
        &mut harness,
        egui::pos2(bar.left() + bar.width() * 0.2, lane_y),
        egui::pos2(bar.left() + bar.width() * 0.8, lane_y),
        Modifiers::NONE,
    );
    let time = harness.state().active_replay().unwrap().cursor().time_us();
    assert!(time > end / 2, "{time}");
    assert!(
        harness
            .state()
            .active_replay()
            .unwrap()
            .cursor()
            .range()
            .is_none()
    );

    // A drag on the ruler selects a time range and zooms into it.
    let ruler_y = bar.top() + 6.0;
    drag(
        &mut harness,
        egui::pos2(bar.left() + bar.width() * 0.25, ruler_y),
        egui::pos2(bar.left() + bar.width() * 0.5, ruler_y),
        Modifiers::NONE,
    );
    let range = harness
        .state()
        .active_replay()
        .unwrap()
        .cursor()
        .range()
        .expect("a zoom range");
    assert!(range.0.abs_diff(end / 4) < end / 50 + 2, "{range:?}");
    assert!(range.1.abs_diff(end / 2) < end / 50 + 2, "{range:?}");
    assert!(harness.state().is_enabled(Command::ResetTimelineZoom));
    // Statistics follow the range.
    harness.state_mut().show_panel(Tab::Statistics);
    harness.run();
    harness.get_by_label_contains("(the timeline zoom)");
    harness.get_by_label("Show the whole log").click();
    harness.run();
    assert!(
        harness
            .state()
            .active_replay()
            .unwrap()
            .cursor()
            .range()
            .is_none()
    );
    harness.get_by_label_contains("The whole log.");
}

#[test]
fn transitions_table_filter_click_and_export() {
    let mut harness = harness();
    let count = total(&harness);
    harness.get_by_label(&format!("{count} of {count} transitions"));
    // The table shows the first rows.
    harness.get_by_label("0.000383");

    // Include filter.
    harness
        .get_by_role_and_label(Role::TextInput, "Include")
        .focus();
    harness.run();
    harness
        .get_by_role_and_label(Role::TextInput, "Include")
        .type_text("say*");
    harness.run();
    let rows = harness
        .state()
        .active_replay()
        .unwrap()
        .current_rows()
        .len();
    assert!(rows > 0 && rows < count, "{rows}");
    harness.get_by_label(&format!("{rows} of {count} transitions"));
    // Status filter: hide IDLE.
    harness
        .get_by_role_and_label(Role::CheckBox, "IDLE")
        .click();
    harness.run();
    let without_idle = harness
        .state()
        .active_replay()
        .unwrap()
        .current_rows()
        .len();
    assert!(without_idle < rows, "{without_idle} {rows}");
    {
        let model = harness.state().active_replay().unwrap();
        for row in model.current_rows() {
            let (transition, _) = model.transition(*row as usize).unwrap();
            assert_ne!(transition.status, NodeStatus::Idle);
            // "say*" matches the name, the path or the type.
            let node = model.node(transition.uid).unwrap();
            assert!(
                [&node.name, &node.path, &node.type_id]
                    .iter()
                    .any(|text| text.to_lowercase().starts_with("say")),
                "{node:?}"
            );
        }
    }

    // A click on a row moves the replay there and selects the node.
    let first_row = harness.state().active_replay().unwrap().current_rows()[1] as usize;
    let (transition, _) = harness
        .state()
        .active_replay()
        .unwrap()
        .transition(first_row)
        .unwrap();
    let name = harness
        .state()
        .active_replay()
        .unwrap()
        .node(transition.uid)
        .unwrap()
        .name
        .clone();
    let time_text = fasttick_app::replay::format_seconds(transition.offset_us);
    harness.get_by_label(&time_text).click();
    harness.run();
    assert_eq!(applied(&harness), first_row + 1);
    {
        let view = harness.state().workspace().active().unwrap();
        let primary = view.canvas.primary().expect("a selected node");
        assert_eq!(view.canvas.tree().get(primary).unwrap().label, name);
    }

    // Export the filtered rows.
    let dir = tempfile::tempdir().unwrap();
    let ctx = harness.ctx.clone();
    for format in [ExportFormat::Csv, ExportFormat::Json] {
        let path = dir.path().join(format!("rows.{}", format.extension()));
        assert!(
            harness
                .state_mut()
                .export_transitions_to(&ctx, path.clone(), format)
        );
        // Wait for the notice of this export: it names the file. (The
        // notice of the previous export can still be visible.)
        let notice = format!("Exported {without_idle} transitions to {}", path.display());
        let deadline = Instant::now() + Duration::from_secs(10);
        while harness.query_by_label_contains(&notice).is_none() {
            assert!(Instant::now() < deadline, "the export did not finish");
            harness.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        // A new notice area takes one more frame to reach its final
        // place (egui sizes an anchored area from its last frame), so let
        // the UI settle before the "Close" click below.
        harness.run();
        harness.get_by_label_contains(&notice);
        let text = std::fs::read_to_string(&path).unwrap();
        match format {
            ExportFormat::Csv => {
                assert_eq!(text.lines().count(), without_idle + 1);
                assert!(text.starts_with("offset_us,absolute_time_us,uid,node_name"));
            }
            ExportFormat::Json => {
                let rows: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(rows.as_array().unwrap().len(), without_idle);
            }
        }
        // Close the notice for the next export.
        harness.get_by_label("Close").click();
        harness.run();
        assert!(harness.state().notices().is_empty());
    }

    // A bad time field shows an error and blocks the export.
    harness
        .get_by_role_and_label(Role::TextInput, "From")
        .focus();
    harness.run();
    harness
        .get_by_role_and_label(Role::TextInput, "From")
        .type_text("soon");
    harness.run();
    harness.get_by_label_contains("From: enter a time in seconds");
    assert!(!harness.state_mut().export_transitions_to(
        &ctx,
        dir.path().join("bad.csv"),
        ExportFormat::Csv
    ));
    harness.get_by_label("Clear filter").click();
    harness.run();
    harness.get_by_label(&format!("{count} of {count} transitions"));
}

#[test]
fn statistics_sort_and_select() {
    let mut harness = harness();
    harness.state_mut().show_panel(Tab::Statistics);
    harness.run();
    harness.get_by_label("Running time");
    // Default sort: running time, largest first. The root ran longest.
    {
        let model = harness.state_mut().active_replay_mut().unwrap();
        let sort = model.stats_sort;
        assert_eq!(sort.column, StatsColumn::Running);
        let (stats, order) = model.stats();
        let first = stats[order[0]].running_time_us;
        assert!(order.iter().all(|i| stats[*i].running_time_us <= first));
    }
    harness.get_by_label("Node").click();
    harness.run();
    let (names, uid) = {
        let model = harness.state_mut().active_replay_mut().unwrap();
        assert_eq!(model.stats_sort.column, StatsColumn::Node);
        assert!(model.stats_sort.ascending);
        let (stats, order) = model.stats();
        let stats = stats.to_vec();
        let order = order.to_vec();
        let names: Vec<String> = order
            .iter()
            .map(|i| model.node(stats[*i].uid).unwrap().name.clone())
            .collect();
        (names, stats[order[0]].uid)
    };
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    // A click on a row selects the node on the canvas.
    let first = names[0].clone();
    let cell = harness
        .query_all_by_label(&first)
        .find(|n| n.accesskit_node().role() == Role::Label)
        .expect("the row of the first node")
        .rect()
        .center();
    harness.event(Event::PointerMoved(cell));
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos: cell,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run();
    let view = harness.state().workspace().active().unwrap();
    let primary = view.canvas.primary().expect("a selected node");
    assert_eq!(view.canvas.tree().get(primary).unwrap().uid, Some(uid));
}

#[test]
fn conditions_show_on_cards_and_in_the_inspector() {
    let mut harness = harness();
    {
        let app = harness.state();
        let tree = app.workspace().active().unwrap().canvas.tree();
        let node = tree
            .nodes()
            .iter()
            .find(|n| n.label == "on_success")
            .expect("the node on_success");
        let names: Vec<&str> = node.conditions.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["_onSuccess", "_post"]);
        assert!(node.card.conditions.len() == 2, "the card shows both");
    }
    // Custom node types get the category of the <TreeNodesModel> in the
    // log: CheckFlag is a condition.
    {
        let app = harness.state();
        let tree = app.workspace().active().unwrap().canvas.tree();
        let node = tree
            .nodes()
            .iter()
            .find(|n| n.label == "check_blocked")
            .unwrap();
        assert_eq!(node.category, fasttick_app::canvas::NodeCategory::Condition);
        assert!(!node.category_inferred);
    }
    // The inspector lists the conditions of the selected node.
    let id = {
        let app = harness.state();
        let tree = app.workspace().active().unwrap().canvas.tree();
        tree.ids()
            .find(|id| tree.get(*id).unwrap().label == "skipped_node")
            .unwrap()
    };
    harness
        .state_mut()
        .workspace_mut()
        .active_mut()
        .unwrap()
        .canvas
        .tree_mut()
        .select_only(id);
    harness.run();
    harness.get_by_label("_skipIf");
    harness.get_by_label("flag == true");
}
