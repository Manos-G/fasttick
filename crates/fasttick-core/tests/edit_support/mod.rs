//! Helpers for the edit tests.

#![allow(dead_code)]

use fasttick_core::bt::BtFile;
use fasttick_core::edit::{CommandError, Editor, History};
use fasttick_core::project::{FileId, LayoutState, Project};
use fasttick_core::xml::{Document, NodeId};

/// The state of a project that an undo must restore: the text of each
/// file, the unconnected nodes and the layout states.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub texts: Vec<(FileId, String)>,
    pub unconnected: Vec<(FileId, String, NodeId, [f32; 2])>,
    pub layouts: Vec<(FileId, String, Option<LayoutState>)>,
}

/// Tree IDs whose layout state the snapshot records (besides the trees of
/// the files).
const EXTRA_LAYOUT_KEYS: &[&str] = &["Main", "Walk", "Sub", "New", "Renamed", "Greet"];

pub fn snapshot(project: &Project) -> Snapshot {
    let texts = project
        .files()
        .map(|(id, _, f)| (id, f.to_xml_string()))
        .collect();
    let unconnected = project
        .all_unconnected()
        .map(|(f, t, n)| (f, t.to_owned(), n.node, n.position))
        .collect();
    let mut layouts = Vec::new();
    for (id, _, f) in project.files() {
        let mut keys: Vec<String> = f.trees().iter().map(|t| t.id.clone()).collect();
        keys.extend(EXTRA_LAYOUT_KEYS.iter().map(|s| (*s).to_owned()));
        keys.sort();
        keys.dedup();
        for key in keys {
            layouts.push((id, key.clone(), project.layout(id, &key).cloned()));
        }
    }
    Snapshot {
        texts,
        unconnected,
        layouts,
    }
}

/// A project with one in-memory file.
pub fn project(text: &str) -> (Project, FileId) {
    let file = BtFile::parse(text, None).expect("test text parses");
    Project::with_file("test", file)
}

/// The text of a file.
pub fn text(project: &Project, file: FileId) -> String {
    project.file(file).unwrap().to_xml_string()
}

/// The first element with this tag in the file.
pub fn tag(project: &Project, file: FileId, name: &str) -> NodeId {
    let doc = project.file(file).unwrap().doc();
    doc.descendants(doc.root())
        .find(|&n| doc.name(n) == Some(name))
        .unwrap_or_else(|| panic!("no <{name}>"))
}

/// The first element with this `name` attribute in the file.
pub fn named(project: &Project, file: FileId, name: &str) -> NodeId {
    let doc = project.file(file).unwrap().doc();
    doc.descendants(doc.root())
        .find(|&n| doc.attribute(n, "name") == Some(name))
        .unwrap_or_else(|| panic!("no node named {name}"))
}

/// Checks that a text parses and writes back unchanged, and that the
/// index of a file with this text equals the index of the project file.
pub fn check_well_formed(project: &Project) {
    for (id, _, file) in project.files() {
        let text = file.to_xml_string();
        let doc = Document::parse(&text)
            .unwrap_or_else(|e| panic!("{id}: the edited text does not parse: {e}\n{text}"));
        assert_eq!(doc.to_string(), text, "{id}: no round trip");
        assert_eq!(
            doc.write_regenerated(),
            text,
            "{id}: the parts of the start tags do not hold the text"
        );
        let fresh = BtFile::parse(&text, None).unwrap();
        let ids: Vec<&str> = fresh.trees().iter().map(|t| t.id.as_str()).collect();
        let current: Vec<&str> = file.trees().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, current, "{id}: the index is stale");
        assert_eq!(
            fresh.index().main_tree_to_execute,
            file.index().main_tree_to_execute
        );
        assert_eq!(
            fresh.index().models.models.len(),
            file.index().models.models.len()
        );
    }
}

/// Runs one session through a history. Checks that undo restores the
/// exact state before (texts, unconnected nodes, layouts and the dirty
/// flags), that redo restores the exact state after, and that the edited
/// files are well formed. Returns the value of the session.
pub fn check<T>(
    project: &mut Project,
    f: impl FnOnce(&mut Editor<'_>) -> Result<T, CommandError>,
) -> T {
    let mut history = History::new();
    let before = snapshot(project);
    let dirty_before: Vec<bool> = project.files().map(|(_, _, f)| f.is_dirty()).collect();
    let value = history.run(project, f).expect("the command succeeds");
    let after = snapshot(project);
    check_well_formed(project);
    if after.texts != before.texts {
        assert!(history.can_undo(), "a change must give an undo step");
    }
    if history.can_undo() {
        history.undo(project).unwrap();
        assert_eq!(snapshot(project), before, "undo does not restore the state");
        let dirty: Vec<bool> = project.files().map(|(_, _, f)| f.is_dirty()).collect();
        assert_eq!(dirty, dirty_before, "undo does not restore the dirty flags");
        history.redo(project).unwrap();
        assert_eq!(snapshot(project), after, "redo does not restore the state");
        check_well_formed(project);
    }
    value
}

/// Runs a session that must fail, and checks that the project did not
/// change.
pub fn check_error<T: std::fmt::Debug>(
    project: &mut Project,
    f: impl FnOnce(&mut Editor<'_>) -> Result<T, CommandError>,
) -> CommandError {
    let mut history = History::new();
    let before = snapshot(project);
    let dirty_before: Vec<bool> = project.files().map(|(_, _, f)| f.is_dirty()).collect();
    let error = history.run(project, f).expect_err("the command fails");
    assert_eq!(
        snapshot(project),
        before,
        "a failed command changed the project"
    );
    let dirty: Vec<bool> = project.files().map(|(_, _, f)| f.is_dirty()).collect();
    assert_eq!(dirty, dirty_before);
    assert!(!history.can_undo());
    // The message is plain text for the user.
    assert!(!error.to_string().is_empty());
    error
}
