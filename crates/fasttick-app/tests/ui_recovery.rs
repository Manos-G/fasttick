//! Headless UI tests of the copies of unsaved changes: a FastTick that
//! stops without a normal close leaves copies, and the next FastTick
//! offers to restore them.

use std::fs;
use std::path::{Path, PathBuf};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use fasttick_app::app::{FastTickApp, Mode, StartOptions};
use fasttick_app::documents::DocId;
use fasttick_app::shortcuts::Command;
use fasttick_app::theme::ThemeChoice;

const MAIN: &str = r#"<root BTCPP_format="4" main_tree_to_execute="Main">
  <include path="sub.xml"/>
  <BehaviorTree ID="Main">
    <SubTree ID="Sub"/>
  </BehaviorTree>
</root>
"#;

const SUB: &str = r#"<root BTCPP_format="4">
  <BehaviorTree ID="Sub">
    <AlwaysSuccess/>
  </BehaviorTree>
</root>
"#;

fn harness(root: &Path, open: Vec<PathBuf>) -> Harness<'static, FastTickApp> {
    let options = StartOptions {
        open,
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
    harness.state_mut().use_recovery_dir(root);
    harness.run();
    harness
}

fn files(dir: &Path) -> (PathBuf, PathBuf) {
    let main = dir.join("main.xml");
    let sub = dir.join("sub.xml");
    fs::write(&main, MAIN).unwrap();
    fs::write(&sub, SUB).unwrap();
    (main, sub)
}

fn active_doc(harness: &Harness<'static, FastTickApp>) -> DocId {
    harness.state().workspace().active().unwrap().doc
}

/// Adds a tree to the main file and a node to the included file.
fn change_both_files(harness: &mut Harness<'static, FastTickApp>) {
    let doc = active_doc(harness);
    let app = harness.state_mut();
    let editor = app.workspace_mut().editor_mut(doc).unwrap();
    let main = editor.file_id();
    let sub = editor
        .tree_files()
        .into_iter()
        .find(|f| *f != main)
        .expect("the included file");
    editor
        .edit(|ed| {
            ed.add_tree(main, "Restored")?;
            ed.add_tree(sub, "RestoredSub")
        })
        .unwrap();
    app.workspace_mut().refresh_editor_views();
    harness.run();
}

fn records(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    out.sort();
    out
}

fn tree_ids(harness: &Harness<'static, FastTickApp>, doc: DocId) -> Vec<String> {
    let app = harness.state();
    let editor = app.workspace().doc(doc).unwrap().editor.as_ref().unwrap();
    editor.trees().into_iter().map(|(_, id)| id).collect()
}

#[test]
fn changes_of_a_stopped_fasttick_can_be_restored() {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (main, sub) = files(work.path());

    let mut first = harness(root.path(), vec![main.clone()]);
    assert_eq!(first.state().recovery_offered(), 0);
    let dir = first.state().recovery_dir().unwrap().to_path_buf();
    assert!(records(&dir).is_empty(), "no changes, no copy");
    change_both_files(&mut first);
    assert_eq!(records(&dir).len(), 1, "the first change is copied at once");
    let record = fs::read_to_string(&records(&dir)[0]).unwrap();
    assert!(record.contains("RestoredSub"), "{record}");

    // A crash: no normal close (`on_exit` does not run).
    drop(first);
    assert!(dir.exists());

    let mut second = harness(root.path(), Vec::new());
    assert_eq!(second.state().recovery_offered(), 1);
    second.get_by_label("Restore unsaved changes");
    second.get_by_label("main.xml");
    second
        .get_by_role_and_label(Role::Button, "Restore")
        .click();
    second.run();
    assert_eq!(second.state().recovery_offered(), 0);
    assert!(!dir.exists(), "the old copies are deleted");

    let doc = active_doc(&second);
    let ids = tree_ids(&second, doc);
    assert!(ids.contains(&"Restored".to_owned()), "{ids:?}");
    assert!(ids.contains(&"RestoredSub".to_owned()), "{ids:?}");
    let editor = second
        .state()
        .workspace()
        .doc(doc)
        .unwrap()
        .editor
        .as_ref()
        .unwrap();
    assert!(editor.is_dirty());
    assert_eq!(editor.path().as_deref(), Some(main.as_path()));
    // The files on disk did not change.
    assert_eq!(fs::read_to_string(&main).unwrap(), MAIN);
    assert_eq!(fs::read_to_string(&sub).unwrap(), SUB);
    // The second FastTick keeps a copy of the restored changes.
    let second_dir = second.state().recovery_dir().unwrap().to_path_buf();
    assert_eq!(records(&second_dir).len(), 1);

    // A save writes the files and deletes the copy.
    let ctx = second.ctx.clone();
    second.state_mut().run_command(&ctx, Command::Save);
    second.run();
    assert!(fs::read_to_string(&main).unwrap().contains("Restored"));
    assert!(fs::read_to_string(&sub).unwrap().contains("RestoredSub"));
    assert!(records(&second_dir).is_empty());

    // A normal close deletes the folder of the copies.
    eframe::App::on_exit(second.state_mut());
    assert!(!second_dir.exists());
}

