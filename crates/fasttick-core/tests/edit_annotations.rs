//! Node notes, groups and canvas notes: the edit commands and their XML.

use fasttick_core::bt::BtFile;
use fasttick_core::bt::annotations::{
    CanvasNoteInfo, GroupInfo, node_group, node_note, tree_groups, tree_notes,
};
use fasttick_core::edit::{CommandError, History, group_members};
use fasttick_core::project::{FileId, Project};
use fasttick_core::xml::NodeId;

const TREE: &str = r#"<root BTCPP_format="4">
    <BehaviorTree ID="Main">
        <Sequence name="seq">
            <AlwaysSuccess name="a"/>
            <AlwaysSuccess name="b"/>
            <Fallback name="fb">
                <AlwaysFailure name="c"/>
                <AlwaysSuccess name="d"/>
            </Fallback>
            <AlwaysSuccess name="e"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Other">
        <AlwaysSuccess name="x"/>
    </BehaviorTree>
</root>
"#;

fn setup() -> (Project, FileId) {
    Project::with_file("demo", BtFile::parse(TREE, None).unwrap())
}

fn named(p: &Project, f: FileId, name: &str) -> NodeId {
    let doc = p.file(f).unwrap().doc();
    doc.descendants(doc.root())
        .find(|&n| doc.attribute(n, "name") == Some(name))
        .unwrap()
}

fn tree_element(p: &Project, f: FileId, id: &str) -> NodeId {
    p.file(f).unwrap().tree(id).unwrap().element
}

fn xml(p: &Project, f: FileId) -> String {
    p.file(f).unwrap().to_xml_string()
}

fn groups(p: &Project, f: FileId) -> Vec<GroupInfo> {
    tree_groups(p.file(f).unwrap().doc(), tree_element(p, f, "Main"))
}

fn notes(p: &Project, f: FileId) -> Vec<CanvasNoteInfo> {
    tree_notes(p.file(f).unwrap().doc(), tree_element(p, f, "Main"))
}

fn group_of(p: &Project, f: FileId, name: &str) -> Option<u32> {
    node_group(p.file(f).unwrap().doc(), named(p, f, name))
}

