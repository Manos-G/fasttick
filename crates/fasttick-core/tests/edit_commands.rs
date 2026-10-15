//! The edit commands: results, errors, and exact undo and redo.

mod edit_support;

use edit_support::{check, check_error, named, project, tag, text};
use fasttick_core::bt::{PostCondition, PreCondition, StructureHash};
use fasttick_core::builtin::ChildRule;
use fasttick_core::edit::{
    CommandError, Condition, NameKind, NodePlace, StructurePolicy, copy, node_place, parent_rule,
};
use fasttick_core::models::{NodeKind, NodeModel, PortDirection, PortModel};
use fasttick_core::project::{LayoutDirection, LayoutState};
use fasttick_core::xml::EditError;

const BASE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<root BTCPP_format="4" main_tree_to_execute="Main">
    <BehaviorTree ID="Main">
        <Sequence name="root">
            <SaySomething message="{greeting}"/>
            <Fallback name="door">
                <IsDoorOpen/>
                <OpenDoor door="{door}"/>
            </Fallback>
            <SubTree ID="Walk" target="{goal}"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Walk">
        <MoveTo goal='{target}'/>
    </BehaviorTree>
    <!-- Models -->
    <TreeNodesModel>
        <Action ID="SaySomething">
            <input_port name="message"/>
        </Action>
        <Condition ID="IsDoorOpen"/>
        <Action ID="OpenDoor">
            <input_port name="door"/>
        </Action>
        <Action ID="MoveTo">
            <input_port name="goal"/>
        </Action>
        <SubTree ID="Walk">
            <input_port name="target"/>
        </SubTree>
    </TreeNodesModel>
</root>
"#;

// ----- add_node -------------------------------------------------------------

#[test]
fn add_node_uses_the_compact_form_and_the_indentation() {
    let (mut p, f) = project(BASE);
    let fallback = named(&p, f, "door");
    let node = check(&mut p, |ed| {
        ed.add_node(f, fallback, 1, "AlwaysSuccess", None)
    });
    assert_eq!(
        text(&p, f),
        BASE.replace(
            "                <IsDoorOpen/>\n",
            "                <IsDoorOpen/>\n                <AlwaysSuccess/>\n"
        )
    );
    let doc = p.file(f).unwrap().doc();
    assert_eq!(doc.name(node), Some("AlwaysSuccess"));
    assert!(matches!(
        node_place(&p, f, node).unwrap(),
        NodePlace::Tree { tree: 0, ref tree_id } if tree_id == "Main"
    ));
}

#[test]
fn add_node_at_the_start_and_the_end() {
    let (mut p, f) = project(BASE);
    let sequence = named(&p, f, "root");
    check(&mut p, |ed| {
        ed.add_node(f, sequence, 0, "AlwaysFailure", None)?;
        ed.add_node(f, sequence, 4, "AlwaysSuccess", None)
    });
    let t = text(&p, f);
    assert!(t.contains(
        "        <Sequence name=\"root\">\n            <AlwaysFailure/>\n            <SaySomething"
    ));
    assert!(t.contains(
        "            <SubTree ID=\"Walk\" target=\"{goal}\"/>\n            <AlwaysSuccess/>\n        </Sequence>"
    ));
}

#[test]
fn add_node_for_a_subtree_and_for_the_explicit_form() {
    let (mut p, f) = project(BASE);
    let sequence = named(&p, f, "root");
    // The inferred model of the tree "Walk" (and the declared SubTree
    // model) give the SubTree form.
    let sub = check(&mut p, |ed| ed.add_node(f, sequence, 0, "Walk", None));
    let doc = p.file(f).unwrap().doc();
    assert_eq!(doc.name(sub), Some("SubTree"));
    assert_eq!(doc.attribute(sub, "ID"), Some("Walk"));

    // A type ID that is not an XML name uses the explicit form.
    let model = NodeModel::new("2Fast", NodeKind::Action);
    let fast = check(&mut p, |ed| {
        ed.add_node(f, sequence, 0, "2Fast", Some(&model))
    });
    let doc = p.file(f).unwrap().doc();
    assert_eq!(doc.name(fast), Some("Action"));
    assert_eq!(doc.attribute(fast, "ID"), Some("2Fast"));

    // A type ID that is a category tag too.
    let model = NodeModel::new("Condition", NodeKind::Condition);
    let cond = check(&mut p, |ed| {
        ed.add_node(f, sequence, 0, "Condition", Some(&model))
    });
    let doc = p.file(f).unwrap().doc();
    assert_eq!(doc.name(cond), Some("Condition"));
    assert_eq!(doc.attribute(cond, "ID"), Some("Condition"));

    // An unknown type with a valid name: compact form.
    let unknown = check(&mut p, |ed| {
        ed.add_node(f, sequence, 0, "MyCustomNode", None)
    });
    assert_eq!(p.file(f).unwrap().doc().name(unknown), Some("MyCustomNode"));
}

#[test]
fn add_node_errors() {
    let (mut p, f) = project(BASE);
    let sequence = named(&p, f, "root");
    let door = tag(&p, f, "IsDoorOpen");
    let fallback = named(&p, f, "door");
    let main = p.file(f).unwrap().tree("Main").unwrap().element;
    let section = tag(&p, f, "TreeNodesModel");

    let e = check_error(&mut p, |ed| ed.add_node(f, sequence, 0, "a b", None));
    assert!(matches!(
        e,
        CommandError::InvalidName {
            kind: NameKind::TypeId,
            ..
        }
    ));
    let e = check_error(&mut p, |ed| ed.add_node(f, sequence, 0, "Root", None));
    assert!(matches!(e, CommandError::InvalidName { .. }));
    // A condition cannot have children.
    let e = check_error(&mut p, |ed| ed.add_node(f, door, 0, "AlwaysSuccess", None));
    assert_eq!(
        e,
        CommandError::ChildLimit {
            parent: "IsDoorOpen".into(),
            rule: ChildRule::Exactly(0)
        }
    );
    // A tree has one root node.
    let e = check_error(&mut p, |ed| ed.add_node(f, main, 0, "AlwaysSuccess", None));
    assert_eq!(
        e,
        CommandError::ChildLimit {
            parent: "BehaviorTree".into(),
            rule: ChildRule::Exactly(1)
        }
    );
    let e = check_error(&mut p, |ed| {
        ed.add_node(f, fallback, 3, "AlwaysSuccess", None)
    });
    assert_eq!(
        e,
        CommandError::Xml(EditError::IndexOutOfRange { index: 3, len: 2 })
    );
    let e = check_error(&mut p, |ed| ed.add_node(f, sequence, 0, "2x", None));
    assert_eq!(e, CommandError::TypeNeedsKind("2x".into()));
    let e = check_error(&mut p, |ed| ed.add_node(f, sequence, 0, "SubTree", None));
    assert_eq!(e, CommandError::SubTreeNeedsTarget);
    let e = check_error(&mut p, |ed| ed.add_node(f, section, 0, "X", None));
    assert_eq!(e, CommandError::NotAParent(section));
    let e = check_error(&mut p, |ed| {
        ed.add_node(fasttick_core::project::FileId(9), sequence, 0, "X", None)
    });
    assert_eq!(
        e,
        CommandError::UnknownFile(fasttick_core::project::FileId(9))
    );
}

