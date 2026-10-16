//! "Open from robot" end to end: the dialog, the copy, the upload after a
//! save, the command after the save, and a file that changed on the robot.
//! The "robot" is a private OpenSSH server (see `sshd/`); the test skips if
//! OpenSSH is not installed.

mod sshd;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use fasttick_app::app::{FastTickApp, Mode, StartOptions};
use fasttick_app::canvas::NodeSource;
use fasttick_app::edit_ops::EditOp;
use fasttick_app::remote::ssh::{self, SshConfig};
use fasttick_app::shortcuts::Command;
use fasttick_app::theme::ThemeChoice;
use sshd::{HOST, TestSshd};

const MAIN: &str = r#"<root BTCPP_format="4" main_tree_to_execute="Main">
  <BehaviorTree ID="Main">
    <Sequence name="seq">
      <AlwaysSuccess name="first"/>
    </Sequence>
  </BehaviorTree>
</root>
"#;

fn harness() -> Harness<'static, FastTickApp> {
    let options = StartOptions {
        open: Vec::new(),
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
    harness
}

/// Runs frames until `done` is true (the remote jobs run on threads).
fn wait_for(
    harness: &mut Harness<'static, FastTickApp>,
    what: &str,
    done: impl Fn(&FastTickApp) -> bool,
) {
    let start = Instant::now();
    while !done(harness.state()) {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "timeout: {what}; notices: {:?}; dialog: {:?}",
            harness.state().notices(),
            harness.state().remote().dialog
        );
        std::thread::sleep(Duration::from_millis(30));
        harness.run_steps(3);
    }
}

fn text_field<'a>(
    harness: &'a Harness<'static, FastTickApp>,
    hint: &str,
) -> egui_kittest::Node<'a> {
    harness
        .query_all_by_role(Role::TextInput)
        .find(|n| n.accesskit_node().placeholder() == Some(hint))
        .unwrap_or_else(|| panic!("a text field with the hint {hint:?}"))
}

/// Focuses a text field and types.
fn type_into(harness: &mut Harness<'static, FastTickApp>, hint: &str, text: &str) {
    text_field(harness, hint).focus();
    harness.run_steps(3);
    text_field(harness, hint).type_text(text);
    harness.run_steps(3);
}

fn active_title(app: &FastTickApp) -> Option<String> {
    let view = app.workspace().active()?;
    Some(app.workspace().doc(view.doc)?.title.clone())
}