#[test]
fn node_note_set_edit_remove_and_undo() {
    let (mut p, f) = setup();
    let a = named(&p, f, "a");
    let mut h = History::new();
    h.run(&mut p, |ed| {
        ed.set_node_note(f, a, Some("Wait for\nthe arm"))
    })
    .unwrap();
    assert!(xml(&p, f).contains(r#"<AlwaysSuccess name="a" _note="Wait for&#10;the arm"/>"#));
    assert_eq!(
        node_note(p.file(f).unwrap().doc(), a),
        Some("Wait for\nthe arm")
    );
    assert_eq!(h.undo_label(), Some("Edit note"));

    // A blank note removes the attribute.
    h.run(&mut p, |ed| ed.set_node_note(f, a, Some("  ")))
        .unwrap();
    assert_eq!(xml(&p, f), TREE);
    h.undo(&mut p).unwrap();
    h.undo(&mut p).unwrap();
    assert_eq!(xml(&p, f), TREE);
    assert!(!p.file(f).unwrap().is_dirty());

    // Not a tree node: the <BehaviorTree> element.
    let element = tree_element(&p, f, "Main");
    assert!(
        h.run(&mut p, |ed| ed.set_node_note(f, element, Some("x")))
            .is_err()
    );
}

#[test]
fn create_and_change_groups() {
    let (mut p, f) = setup();
    let (a, b, fb, c) = (
        named(&p, f, "a"),
        named(&p, f, "b"),
        named(&p, f, "fb"),
        named(&p, f, "c"),
    );
    let mut h = History::new();
    let first = h
        .run(&mut p, |ed| ed.create_group(f, "Main", &[a, b], "Pick"))
        .unwrap();
    assert_eq!(first, 1);
    let text = xml(&p, f);
    assert!(
        text.contains(r#"<BehaviorTree ID="Main" _group_1="Pick">"#),
        "{text}"
    );
    assert!(text.contains(r#"<AlwaysSuccess name="a" _group="1"/>"#));
    assert!(text.contains(r#"<AlwaysSuccess name="b" _group="1"/>"#));

    // A second group takes b from the first one.
    let second = h
        .run(&mut p, |ed| ed.create_group(f, "Main", &[b, fb, c], ""))
        .unwrap();
    assert_eq!(second, 2);
    assert_eq!(group_of(&p, f, "b"), Some(2));
    assert_eq!(
        groups(&p, f)
            .iter()
            .map(|g| g.title.as_str())
            .collect::<Vec<_>>(),
        ["Pick", "Group"]
    );

    // Members, title, description, color.
    h.run(&mut p, |ed| {
        ed.set_group_members(f, "Main", 1, &[a, c])?;
        ed.set_group_title(f, "Main", 1, "Pick the box")?;
        ed.set_group_description(f, "Main", 1, "Moves the arm.\nThen closes the hand.")?;
        ed.set_group_color(f, "Main", 1, Some("Green"))
    })
    .unwrap();
    let members = group_members(&p, f, "Main");
    assert_eq!(members[&1], vec![a, c]);
    assert_eq!(members[&2], vec![b, fb]);
    assert_eq!(
        groups(&p, f)[0],
        GroupInfo {
            id: 1,
            title: "Pick the box".into(),
            description: "Moves the arm.\nThen closes the hand.".into(),
            color: Some("green".into()),
        }
    );
    // An unknown color removes the attribute.
    h.run(&mut p, |ed| ed.set_group_color(f, "Main", 1, Some("pink")))
        .unwrap();
    assert_eq!(groups(&p, f)[0].color, None);

    // Errors: an empty group, a node of another tree, an unknown group.
    let x = named(&p, f, "x");
    assert_eq!(
        h.run(&mut p, |ed| ed.set_group_members(f, "Main", 1, &[])),
        Err(CommandError::EmptyGroup)
    );
    assert!(matches!(
        h.run(&mut p, |ed| ed.create_group(f, "Main", &[a, x], "x")),
        Err(CommandError::NotInTree { .. })
    ));
    assert!(matches!(
        h.run(&mut p, |ed| ed.set_group_title(f, "Main", 9, "x")),
        Err(CommandError::UnknownGroup { id: 9, .. })
    ));

    // Delete a group: the members stay, without the attribute.
    h.run(&mut p, |ed| ed.delete_group(f, "Main", 2)).unwrap();
    assert_eq!(group_of(&p, f, "b"), None);
    assert_eq!(group_of(&p, f, "fb"), None);
    assert_eq!(groups(&p, f).len(), 1);
    assert!(!xml(&p, f).contains("_group_2"));

    // Undo all: the exact text.
    while h.can_undo() {
        h.undo(&mut p).unwrap();
    }
    assert_eq!(xml(&p, f), TREE);
}

#[test]
fn groups_without_members_go_away() {
    let (mut p, f) = setup();
    let (a, b, e) = (named(&p, f, "a"), named(&p, f, "b"), named(&p, f, "e"));
    let mut h = History::new();
    h.run(&mut p, |ed| {
        ed.create_group(f, "Main", &[a, b], "One")?;
        ed.create_group(f, "Main", &[e], "Two")
    })
    .unwrap();
    h.run(&mut p, |ed| ed.remove_from_group(f, &[a, b]))
        .unwrap();
    assert_eq!(
        groups(&p, f).iter().map(|g| g.id).collect::<Vec<_>>(),
        [2],
        "the empty group 1 is gone"
    );
    // Delete the only member of group 2, then clean up.
    h.run(&mut p, |ed| {
        ed.delete_nodes(f, &[e])?;
        ed.remove_empty_groups(f)
    })
    .unwrap();
    assert!(groups(&p, f).is_empty());
    // A new group after that gets ID 1 again.
    let id = h
        .run(&mut p, |ed| ed.create_group(f, "Main", &[a], "Again"))
        .unwrap();
    assert_eq!(id, 1);
}

#[test]
fn unconnected_nodes_can_be_members() {
    let (mut p, f) = setup();
    let (a, d) = (named(&p, f, "a"), named(&p, f, "d"));
    let mut h = History::new();
    h.run(&mut p, |ed| {
        ed.disconnect(f, d, [400.0, 200.0])?;
        ed.create_group(f, "Main", &[a, d], "Mixed")
    })
    .unwrap();
    assert_eq!(group_members(&p, f, "Main")[&1], vec![a, d]);
}

#[test]
fn moved_nodes_keep_or_leave_their_group() {
    let (mut p, f) = setup();
    let (seq, a, b, fb, c, d, e) = (
        named(&p, f, "seq"),
        named(&p, f, "a"),
        named(&p, f, "b"),
        named(&p, f, "fb"),
        named(&p, f, "c"),
        named(&p, f, "d"),
        named(&p, f, "e"),
    );
    let mut h = History::new();
    h.run(&mut p, |ed| ed.create_group(f, "Main", &[a, b], "Pick"))
        .unwrap();

    // b moves into the Fallback: no neighbour is in the group, so it
    // leaves the group.
    h.run(&mut p, |ed| {
        ed.move_node(f, b, fb, 0)?;
        ed.keep_groups_in_place(f, &[b])
    })
    .unwrap();
    assert_eq!(group_of(&p, f, "b"), None);
    assert_eq!(group_of(&p, f, "a"), Some(1));

    // e moves next to a (a member): it stays out (it was not a member),
    // and a member that moves next to a member stays in.
    h.run(&mut p, |ed| {
        ed.set_group_members(f, "Main", 1, &[a, e])?;
        ed.move_node(f, e, seq, 1)?;
        ed.keep_groups_in_place(f, &[e])
    })
    .unwrap();
    assert_eq!(group_of(&p, f, "e"), Some(1));

    // A whole group moves: it stays.
    h.run(&mut p, |ed| {
        ed.create_group(f, "Main", &[c, d], "Inner")?;
        ed.move_node(f, fb, seq, 0)?;
        ed.keep_groups_in_place(f, &[fb])
    })
    .unwrap();
    assert_eq!(group_of(&p, f, "c"), Some(2));
    assert_eq!(group_of(&p, f, "d"), Some(2));

    // One member leaves. The last member is the whole group: the group
    // moves with it.
    h.run(&mut p, |ed| {
        ed.move_node(f, c, seq, 0)?;
        ed.keep_groups_in_place(f, &[c])
    })
    .unwrap();
    assert_eq!(group_of(&p, f, "c"), None);
    assert_eq!(group_of(&p, f, "d"), Some(2));
    h.run(&mut p, |ed| {
        ed.move_node(f, d, seq, 0)?;
        ed.keep_groups_in_place(f, &[d])
    })
    .unwrap();
    assert_eq!(group_of(&p, f, "d"), Some(2));

    // Group 1 (a, e): e moves into the Fallback, away from a. It leaves;
    // then a leaves with Remove from group, and the group goes away.
    h.run(&mut p, |ed| {
        ed.move_node(f, e, fb, 0)?;
        ed.keep_groups_in_place(f, &[e])
    })
    .unwrap();
    assert_eq!(group_of(&p, f, "e"), None);
    h.run(&mut p, |ed| ed.remove_from_group(f, &[a])).unwrap();
    assert!(groups(&p, f).iter().all(|g| g.id != 1));
}

#[test]
fn pasted_nodes_leave_groups_that_the_tree_does_not_have() {
    let (mut p, f) = setup();
    let (seq, a) = (named(&p, f, "seq"), named(&p, f, "a"));
    let mut h = History::new();
    h.run(&mut p, |ed| ed.create_group(f, "Main", &[a], "Pick"))
        .unwrap();
    // Next to a member of group 1: it joins. Group 5 is not defined.
    let pasted = h
        .run(&mut p, |ed| {
            let pasted = ed.paste(
                f,
                r#"<Sleep msec="1" _group="1"/><Sleep msec="2" _group="5"/>"#,
                seq,
                1,
            )?;
            ed.keep_groups_in_place(f, &pasted)?;
            Ok(pasted)
        })
        .unwrap();
    let doc = p.file(f).unwrap().doc();
    assert_eq!(node_group(doc, pasted[0]), Some(1));
    assert_eq!(node_group(doc, pasted[1]), None);
}

#[test]
fn canvas_notes() {
    let (mut p, f) = setup();
    let mut h = History::new();
    let (one, two) = h
        .run(&mut p, |ed| {
            let one = ed.add_canvas_note(f, "Main", [10.4, -20.0, 200.0, 80.0], "Check\nthis")?;
            let two = ed.add_canvas_note(f, "Main", [0.0, 0.0, 5.0, 5.0], "")?;
            Ok((one, two))
        })
        .unwrap();
    assert_eq!((one, two), (1, 2));
    let text = xml(&p, f);
    assert!(
        text.contains(r#"_note_1="Check&#10;this" _note_1_box="10 -20 200 80""#),
        "{text}"
    );
    // The smallest size.
    assert_eq!(notes(&p, f)[1].rect, [0.0, 0.0, 60.0, 32.0]);

    h.run(&mut p, |ed| {
        ed.set_canvas_note_text(f, "Main", 2, "Second")?;
        ed.set_canvas_note_box(f, "Main", 2, [50.0, 60.0, 300.0, 120.0])?;
        ed.set_canvas_note_color(f, "Main", 2, Some("blue"))
    })
    .unwrap();
    assert_eq!(
        notes(&p, f)[1],
        CanvasNoteInfo {
            id: 2,
            text: "Second".into(),
            rect: [50.0, 60.0, 300.0, 120.0],
            color: Some("blue".into()),
        }
    );
    assert!(matches!(
        h.run(&mut p, |ed| ed.set_canvas_note_box(
            f,
            "Main",
            2,
            [f32::NAN, 0.0, 1.0, 1.0]
        )),
        Err(CommandError::InvalidPosition(_))
    ));
    h.run(&mut p, |ed| ed.delete_canvas_note(f, "Main", 1))
        .unwrap();
    assert_eq!(notes(&p, f).iter().map(|n| n.id).collect::<Vec<_>>(), [2]);
    assert!(matches!(
        h.run(&mut p, |ed| ed.set_canvas_note_text(f, "Main", 1, "x")),
        Err(CommandError::UnknownNote { id: 1, .. })
    ));

    // The saved text reads back the same.
    let reread = BtFile::parse(&xml(&p, f), None).unwrap();
    let element = reread.tree("Main").unwrap().element;
    assert_eq!(tree_notes(reread.doc(), element), notes(&p, f));

    while h.can_undo() {
        h.undo(&mut p).unwrap();
    }
    assert_eq!(xml(&p, f), TREE);
}

#[test]
fn the_checks_accept_the_annotations() {
    let (mut p, f) = setup();
    let before = fasttick_core::validate::validate_project(&p);
    let (seq, a) = (named(&p, f, "seq"), named(&p, f, "a"));
    let mut h = History::new();
    h.run(&mut p, |ed| {
        ed.set_node_note(f, seq, Some("A note"))?;
        let id = ed.create_group(f, "Main", &[seq, a], "Group")?;
        ed.set_group_description(f, "Main", id, "Text")?;
        ed.set_group_color(f, "Main", id, Some("red"))?;
        ed.add_canvas_note(f, "Main", [0.0, 0.0, 100.0, 50.0], "Note")?;
        Ok(())
    })
    .unwrap();
    assert_eq!(fasttick_core::validate::validate_project(&p), before);
}