#[test]
fn allow_invalid_structure() {
    let (mut p, f) = project(BASE);
    let door = tag(&p, f, "IsDoorOpen");
    let main = p.file(f).unwrap().tree("Main").unwrap().element;
    check(&mut p, |ed| {
        ed.set_structure_policy(StructurePolicy::AllowInvalid);
        ed.add_node(f, door, 0, "AlwaysSuccess", None)?;
        ed.add_node(f, main, 1, "AlwaysFailure", None)
    });
    let t = text(&p, f);
    assert!(t.contains(
        "<IsDoorOpen>\n                    <AlwaysSuccess/>\n                </IsDoorOpen>"
    ));
    assert_eq!(
        p.file(f).unwrap().tree("Main").unwrap().extra_roots.len(),
        1
    );
}

#[test]
fn add_root_node_to_an_empty_tree() {
    let (mut p, f) = project(BASE);
    let tree = check(&mut p, |ed| ed.add_tree(f, "New"));
    let root = check(&mut p, |ed| ed.add_node(f, tree, 0, "Sequence", None));
    let child = check(&mut p, |ed| ed.add_node(f, root, 0, "AlwaysSuccess", None));
    assert!(text(&p, f).contains(
        "    <BehaviorTree ID=\"New\">\n        <Sequence>\n            <AlwaysSuccess/>\n        </Sequence>\n    </BehaviorTree>\n    <!-- Models -->"
    ));
    assert_eq!(p.file(f).unwrap().tree("New").unwrap().root, Some(root));
    assert!(matches!(
        node_place(&p, f, child),
        Ok(NodePlace::Tree { tree: 2, .. })
    ));
}

// ----- delete_nodes ---------------------------------------------------------

#[test]
fn delete_nodes_removes_subtrees_and_white_space() {
    let (mut p, f) = project(BASE);
    let fallback = named(&p, f, "door");
    let door = tag(&p, f, "IsDoorOpen");
    let say = tag(&p, f, "SaySomething");
    check(&mut p, |ed| ed.delete_nodes(f, &[door, fallback, say]));
    assert_eq!(
        text(&p, f),
        BASE.replace(
            "            <SaySomething message=\"{greeting}\"/>\n            <Fallback name=\"door\">\n                <IsDoorOpen/>\n                <OpenDoor door=\"{door}\"/>\n            </Fallback>\n",
            ""
        )
    );
}

#[test]
fn delete_nodes_errors() {
    let (mut p, f) = project(BASE);
    let main = p.file(f).unwrap().tree("Main").unwrap().element;
    let section = tag(&p, f, "TreeNodesModel");
    let root = p.file(f).unwrap().index().root.unwrap();
    assert_eq!(
        check_error(&mut p, |ed| ed.delete_nodes(f, &[])),
        CommandError::EmptySelection
    );
    for node in [main, section, root] {
        assert_eq!(
            check_error(&mut p, |ed| ed.delete_nodes(f, &[node])),
            CommandError::NotATreeNode(node)
        );
    }
    // A deleted node is detached; it is not a tree node any more.
    let say = tag(&p, f, "SaySomething");
    check(&mut p, |ed| ed.delete_nodes(f, &[say]));
    assert_eq!(
        check_error(&mut p, |ed| ed.delete_nodes(f, &[say])),
        CommandError::NotATreeNode(say)
    );
}

// ----- move_node ------------------------------------------------------------

#[test]
fn move_node_reorders_and_reparents() {
    let (mut p, f) = project(BASE);
    let sequence = named(&p, f, "root");
    let sub = tag(&p, f, "SubTree");
    check(&mut p, |ed| ed.move_node(f, sub, sequence, 0));
    assert_eq!(
        text(&p, f),
        BASE.replace(
            "            <SubTree ID=\"Walk\" target=\"{goal}\"/>\n",
            ""
        )
        .replace(
            "        <Sequence name=\"root\">\n",
            "        <Sequence name=\"root\">\n            <SubTree ID=\"Walk\" target=\"{goal}\"/>\n"
        )
    );

    // Out of the Fallback, one level up: the indentation changes.
    let (mut p, f) = project(BASE);
    let sequence = named(&p, f, "root");
    let door = tag(&p, f, "IsDoorOpen");
    check(&mut p, |ed| ed.move_node(f, door, sequence, 3));
    assert!(text(&p, f).contains(
        "            <SubTree ID=\"Walk\" target=\"{goal}\"/>\n            <IsDoorOpen/>\n        </Sequence>"
    ));

    // A subtree moves deeper: its inner white space is indented again.
    let (mut p, f) = project(BASE);
    let fallback = named(&p, f, "door");
    let walk_root = tag(&p, f, "MoveTo");
    let walk = p.file(f).unwrap().tree("Walk").unwrap().element;
    check(&mut p, |ed| {
        ed.set_structure_policy(StructurePolicy::AllowInvalid);
        ed.delete_nodes(f, &[walk_root])?;
        ed.move_node(f, fallback, walk, 0)
    });
    assert!(text(&p, f).contains(
        "    <BehaviorTree ID=\"Walk\">\n        <Fallback name=\"door\">\n            <IsDoorOpen/>\n            <OpenDoor door=\"{door}\"/>\n        </Fallback>\n    </BehaviorTree>"
    ));
}

#[test]
fn move_node_to_the_same_place_does_nothing() {
    let (mut p, f) = project(BASE);
    let sequence = named(&p, f, "root");
    let fallback = named(&p, f, "door");
    let mut history = fasttick_core::edit::History::new();
    history
        .run(&mut p, |ed| ed.move_node(f, fallback, sequence, 1))
        .unwrap();
    assert!(!history.can_undo());
    assert!(!p.file(f).unwrap().is_dirty());
    assert_eq!(text(&p, f), BASE);
}

#[test]
fn move_node_errors() {
    let (mut p, f) = project(BASE);
    let sequence = named(&p, f, "root");
    let fallback = named(&p, f, "door");
    let door = tag(&p, f, "IsDoorOpen");
    let say = tag(&p, f, "SaySomething");
    let main = p.file(f).unwrap().tree("Main").unwrap().element;
    assert_eq!(
        check_error(&mut p, |ed| ed.move_node(f, sequence, fallback, 0)),
        CommandError::IntoOwnSubtree(sequence)
    );
    assert_eq!(
        check_error(&mut p, |ed| ed.move_node(f, fallback, fallback, 0)),
        CommandError::IntoOwnSubtree(fallback)
    );
    assert_eq!(
        check_error(&mut p, |ed| ed.move_node(f, say, door, 0)),
        CommandError::ChildLimit {
            parent: "IsDoorOpen".into(),
            rule: ChildRule::Exactly(0)
        }
    );
    assert!(matches!(
        check_error(&mut p, |ed| ed.move_node(f, say, main, 0)),
        CommandError::ChildLimit { .. }
    ));
    assert_eq!(
        check_error(&mut p, |ed| ed.move_node(f, say, sequence, 3)),
        CommandError::Xml(EditError::IndexOutOfRange { index: 3, len: 2 })
    );
    assert_eq!(
        check_error(&mut p, |ed| ed.move_node(f, main, sequence, 0)),
        CommandError::NotATreeNode(main)
    );
}

// ----- attributes, names, ports, conditions ---------------------------------