#[test]
fn a_new_file_is_restored_and_delete_removes_copies() {
    let root = tempfile::tempdir().unwrap();
    let mut first = harness(root.path(), Vec::new());
    let ctx = first.ctx.clone();
    first.state_mut().run_command(&ctx, Command::NewProject);
    first.run();
    change_main(&mut first);
    // The new file had unsaved changes (its first tree) before the change,
    // so its first copy is from before the change.
    let ctx = first.ctx.clone();
    first.state_mut().write_recovery_now(&ctx);
    let first_dir = first.state().recovery_dir().unwrap().to_path_buf();
    assert_eq!(records(&first_dir).len(), 1);
    drop(first);

    // "Not now" keeps the copies for the next start.
    let mut second = harness(root.path(), Vec::new());
    assert_eq!(second.state().recovery_offered(), 1);
    second.get_by_label("Untitled");
    second
        .get_by_role_and_label(Role::Button, "Not now")
        .click();
    second.run();
    assert!(first_dir.exists());
    drop(second);

    let mut third = harness(root.path(), Vec::new());
    assert_eq!(third.state().recovery_offered(), 1);
    third.get_by_role_and_label(Role::Button, "Restore").click();
    third.run();
    let doc = active_doc(&third);
    let ids = tree_ids(&third, doc);
    assert!(ids.contains(&"Restored".to_owned()), "{ids:?}");
    let app = third.state();
    let document = app.workspace().doc(doc).unwrap();
    assert_eq!(document.title, "Untitled");
    assert!(document.editor.as_ref().unwrap().path().is_none());
    drop(third);

    // The third FastTick stopped too: its copy is offered, and Delete
    // removes it.
    let mut fourth = harness(root.path(), Vec::new());
    assert_eq!(fourth.state().recovery_offered(), 1);
    fourth.get_by_role_and_label(Role::Button, "Delete").click();
    fourth.run();
    drop(fourth);
    let fifth = harness(root.path(), Vec::new());
    assert_eq!(fifth.state().recovery_offered(), 0);
}

#[test]
fn a_document_without_changes_has_no_copy() {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (main, _) = files(work.path());
    let mut first = harness(root.path(), vec![main]);
    let dir = first.state().recovery_dir().unwrap().to_path_buf();
    change_main(&mut first);
    assert_eq!(records(&dir).len(), 1);
    // Undo: no unsaved changes, so the copy goes away.
    let ctx = first.ctx.clone();
    first.state_mut().run_command(&ctx, Command::Undo);
    first.run();
    assert!(records(&dir).is_empty());
}

fn change_main(harness: &mut Harness<'static, FastTickApp>) {
    let doc = active_doc(harness);
    let app = harness.state_mut();
    let editor = app.workspace_mut().editor_mut(doc).unwrap();
    let main = editor.file_id();
    editor.edit(|ed| ed.add_tree(main, "Restored")).unwrap();
    app.workspace_mut().refresh_editor_views();
    harness.run();
}

#[test]
fn changes_go_into_a_document_that_is_open_already() {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (main, _) = files(work.path());
    let mut first = harness(root.path(), vec![main.clone()]);
    change_main(&mut first);
    drop(first);

    // The second FastTick opens the same file from the command line.
    let mut second = harness(root.path(), vec![main]);
    assert_eq!(second.state().workspace().docs().len(), 1);
    second
        .get_by_role_and_label(Role::Button, "Restore")
        .click();
    second.run();
    assert_eq!(second.state().workspace().docs().len(), 1);
    let doc = active_doc(&second);
    assert!(tree_ids(&second, doc).contains(&"Restored".to_owned()));
    let app = second.state();
    let editor = app.workspace().doc(doc).unwrap().editor.as_ref().unwrap();
    assert!(editor.is_dirty());
    assert!(!editor.can_undo(), "the undo steps were for the old text");
}