/// Renames the node `from` of the active document and saves.
fn rename_and_save(harness: &mut Harness<'static, FastTickApp>, from: &str, to: &str) {
    let ctx = harness.ctx.clone();
    let app = harness.state_mut();
    let view = app.active_edit_view().expect("an edit view");
    let tree = app.workspace().active().unwrap().canvas.tree();
    let node = tree
        .nodes()
        .iter()
        .find(|n| n.label == from)
        .and_then(|n| match n.source {
            NodeSource::Xml(_, node) => Some(node),
            _ => None,
        })
        .expect("the node");
    app.run_edit(
        &ctx,
        view,
        EditOp::SetName {
            node,
            name: Some(to.to_owned()),
        },
    );
    app.run_command(&ctx, Command::Save);
    harness.run_steps(3);
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn open_edit_save_and_conflict_on_a_robot() {
    let Some(server) = TestSshd::start() else {
        return;
    };
    let robot = server.files().join("trees");
    std::fs::create_dir_all(&robot).unwrap();
    // The main file includes a file of another folder by its absolute
    // path on the robot.
    let lib = server.files().join("lib/lib.xml");
    std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
    std::fs::write(
        &lib,
        r#"<root BTCPP_format="4"><BehaviorTree ID="Lib"><AlwaysSuccess/></BehaviorTree></root>"#,
    )
    .unwrap();
    let main_text = MAIN.replace(
        "  <BehaviorTree ID=\"Main\">",
        &format!(
            "  <include path=\"{}\"/>\n  <BehaviorTree ID=\"Main\">",
            lib.display()
        ),
    );
    assert!(main_text.contains("<include"));
    let main = robot.join("main.xml");
    std::fs::write(&main, &main_text).unwrap();
    std::fs::write(robot.join("notes.txt"), "not a tree").unwrap();
    let marker = server.files().join("restarted");

    let cache = tempfile::tempdir().unwrap();
    let mut harness = harness();
    harness.state_mut().remote_mut().configure(
        cache.path().to_path_buf(),
        Some(server.client_config()),
        Some(PathBuf::from(env!("CARGO_BIN_EXE_fasttick"))),
    );

    // File > Open from robot: the address and the folder, then Connect.
    let ctx = harness.ctx.clone();
    harness.state_mut().run_command(&ctx, Command::OpenRemote);
    harness.run_steps(3);
    type_into(&mut harness, "pi@robot.local", HOST);
    type_into(&mut harness, "your home folder", robot.to_str().unwrap());
    harness
        .get_by_role_and_label(Role::Button, "Connect")
        .click();
    harness.run_steps(3);
    wait_for(&mut harness, "the folder listing", |app| {
        app.remote()
            .dialog
            .as_ref()
            .is_some_and(|d| d.listing.is_some())
    });
    // The window grows with the list: let its size settle before a click.
    let _ = harness.try_run();
    // The tree file shows; the text file does not.
    assert!(harness.query_by_label_contains("notes.txt").is_none());
    harness
        .query_all(egui_kittest::kittest::by().label_contains("main.xml"))
        .find(|n| n.accesskit_node().role() == Role::Button)
        .expect("the file row")
        .click();
    harness.run_steps(3);
    harness.get_by_role_and_label(Role::Button, "Open").click();
    harness.run_steps(3);
    wait_for(&mut harness, "the open document", |app| {
        active_title(app).as_deref() == Some("fasttick-test: main.xml")
    });
    {
        let app = harness.state();
        assert!(app.remote().dialog.is_none());
        let url = format!("ssh://{HOST}{}", main.display());
        assert_eq!(
            app.settings().recent.first().map(|r| r.path.clone()),
            Some(PathBuf::from(&url))
        );
        // The absolute include resolves to the copy of the robot's file.
        let view = app.workspace().active().unwrap();
        let editor = app
            .workspace()
            .doc(view.doc)
            .unwrap()
            .editor
            .as_ref()
            .unwrap();
        assert!(editor.project().find_tree("Lib").is_some());
        let include = editor.project().includes(editor.file_id())[0]
            .path
            .clone()
            .unwrap();
        assert!(
            include.starts_with(cache.path()),
            "the include must be the copy in the cache: {}",
            include.display()
        );
        assert!(
            editor
                .problems()
                .iter()
                .all(|p| !p.message.contains("include")),
            "{:?}",
            editor.problems()
        );
        let saved = &app.settings().remote_hosts[0];
        assert_eq!(saved.address, HOST);
        assert_eq!(saved.folder, robot.to_str().unwrap());
    }

    // A save uploads the file. Then the command after the save runs.
    harness.state_mut().settings_mut().remote_hosts[0].restart_command =
        format!("touch '{}'", marker.display());
    rename_and_save(&mut harness, "first", "renamed");
    wait_for(&mut harness, "the upload", |app| {
        app.remote().index.pending().is_empty() && app.remote().uploading == 0
    });
    assert!(read(&main).contains(r#"name="renamed""#), "{}", read(&main));
    wait_for(&mut harness, "the command after the save", |_| {
        marker.exists()
    });
    assert!(
        harness
            .state()
            .notices()
            .iter()
            .any(|n| n.text.contains("Saved main.xml to fasttick-test")),
        "{:?}",
        harness.state().notices()
    );

    // Somebody changes the file on the robot. The next save does not
    // overwrite it: FastTick asks.
    let robot_version = read(&main).replace("renamed", "changed_on_robot");
    std::fs::write(&main, &robot_version).unwrap();
    rename_and_save(&mut harness, "renamed", "mine");
    wait_for(&mut harness, "the conflict", |app| {
        !app.remote().conflicts.is_empty()
    });
    assert_eq!(read(&main), robot_version);
    let _ = harness.try_run();
    harness.get_by_label("Changed on the robot");
    assert!(harness.state().remote().index.pending().len() == 1);
    harness
        .get_by_role_and_label(Role::Button, "Reload from robot")
        .click();
    harness.run_steps(3);
    wait_for(&mut harness, "the reloaded document", |app| {
        app.workspace().active().is_some_and(|view| {
            view.canvas
                .tree()
                .nodes()
                .iter()
                .any(|n| n.label == "changed_on_robot")
        })
    });
    assert!(harness.state().remote().index.pending().is_empty());
    assert_eq!(read(&main), robot_version);

    let control = SshConfig {
        program: PathBuf::from("ssh"),
        config_file: Some(server.client_config()),
        askpass: None,
        control_dir: Some(cache.path().join("ctl")),
    };
    ssh::close_master(&control, &server.host(HOST));
}