#[test]
fn set_and_remove_attributes() {
    let (mut p, f) = project(BASE);
    let move_to = tag(&p, f, "MoveTo");
    // An existing attribute keeps its quotes.
    check(&mut p, |ed| ed.set_attribute(f, move_to, "goal", "{other}"));
    assert!(text(&p, f).contains("<MoveTo goal='{other}'/>"));
    // A new attribute copies the style of the last one.
    check(&mut p, |ed| ed.set_attribute(f, move_to, "speed", "1 < 2"));
    assert!(text(&p, f).contains("<MoveTo goal='{other}' speed='1 &lt; 2'/>"));
    check(&mut p, |ed| ed.remove_attribute(f, move_to, "goal"));
    assert!(text(&p, f).contains("<MoveTo speed='1 &lt; 2'/>"));
    // Nothing to remove: no step.
    let mut history = fasttick_core::edit::History::new();
    history
        .run(&mut p, |ed| ed.remove_attribute(f, move_to, "goal"))
        .unwrap();
    history
        .run(&mut p, |ed| ed.set_attribute(f, move_to, "speed", "1 < 2"))
        .unwrap();
    assert!(!history.can_undo());
}

#[test]
fn attribute_errors() {
    let (mut p, f) = project(BASE);
    let move_to = tag(&p, f, "MoveTo");
    assert_eq!(
        check_error(&mut p, |ed| ed.set_attribute(f, move_to, "1x", "v")),
        CommandError::InvalidAttributeName("1x".into())
    );
    assert!(matches!(
        check_error(&mut p, |ed| ed.set_attribute(f, move_to, "name", "a\u{1}b")),
        CommandError::InvalidName {
            kind: NameKind::InstanceName,
            ..
        }
    ));
    let sub = tag(&p, f, "SubTree");
    assert!(matches!(
        check_error(&mut p, |ed| ed.set_attribute(f, sub, "ID", "a/b")),
        CommandError::InvalidName {
            kind: NameKind::TreeId,
            ..
        }
    ));
    assert!(matches!(
        check_error(&mut p, |ed| ed.set_attribute(f, move_to, "x", "a\0b")),
        CommandError::Xml(EditError::InvalidContent(_))
    ));
    assert_eq!(
        check_error(&mut p, |ed| ed.set_port_value(
            f,
            move_to,
            "_private",
            Some("1")
        )),
        CommandError::NotAPortName("_private".into())
    );
    assert_eq!(
        check_error(&mut p, |ed| ed.set_port_value(
            f,
            move_to,
            "name",
            Some("1")
        )),
        CommandError::NotAPortName("name".into())
    );
    let section = tag(&p, f, "TreeNodesModel");
    assert_eq!(
        check_error(&mut p, |ed| ed.set_instance_name(f, section, Some("x"))),
        CommandError::NotATreeNode(section)
    );
}

#[test]
fn instance_names_ports_and_conditions() {
    let (mut p, f) = project(BASE);
    let move_to = tag(&p, f, "MoveTo");
    check(&mut p, |ed| {
        ed.set_instance_name(f, move_to, Some("go home"))
    });
    assert!(text(&p, f).contains("<MoveTo goal='{target}' name='go home'/>"));
    check(&mut p, |ed| ed.set_instance_name(f, move_to, None));
    assert!(text(&p, f).contains("<MoveTo goal='{target}'/>"));
    // An empty name is valid in BT.CPP.
    check(&mut p, |ed| ed.set_instance_name(f, move_to, Some("")));
    assert!(text(&p, f).contains("<MoveTo goal='{target}' name=''/>"));

    check(&mut p, |ed| {
        ed.set_port_value(f, move_to, "goal", Some("{=}"))?;
        ed.set_port_value(f, move_to, "speed", Some("2.5"))
    });
    assert!(text(&p, f).contains("<MoveTo goal='{=}' name='' speed='2.5'/>"));
    check(&mut p, |ed| ed.set_port_value(f, move_to, "speed", None));
    assert!(text(&p, f).contains("<MoveTo goal='{=}' name=''/>"));

    check(&mut p, |ed| {
        ed.set_condition(f, move_to, PreCondition::SkipIf, Some("done == true"))?;
        ed.set_condition(
            f,
            move_to,
            Condition::Post(PostCondition::OnSuccess),
            Some("done := true"),
        )
    });
    assert!(
        text(&p, f).contains(
            "<MoveTo goal='{=}' name='' _skipIf='done == true' _onSuccess='done := true'/>"
        )
    );
    check(&mut p, |ed| {
        ed.set_condition(f, move_to, PreCondition::SkipIf, None)
    });
    assert!(text(&p, f).contains("<MoveTo goal='{=}' name='' _onSuccess='done := true'/>"));
    assert_eq!(Condition::from(PostCondition::Post).to_string(), "_post");
}

// ----- change_node_type -----------------------------------------------------

#[test]
fn change_type_of_compact_nodes() {
    let (mut p, f) = project(BASE);
    let fallback = named(&p, f, "door");
    check(&mut p, |ed| {
        ed.change_node_type(f, fallback, "ReactiveFallback", None)
    });
    assert_eq!(
        text(&p, f),
        BASE.replace(
            "<Fallback name=\"door\">",
            "<ReactiveFallback name=\"door\">"
        )
        .replace("</Fallback>", "</ReactiveFallback>")
    );
    // To a SubTree and back.
    let move_to = tag(&p, f, "MoveTo");
    let model = NodeModel::new("Patrol", NodeKind::SubTree);
    check(&mut p, |ed| {
        ed.change_node_type(f, move_to, "Patrol", Some(&model))
    });
    assert!(text(&p, f).contains("<SubTree ID=\"Patrol\" goal='{target}'/>"));
    check(&mut p, |ed| ed.change_node_type(f, move_to, "MoveTo", None));
    assert!(text(&p, f).contains("<MoveTo goal='{target}'/>"));
    // A SubTree node gets another tree.
    let sub = tag(&p, f, "SubTree");
    check(&mut p, |ed| ed.change_node_type(f, sub, "Main", None));
    assert!(text(&p, f).contains("<SubTree ID=\"Main\" target=\"{goal}\"/>"));
}

#[test]
fn change_type_of_explicit_nodes() {
    let text_in = r#"<root BTCPP_format="4">
  <BehaviorTree ID="T">
    <Control ID="Sequence" name="s">
      <Action ID="Old" name="n" x="1"/>
      <X ID="stray"/>
    </Control>
  </BehaviorTree>
</root>"#;
    let (mut p, f) = project(text_in);
    let old = named(&p, f, "n");
    let model = NodeModel::new("Check", NodeKind::Condition);
    check(&mut p, |ed| {
        ed.change_node_type(f, old, "Check", Some(&model))
    });
    assert!(text(&p, f).contains("<Condition ID=\"Check\" name=\"n\" x=\"1\"/>"));
    // Unknown kind: the tag stays.
    check(&mut p, |ed| ed.change_node_type(f, old, "Unknown", None));
    assert!(text(&p, f).contains("<Condition ID=\"Unknown\" name=\"n\" x=\"1\"/>"));
    // A compact node with a stray ID attribute keeps it; the SubTree form
    // sets it.
    let stray = tag(&p, f, "X");
    check(&mut p, |ed| ed.change_node_type(f, stray, "Y", None));
    assert!(text(&p, f).contains("<Y ID=\"stray\"/>"));
    let model = NodeModel::new("T2", NodeKind::SubTree);
    check(&mut p, |ed| {
        ed.change_node_type(f, stray, "T2", Some(&model))
    });
    assert!(text(&p, f).contains("<SubTree ID=\"T2\"/>"));
    // A compact node to the explicit form (the type is not an XML name).
    let model = NodeModel::new("3D", NodeKind::Action);
    check(&mut p, |ed| {
        ed.change_node_type(f, stray, "3D", Some(&model))
    });
    assert!(text(&p, f).contains("<Action ID=\"3D\"/>"));
    // Explicit Control to another control.
    let seq = named(&p, f, "s");
    check(&mut p, |ed| ed.change_node_type(f, seq, "Fallback", None));
    assert!(text(&p, f).contains("<Control ID=\"Fallback\" name=\"s\">"));
}

#[test]
fn change_type_checks_the_children() {
    let (mut p, f) = project(BASE);
    let sequence = named(&p, f, "root");
    assert_eq!(
        check_error(&mut p, |ed| ed
            .change_node_type(f, sequence, "Inverter", None)),
        CommandError::TooManyChildren {
            type_id: "Inverter".into(),
            rule: ChildRule::Exactly(1),
            count: 3
        }
    );
    assert!(matches!(
        check_error(&mut p, |ed| ed.change_node_type(f, sequence, "Walk", None)),
        CommandError::TooManyChildren { .. }
    ));
    assert!(matches!(
        check_error(&mut p, |ed| ed
            .change_node_type(f, sequence, "Bad Type", None)),
        CommandError::InvalidName { .. }
    ));
    check(&mut p, |ed| {
        ed.set_structure_policy(StructurePolicy::AllowInvalid);
        ed.change_node_type(f, sequence, "Inverter", None)
    });
    assert!(text(&p, f).contains("<Inverter name=\"root\">"));
}

// ----- copy, paste, duplicate ------------------------------------------------

#[test]
fn copy_writes_a_fragment_without_indentation() {
    let (p, f) = project(BASE);
    let fallback = named(&p, f, "door");
    let door = tag(&p, f, "IsDoorOpen");
    let say = tag(&p, f, "SaySomething");
    assert_eq!(
        copy(&p, f, &[door, fallback]).unwrap(),
        "<Fallback name=\"door\">\n    <IsDoorOpen/>\n    <OpenDoor door=\"{door}\"/>\n</Fallback>"
    );
    // Document order, not selection order.
    assert_eq!(
        copy(&p, f, &[fallback, say]).unwrap(),
        "<SaySomething message=\"{greeting}\"/>\n<Fallback name=\"door\">\n    <IsDoorOpen/>\n    <OpenDoor door=\"{door}\"/>\n</Fallback>"
    );
    assert_eq!(copy(&p, f, &[]), Err(CommandError::EmptySelection));
    let section = tag(&p, f, "TreeNodesModel");
    assert_eq!(
        copy(&p, f, &[section]),
        Err(CommandError::NotATreeNode(section))
    );
}

#[test]
fn copy_keeps_tabs_and_crlf_of_the_file() {
    let source = "<root>\r\n\t<BehaviorTree ID=\"T\">\r\n\t\t<Sequence>\r\n\t\t\t<A/>\r\n\t\t</Sequence>\r\n\t</BehaviorTree>\r\n</root>\r\n";
    let (p, f) = project(source);
    let sequence = tag(&p, f, "Sequence");
    let a = tag(&p, f, "A");
    assert_eq!(
        copy(&p, f, &[sequence]).unwrap(),
        "<Sequence>\r\n\t<A/>\r\n</Sequence>"
    );
    assert_eq!(
        copy(&p, f, &[a, sequence, a]).unwrap(),
        "<Sequence>\r\n\t<A/>\r\n</Sequence>"
    );

    // Two nodes: the line end between them is CR LF too, and the text
    // pastes again.
    let (mut p, f) = project(&source.replace("<A/>", "<A/>\r\n\t\t\t<B/>"));
    let sequence = tag(&p, f, "Sequence");
    let a = tag(&p, f, "A");
    let b = tag(&p, f, "B");
    let clip = copy(&p, f, &[b, a]).unwrap();
    assert_eq!(clip, "<A/>\r\n<B/>");
    let pasted = check(&mut p, |ed| ed.paste(f, &clip, sequence, 0));
    assert_eq!(pasted.len(), 2);
    assert!(text(&p, f).contains(
        "<Sequence>\r\n\t\t\t<A/>\r\n\t\t\t<B/>\r\n\t\t\t<A/>\r\n\t\t\t<B/>\r\n\t\t</Sequence>"
    ));
}

#[test]
fn paste_indents_the_nodes() {
    let (mut p, f) = project(BASE);
    let fallback = named(&p, f, "door");
    let sequence = named(&p, f, "root");
    let clip = copy(&p, f, &[fallback]).unwrap();
    let pasted = check(&mut p, |ed| ed.paste(f, &clip, sequence, 3));
    assert_eq!(pasted.len(), 1);
    assert!(text(&p, f).contains(
        "            <SubTree ID=\"Walk\" target=\"{goal}\"/>\n            <Fallback name=\"door\">\n                <IsDoorOpen/>\n                <OpenDoor door=\"{door}\"/>\n            </Fallback>\n        </Sequence>"
    ));
    // Two elements, with a comment between them (dropped), at the top of
    // the Fallback.
    let pasted = check(&mut p, |ed| {
        ed.paste(
            f,
            "<A/>\n<!-- c -->\n<B x=\"1\">\n  <C/>\n</B>",
            fallback,
            0,
        )
    });
    assert_eq!(pasted.len(), 2);
    assert!(text(&p, f).contains(
        "            <Fallback name=\"door\">\n                <A/>\n                <B x=\"1\">\n                    <C/>\n                </B>\n                <IsDoorOpen/>"
    ));
}

#[test]
fn paste_errors() {
    let (mut p, f) = project(BASE);
    let sequence = named(&p, f, "root");
    let door = tag(&p, f, "IsDoorOpen");
    assert!(matches!(
        check_error(&mut p, |ed| ed.paste(f, "<A>", sequence, 0)),
        CommandError::Clipboard(_)
    ));
    assert!(matches!(
        check_error(&mut p, |ed| ed.paste(f, "  ", sequence, 0)),
        CommandError::Clipboard(_)
    ));
    // The XML parser reports a fragment without an element.
    let e = check_error(&mut p, |ed| ed.paste(f, "<!-- only -->", sequence, 0));
    assert!(e.to_string().contains("no element"), "{e}");
    assert_eq!(
        check_error(&mut p, |ed| ed.paste(
            f,
            "<BehaviorTree ID=\"X\"/>",
            sequence,
            0
        )),
        CommandError::NotPastable("BehaviorTree".into())
    );
    assert_eq!(
        check_error(&mut p, |ed| ed.paste(
            f,
            "<A/><include path=\"x\"/>",
            sequence,
            0
        )),
        CommandError::NotPastable("include".into())
    );
    assert!(matches!(
        check_error(&mut p, |ed| ed.paste(f, "<A/>", door, 0)),
        CommandError::ChildLimit { .. }
    ));
    let fallback = named(&p, f, "door");
    assert!(matches!(
        check_error(&mut p, |ed| ed.paste(f, "<A/>", fallback, 5)),
        CommandError::Xml(EditError::IndexOutOfRange { .. })
    ));
}

#[test]
fn duplicate_inserts_copies_after_the_originals() {
    let (mut p, f) = project(BASE);
    let say = tag(&p, f, "SaySomething");
    let door = tag(&p, f, "IsDoorOpen");
    let fallback = named(&p, f, "door");
    let copies = check(&mut p, |ed| ed.duplicate(f, &[door, say, fallback]));
    assert_eq!(copies.len(), 2);
    let t = text(&p, f);
    assert!(t.contains(
        "            <SaySomething message=\"{greeting}\"/>\n            <SaySomething message=\"{greeting}\"/>\n            <Fallback name=\"door\">"
    ));
    assert_eq!(t.matches("<Fallback name=\"door\">").count(), 2);
    let doc = p.file(f).unwrap().doc();
    assert_eq!(doc.name(copies[0]), Some("SaySomething"));
    assert_ne!(copies[0], say);
    // Errors.
    let main = p.file(f).unwrap().tree("Main").unwrap().root.unwrap();
    assert!(matches!(
        check_error(&mut p, |ed| ed.duplicate(f, &[main])),
        CommandError::ChildLimit { .. }
    ));
    assert_eq!(
        check_error(&mut p, |ed| ed.duplicate(f, &[])),
        CommandError::EmptySelection
    );
}

// ----- unconnected nodes ----------------------------------------------------

#[test]
fn disconnect_and_connect() {
    let (mut p, f) = project(BASE);
    let fallback = named(&p, f, "door");
    let sequence = named(&p, f, "root");
    let door = tag(&p, f, "IsDoorOpen");
    check(&mut p, |ed| ed.disconnect(f, fallback, [100.0, 50.0]));
    assert!(!text(&p, f).contains("Fallback"));
    assert_eq!(p.unconnected(f, "Main").len(), 1);
    assert_eq!(p.unconnected(f, "Main")[0].node, fallback);
    assert_eq!(p.unconnected(f, "Main")[0].position, [100.0, 50.0]);
    assert!(matches!(
        node_place(&p, f, door).unwrap(),
        NodePlace::Unconnected { root, .. } if root == fallback
    ));

    // Errors while the node is unconnected.
    assert_eq!(
        check_error(&mut p, |ed| ed.disconnect(f, fallback, [0.0, 0.0])),
        CommandError::AlreadyUnconnected(fallback)
    );
    assert_eq!(
        check_error(&mut p, |ed| ed.connect(f, fallback, door, 0)),
        CommandError::IntoOwnSubtree(fallback)
    );
    assert_eq!(
        check_error(&mut p, |ed| ed.connect(f, door, sequence, 0)),
        CommandError::NotUnconnected(door)
    );
    assert!(matches!(
        check_error(&mut p, |ed| ed.disconnect(f, door, [f32::NAN, 0.0])),
        CommandError::InvalidPosition(_)
    ));

    // A node in the unconnected subtree can get a new child.
    let added = check(&mut p, |ed| {
        ed.add_node(f, fallback, 2, "AlwaysFailure", None)
    });
    assert!(matches!(
        node_place(&p, f, added),
        Ok(NodePlace::Unconnected { .. })
    ));

    // Connect at the old place: the text is the original plus the new node.
    check(&mut p, |ed| ed.connect(f, fallback, sequence, 1));
    assert_eq!(
        text(&p, f),
        BASE.replace(
            "                <OpenDoor door=\"{door}\"/>\n",
            "                <OpenDoor door=\"{door}\"/>\n                <AlwaysFailure/>\n"
        )
    );
    assert!(p.unconnected(f, "Main").is_empty());
}

#[test]
fn disconnect_then_connect_restores_the_bytes() {
    let (mut p, f) = project(BASE);
    let fallback = named(&p, f, "door");
    let sequence = named(&p, f, "root");
    check(&mut p, |ed| {
        ed.disconnect(f, fallback, [1.0, 2.0])?;
        ed.connect(f, fallback, sequence, 1)
    });
    assert_eq!(text(&p, f), BASE);
    // A move of an unconnected node is a connect.
    check(&mut p, |ed| ed.disconnect(f, fallback, [1.0, 2.0]));
    check(&mut p, |ed| ed.move_node(f, fallback, sequence, 1));
    assert_eq!(text(&p, f), BASE);
}

#[test]
fn unconnected_nodes_add_move_delete_duplicate_paste() {
    let (mut p, f) = project(BASE);
    let a = check(&mut p, |ed| {
        ed.add_unconnected_node(f, "Walk", "AlwaysSuccess", None, [10.0, 20.0])
    });
    let b = check(&mut p, |ed| {
        ed.add_unconnected_node(f, "Walk", "Sequence", None, [30.0, 40.0])
    });
    assert_eq!(text(&p, f), BASE);
    let list: Vec<_> = p.unconnected(f, "Walk").iter().map(|n| n.node).collect();
    assert_eq!(list, vec![a, b]);

    check(&mut p, |ed| ed.set_unconnected_position(f, a, [11.0, 21.0]));
    assert_eq!(p.unconnected(f, "Walk")[0].position, [11.0, 21.0]);
    assert_eq!(
        check_error(&mut p, |ed| ed.set_unconnected_position(
            f,
            a,
            [f32::INFINITY, 0.0]
        )),
        CommandError::InvalidPosition([f32::INFINITY, 0.0])
    );

    // Delete the first: the order of the rest stays, and undo puts it back
    // at the same index (check() compares the lists).
    check(&mut p, |ed| ed.delete_nodes(f, &[a]));
    assert_eq!(p.unconnected(f, "Walk").len(), 1);

    let copies = check(&mut p, |ed| ed.duplicate(f, &[b]));
    assert_eq!(p.unconnected(f, "Walk")[1].node, copies[0]);
    assert_eq!(p.unconnected(f, "Walk")[1].position, [54.0, 64.0]);

    let pasted = check(&mut p, |ed| {
        ed.paste_unconnected(f, "Walk", "<X/><Y/>", [0.0, 0.0])
    });
    assert_eq!(pasted.len(), 2);
    assert_eq!(p.unconnected(f, "Walk")[3].position, [24.0, 24.0]);

    assert_eq!(
        check_error(&mut p, |ed| ed.add_unconnected_node(
            f,
            "Nope",
            "A",
            None,
            [0.0, 0.0]
        )),
        CommandError::TreeNotFound("Nope".into())
    );
    let move_to = tag(&p, f, "MoveTo");
    assert_eq!(
        check_error(&mut p, |ed| ed.set_unconnected_position(
            f,
            move_to,
            [0.0, 0.0]
        )),
        CommandError::NotUnconnected(move_to)
    );
    assert_eq!(text(&p, f), BASE);
}

// ----- trees -----------------------------------------------------------------

#[test]
fn add_tree_goes_before_the_models() {
    let (mut p, f) = project(BASE);
    let tree = check(&mut p, |ed| ed.add_tree(f, "New"));
    assert_eq!(
        text(&p, f),
        BASE.replace(
            "    <!-- Models -->\n",
            "    <BehaviorTree ID=\"New\"/>\n    <!-- Models -->\n"
        )
    );
    assert_eq!(p.file(f).unwrap().tree("New").unwrap().element, tree);
    // Without a model section: at the end.
    let (mut p, f) = project("<root BTCPP_format=\"4\">\n  <BehaviorTree ID=\"A\"/>\n</root>\n");
    check(&mut p, |ed| ed.add_tree(f, "B"));
    assert_eq!(
        text(&p, f),
        "<root BTCPP_format=\"4\">\n  <BehaviorTree ID=\"A\"/>\n  <BehaviorTree ID=\"B\"/>\n</root>\n"
    );
    // An empty file.
    let (mut p, f) = project(fasttick_core::bt::NEW_FILE_TEXT);
    check(&mut p, |ed| ed.add_tree(f, "Main"));
    assert_eq!(
        text(&p, f),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<root BTCPP_format=\"4\">\n    <BehaviorTree ID=\"Main\"/>\n</root>\n"
    );
}

#[test]
fn add_tree_errors() {
    let (mut p, f) = project(BASE);
    assert_eq!(
        check_error(&mut p, |ed| ed.add_tree(f, "Walk")),
        CommandError::TreeExists("Walk".into())
    );
    for bad in ["", "root", "a b", "a.b"] {
        assert!(matches!(
            check_error(&mut p, |ed| ed.add_tree(f, bad)),
            CommandError::InvalidName {
                kind: NameKind::TreeId,
                ..
            }
        ));
    }
    let (mut p, f) = project("<TreeNodesModel><Action ID=\"A\"/></TreeNodesModel>");
    assert_eq!(
        check_error(&mut p, |ed| ed.add_tree(f, "T")),
        CommandError::NotATreeFile
    );
    let (mut p, f) = project("<!-- no root -->");
    assert_eq!(
        check_error(&mut p, |ed| ed.add_tree(f, "T")),
        CommandError::NoRootElement
    );
}

#[test]
fn delete_tree_removes_its_state() {
    let (mut p, f) = project(BASE);
    let mut layout = LayoutState::new(LayoutDirection::TopDown, StructureHash(1));
    layout.set_position(&[0], [1.0, 2.0]);
    p.set_layout(f, "Walk", layout);
    check(&mut p, |ed| {
        ed.add_unconnected_node(f, "Walk", "AlwaysSuccess", None, [0.0, 0.0])
    });
    check(&mut p, |ed| ed.delete_tree(f, "Walk"));
    assert!(!text(&p, f).contains("<BehaviorTree ID=\"Walk\">"));
    assert!(p.layout(f, "Walk").is_none());
    assert!(p.unconnected(f, "Walk").is_empty());
    assert_eq!(
        check_error(&mut p, |ed| ed.delete_tree(f, "Walk")),
        CommandError::TreeNotFound("Walk".into())
    );
}

#[test]
fn rename_tree_updates_references_and_state() {
    let (mut p, f) = project(BASE);
    let mut layout = LayoutState::new(LayoutDirection::LeftRight, StructureHash(7));
    layout.set_position(&[0], [5.0, 6.0]);
    p.set_layout(f, "Walk", layout.clone());
    let node = check(&mut p, |ed| {
        ed.add_unconnected_node(f, "Walk", "Walk", None, [0.0, 0.0])
    });
    let count = check(&mut p, |ed| ed.rename_tree(f, "Walk", "Stroll"));
    // The SubTree node, the SubTree model and the unconnected SubTree node.
    assert_eq!(count, 3);
    assert_eq!(text(&p, f), BASE.replace("ID=\"Walk\"", "ID=\"Stroll\""));
    assert_eq!(p.layout(f, "Stroll"), Some(&layout));
    assert!(p.layout(f, "Walk").is_none());
    assert_eq!(p.unconnected(f, "Stroll")[0].node, node);
    assert_eq!(
        p.file(f).unwrap().doc().attribute(node, "ID"),
        Some("Stroll")
    );

    // The main tree.
    let count = check(&mut p, |ed| ed.rename_tree(f, "Main", "Start"));
    assert_eq!(count, 1);
    assert!(text(&p, f).contains("main_tree_to_execute=\"Start\""));
    assert!(text(&p, f).contains("<BehaviorTree ID=\"Start\">"));
    assert_eq!(check(&mut p, |ed| ed.rename_tree(f, "Start", "Start")), 0);
}

#[test]
fn rename_tree_over_stale_state() {
    let (mut p, f) = project(BASE);
    let walk = LayoutState::new(LayoutDirection::TopDown, StructureHash(1));
    let stale = LayoutState::new(LayoutDirection::LeftRight, StructureHash(2));
    p.set_layout(f, "Walk", walk.clone());
    p.set_layout(f, "Stroll", stale);
    let old_node = check(&mut p, |ed| {
        ed.add_unconnected_node(f, "Walk", "AlwaysSuccess", None, [0.0, 0.0])
    });
    // A stale unconnected node of a tree "Stroll" that does not exist.
    let tree = p.file(f).unwrap().tree("Main").unwrap().element;
    let main_node = check(&mut p, |ed| {
        ed.add_unconnected_node(f, "Main", "AlwaysFailure", None, [1.0, 1.0])
    });
    check(&mut p, |ed| ed.rename_tree(f, "Main", "Stroll"));
    check(&mut p, |ed| ed.rename_tree(f, "Stroll", "Main"));
    assert_eq!(p.file(f).unwrap().tree("Main").unwrap().element, tree);
    assert_eq!(p.unconnected(f, "Main")[0].node, main_node);
    // Now the real test: "Walk" goes over the stale state of "Stroll".
    check(&mut p, |ed| ed.rename_tree(f, "Walk", "Stroll"));
    assert_eq!(p.layout(f, "Stroll"), Some(&walk));
    assert_eq!(p.unconnected(f, "Stroll")[0].node, old_node);
}

#[test]
fn parent_rules_for_drop_targets() {
    let (p, f) = project(BASE);
    let main = p.file(f).unwrap().tree("Main").unwrap().element;
    let sequence = named(&p, f, "root");
    let door = tag(&p, f, "IsDoorOpen");
    let sub = tag(&p, f, "SubTree");
    assert_eq!(
        parent_rule(&p, f, main).unwrap(),
        ("BehaviorTree".to_owned(), Some(ChildRule::Exactly(1)))
    );
    assert_eq!(
        parent_rule(&p, f, sequence).unwrap(),
        ("Sequence".to_owned(), Some(ChildRule::AtLeast(1)))
    );
    assert_eq!(
        parent_rule(&p, f, door).unwrap(),
        ("IsDoorOpen".to_owned(), Some(ChildRule::Exactly(0)))
    );
    // A SubTree uses the rule of "SubTree", not of its tree ID.
    assert_eq!(
        parent_rule(&p, f, sub).unwrap(),
        ("SubTree".to_owned(), Some(ChildRule::Exactly(0)))
    );
    let (p, f) = project("<root><BehaviorTree ID=\"T\"><Custom/></BehaviorTree></root>");
    let custom = tag(&p, f, "Custom");
    assert_eq!(
        parent_rule(&p, f, custom).unwrap(),
        ("Custom".to_owned(), None)
    );
}

#[test]
fn rename_tree_errors() {
    let (mut p, f) = project(BASE);
    assert_eq!(
        check_error(&mut p, |ed| ed.rename_tree(f, "Walk", "Main")),
        CommandError::TreeExists("Main".into())
    );
    assert_eq!(
        check_error(&mut p, |ed| ed.rename_tree(f, "Nope", "X")),
        CommandError::TreeNotFound("Nope".into())
    );
    assert!(matches!(
        check_error(&mut p, |ed| ed.rename_tree(f, "Walk", "a:b")),
        CommandError::InvalidName { .. }
    ));
}

#[test]
fn rename_a_tree_without_id() {
    let (mut p, f) = project(
        "<root BTCPP_format=\"4\">\n  <BehaviorTree>\n    <A/>\n  </BehaviorTree>\n</root>\n",
    );
    check(&mut p, |ed| ed.rename_tree(f, "BehaviorTree_0", "Named"));
    assert_eq!(
        text(&p, f),
        "<root BTCPP_format=\"4\">\n  <BehaviorTree ID=\"Named\">\n    <A/>\n  </BehaviorTree>\n</root>\n"
    );
}

#[test]
fn set_main_tree() {
    let (mut p, f) = project(BASE);
    check(&mut p, |ed| ed.set_main_tree(f, Some("Walk")));
    assert!(text(&p, f).contains("<root BTCPP_format=\"4\" main_tree_to_execute=\"Walk\">"));
    check(&mut p, |ed| ed.set_main_tree(f, None));
    assert!(text(&p, f).contains("<root BTCPP_format=\"4\">"));
    check(&mut p, |ed| ed.set_main_tree(f, Some("Main")));
    assert_eq!(
        check_error(&mut p, |ed| ed.set_main_tree(f, Some("Nope"))),
        CommandError::TreeNotFound("Nope".into())
    );
}

#[test]
fn set_layout_is_undoable() {
    let (mut p, f) = project(BASE);
    let mut layout = LayoutState::new(LayoutDirection::TopDown, StructureHash(3));
    layout.set_position(&[0, 1], [3.0, 4.0]);
    check(&mut p, |ed| ed.set_layout(f, "Main", Some(layout.clone())));
    assert_eq!(p.layout(f, "Main"), Some(&layout));
    check(&mut p, |ed| ed.set_layout(f, "Main", None));
    assert!(p.layout(f, "Main").is_none());
}

// ----- create_subtree_from_selection ----------------------------------------

#[test]
fn create_subtree_from_several_siblings() {
    let (mut p, f) = project(BASE);
    let say = tag(&p, f, "SaySomething");
    let fallback = named(&p, f, "door");
    let result = check(&mut p, |ed| {
        ed.create_subtree_from_selection(f, &[fallback, say], "Greet")
    });
    assert_eq!(result.remapped_keys, vec!["greeting", "door"]);
    assert!(!result.autoremap);
    assert!(result.sequence.is_some());
    let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<root BTCPP_format="4" main_tree_to_execute="Main">
    <BehaviorTree ID="Main">
        <Sequence name="root">
            <SubTree ID="Greet" greeting="{greeting}" door="{door}"/>
            <SubTree ID="Walk" target="{goal}"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Walk">
        <MoveTo goal='{target}'/>
    </BehaviorTree>
    <BehaviorTree ID="Greet">
        <Sequence>
            <SaySomething message="{greeting}"/>
            <Fallback name="door">
                <IsDoorOpen/>
                <OpenDoor door="{door}"/>
            </Fallback>
        </Sequence>
    </BehaviorTree>
    <!-- Models -->
    <TreeNodesModel>
        <Action ID="SaySomething">
            <input_port name="message"/>
        </Action>
        <Condition ID="IsDoorOpen"/>
        <Action ID="OpenDoor">
            <input_port name="door"/>
        </Action>
        <Action ID="MoveTo">
            <input_port name="goal"/>
        </Action>
        <SubTree ID="Walk">
            <input_port name="target"/>
        </SubTree>
        <SubTree ID="Greet">
            <inout_port name="greeting"/>
            <inout_port name="door"/>
        </SubTree>
    </TreeNodesModel>
</root>
"#;
    assert_eq!(text(&p, f), expected);
    // The project sees the new tree and its model.
    let (model, _) = p.lookup_model(f, "Greet").unwrap();
    assert_eq!(model.kind, NodeKind::SubTree);
    assert_eq!(model.ports.len(), 2);
    assert_eq!(model.ports[0].direction, PortDirection::InOut);
}

#[test]
fn create_subtree_from_one_node_with_special_keys() {
    let source = r#"<root BTCPP_format="4">
    <BehaviorTree ID="Main">
        <Sequence>
            <Fallback _skipIf="skip">
                <Check value="{@global}" other="{ spaced }"/>
                <Act out="{=}" in="literal" with="{key}"/>
                <SubTree ID="Inner" x="{=}" y="=" z="{key}"/>
                <Script code="a := 1"/>
            </Fallback>
            <AlwaysSuccess/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Inner"><AlwaysSuccess/></BehaviorTree>
</root>
"#;
    let (mut p, f) = project(source);
    let fallback = tag(&p, f, "Fallback");
    // " spaced " is not a valid port name: refused.
    assert_eq!(
        check_error(&mut p, |ed| ed.create_subtree_from_selection(
            f,
            &[fallback],
            "Part"
        )),
        CommandError::KeysNotRemappable(vec![" spaced ".into()])
    );
    let check_node = tag(&p, f, "Check");
    check(&mut p, |ed| {
        ed.set_port_value(f, check_node, "other", Some("{spaced}"))
    });
    let result = check(&mut p, |ed| {
        ed.create_subtree_from_selection(f, &[fallback], "Part")
    });
    assert_eq!(result.remapped_keys, vec!["spaced", "out", "key", "x"]);
    assert_eq!(result.sequence, None);
    assert_eq!(result.script_nodes.len(), 2);
    let t = text(&p, f);
    assert!(t.contains(
        "        <Sequence>\n            <SubTree ID=\"Part\" spaced=\"{spaced}\" out=\"{out}\" key=\"{key}\" x=\"{x}\"/>\n            <AlwaysSuccess/>"
    ));
    assert!(t.contains(
        "    <BehaviorTree ID=\"Part\">\n        <Fallback _skipIf=\"skip\">\n            <Check value=\"{@global}\" other=\"{spaced}\"/>"
    ));
    // No model section before: one is made at the end.
    assert!(t.ends_with(
        "    <TreeNodesModel>\n        <SubTree ID=\"Part\">\n            <inout_port name=\"spaced\"/>\n            <inout_port name=\"out\"/>\n            <inout_port name=\"key\"/>\n            <inout_port name=\"x\"/>\n        </SubTree>\n    </TreeNodesModel>\n</root>\n"
    ));
}

#[test]
fn create_subtree_with_autoremap_and_without_keys() {
    let source = r#"<root BTCPP_format="4">
  <BehaviorTree ID="Main">
    <Sequence>
      <SubTree ID="Inner" _autoremap="true" a="{my.key}"/>
      <AlwaysSuccess/>
    </Sequence>
  </BehaviorTree>
  <BehaviorTree ID="Inner"><AlwaysSuccess/></BehaviorTree>
</root>
"#;
    let (mut p, f) = project(source);
    let inner = tag(&p, f, "SubTree");
    let result = check(&mut p, |ed| {
        ed.create_subtree_from_selection(f, &[inner], "Outer")
    });
    assert!(result.autoremap);
    assert!(result.remapped_keys.is_empty());
    let t = text(&p, f);
    assert!(t.contains("<SubTree ID=\"Outer\" _autoremap=\"true\"/>"));
    // No keys: no model.
    assert!(!t.contains("TreeNodesModel"));

    // A private key cannot be remapped, also with _autoremap.
    let (mut p, f) = project(&source.replace("{my.key}", "{_private}"));
    let inner = tag(&p, f, "SubTree");
    assert_eq!(
        check_error(&mut p, |ed| ed.create_subtree_from_selection(
            f,
            &[inner],
            "Outer"
        )),
        CommandError::KeysNotRemappable(vec!["_private".into()])
    );
    // Without _autoremap, "my.key" is refused.
    let (mut p, f) = project(&source.replace(" _autoremap=\"true\"", ""));
    let inner = tag(&p, f, "SubTree");
    assert_eq!(
        check_error(&mut p, |ed| ed.create_subtree_from_selection(
            f,
            &[inner],
            "Outer"
        )),
        CommandError::KeysNotRemappable(vec!["my.key".into()])
    );
}

#[test]
fn create_subtree_errors() {
    let (mut p, f) = project(BASE);
    let say = tag(&p, f, "SaySomething");
    let sub = tag(&p, f, "SubTree");
    let door = tag(&p, f, "IsDoorOpen");
    assert_eq!(
        check_error(&mut p, |ed| ed.create_subtree_from_selection(
            f,
            &[say, sub],
            "X"
        )),
        CommandError::NotContiguous
    );
    assert_eq!(
        check_error(&mut p, |ed| ed.create_subtree_from_selection(
            f,
            &[say, door],
            "X"
        )),
        CommandError::NotSiblings
    );
    assert_eq!(
        check_error(&mut p, |ed| ed.create_subtree_from_selection(
            f,
            &[say],
            "Walk"
        )),
        CommandError::TreeExists("Walk".into())
    );
    assert_eq!(
        check_error(&mut p, |ed| ed.create_subtree_from_selection(
            f,
            &[say],
            "OpenDoor"
        )),
        CommandError::ModelExists("OpenDoor".into())
    );
    assert_eq!(
        check_error(&mut p, |ed| ed.create_subtree_from_selection(f, &[], "X")),
        CommandError::EmptySelection
    );
    let a = check(&mut p, |ed| {
        ed.add_unconnected_node(f, "Main", "A", None, [0.0, 0.0])
    });
    assert_eq!(
        check_error(&mut p, |ed| ed.create_subtree_from_selection(f, &[a], "X")),
        CommandError::NoParent(a)
    );
}

#[test]
fn create_subtree_from_the_root_node() {
    let (mut p, f) = project(BASE);
    let move_to = tag(&p, f, "MoveTo");
    check(&mut p, |ed| {
        ed.create_subtree_from_selection(f, &[move_to], "Leg")
    });
    let t = text(&p, f);
    assert!(t.contains(
        "    <BehaviorTree ID=\"Walk\">\n        <SubTree ID=\"Leg\" target=\"{target}\"/>\n    </BehaviorTree>"
    ));
    assert!(t.contains(
        "    <BehaviorTree ID=\"Leg\">\n        <MoveTo goal='{target}'/>\n    </BehaviorTree>"
    ));
}

// ----- models ----------------------------------------------------------------

#[test]
fn model_edits() {
    let (mut p, f) = project(BASE);
    let mut model = NodeModel::new("Beep", NodeKind::Action);
    model
        .ports
        .push(PortModel::new("volume", PortDirection::Input));
    check(&mut p, |ed| ed.add_model(f, &model));
    assert!(text(&p, f).contains(
        "        <Action ID=\"Beep\">\n            <input_port name=\"volume\"/>\n        </Action>\n    </TreeNodesModel>"
    ));
    assert_eq!(p.lookup_model(f, "Beep").unwrap().0.ports.len(), 1);

    let mut changed = model.clone();
    changed.kind = NodeKind::Condition;
    changed.id = "IsLoud".into();
    check(&mut p, |ed| ed.replace_model(f, "Beep", &changed));
    assert!(text(&p, f).contains("<Condition ID=\"IsLoud\">"));
    assert!(p.lookup_model(f, "Beep").is_none());
    check(&mut p, |ed| ed.remove_model(f, "IsLoud"));
    assert_eq!(text(&p, f), BASE);

    // Errors.
    assert!(matches!(
        check_error(&mut p, |ed| ed
            .add_model(f, &NodeModel::new("a b", NodeKind::Action))),
        CommandError::InvalidName { .. }
    ));
    let mut bad = NodeModel::new("Ok", NodeKind::Action);
    bad.ports.push(PortModel::new("1st", PortDirection::Input));
    assert!(matches!(
        check_error(&mut p, |ed| ed.add_model(f, &bad)),
        CommandError::InvalidName {
            kind: NameKind::PortName,
            ..
        }
    ));
    let mut dup = NodeModel::new("Ok", NodeKind::Action);
    dup.ports.push(PortModel::new("p", PortDirection::Input));
    dup.ports.push(PortModel::new("p", PortDirection::Output));
    assert_eq!(
        check_error(&mut p, |ed| ed.add_model(f, &dup)),
        CommandError::DuplicatePort("p".into())
    );
    assert!(matches!(
        check_error(&mut p, |ed| ed
            .add_model(f, &NodeModel::new("MoveTo", NodeKind::Action))),
        CommandError::Model(_)
    ));
    assert!(matches!(
        check_error(&mut p, |ed| ed.remove_model(f, "Nope")),
        CommandError::Model(_)
    ));
}

// ----- sessions ----------------------------------------------------------------

#[test]
fn a_failed_command_in_a_session_changes_nothing() {
    let (mut p, f) = project(BASE);
    let sequence = named(&p, f, "root");
    let door = tag(&p, f, "IsDoorOpen");
    let mut history = fasttick_core::edit::History::new();
    let result = history.run(&mut p, |ed| {
        ed.add_node(f, sequence, 0, "AlwaysSuccess", None)?;
        // This fails and undoes only itself.
        assert!(ed.add_node(f, door, 0, "AlwaysSuccess", None).is_err());
        assert_eq!(ed.label(), Some("Add node"));
        ed.set_label("Two nodes");
        ed.add_node(f, sequence, 0, "AlwaysFailure", None)?;
        // Then the session fails: everything goes back.
        ed.delete_nodes(f, &[])
    });
    assert_eq!(result, Err(CommandError::EmptySelection));
    assert_eq!(text(&p, f), BASE);
    assert!(!p.file(f).unwrap().is_dirty());
    assert!(!history.can_undo());

    let n = history
        .run(&mut p, |ed| {
            ed.set_label("Two nodes");
            ed.add_node(f, sequence, 0, "AlwaysSuccess", None)?;
            ed.add_node(f, sequence, 0, "AlwaysFailure", None)
        })
        .unwrap();
    assert_eq!(history.undo_label(), Some("Two nodes"));
    assert!(p.file(f).unwrap().doc().is_attached(n));
}
