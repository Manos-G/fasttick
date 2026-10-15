//! Tests of `fasttick_core::search` on projects and runtime trees.

use std::time::{Duration, Instant};

use fasttick_core::bt::BtFile;
use fasttick_core::project::{FileId, Project};
use fasttick_core::runtime::RuntimeTree;
use fasttick_core::search::{
    Field, Fields, Location, Query, search_project, search_runtime, search_tree,
};

const MAIN: &str = r#"<root BTCPP_format="4" main_tree_to_execute="Main">
    <BehaviorTree ID="Main">
        <Sequence name="patrol">
            <MoveBase name="go_home" goal="{home_pose}" speed="0.5"/>
            <SubTree ID="Inspect" target="{home_pose}"/>
            <Script code="count := 1" _skipIf="battery &lt; 10" _onSuccess="done := true"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Inspect">
        <Fallback name="inspect_or_report">
            <Action ID="TakePicture" name="snap" target="{target}"/>
            <ReportProblem message="Move failed"/>
        </Fallback>
    </BehaviorTree>
</root>"#;

fn project() -> (Project, FileId) {
    let file = BtFile::parse(MAIN, None).expect("valid XML");
    Project::with_file("demo", file)
}

fn fields(found: &[fasttick_core::search::SearchMatch]) -> Vec<(Field, Option<&str>, &str)> {
    found
        .iter()
        .map(|m| (m.field, m.attribute.as_deref(), &m.text[m.range.clone()]))
        .collect()
}

#[test]
fn finds_each_field() {
    let (project, _) = project();
    let name = search_project(&project, &Query::new("go_home"));
    assert_eq!(fields(&name), vec![(Field::Name, None, "go_home")]);

    let type_id = search_project(&project, &Query::new("MoveBase"));
    assert_eq!(fields(&type_id), vec![(Field::Type, None, "MoveBase")]);

    let port_name = search_project(&project, &Query::new("speed"));
    assert_eq!(
        fields(&port_name),
        vec![(Field::PortName, Some("speed"), "speed")]
    );

    let port_value = search_project(&project, &Query::new("0.5"));
    assert_eq!(
        fields(&port_value),
        vec![(Field::PortValue, Some("speed"), "0.5")]
    );

    let condition = search_project(&project, &Query::new("battery"));
    assert_eq!(
        fields(&condition),
        vec![(Field::Condition, Some("_skipIf"), "battery")]
    );
    // The decoded value is searched: "&lt;" is "<".
    assert_eq!(condition[0].text, "battery < 10");
}

#[test]
fn subtree_type_is_the_target_tree_and_explicit_form_uses_id() {
    let (project, _) = project();
    let found = search_project(
        &project,
        &Query::new("Inspect").in_fields(Fields {
            type_id: true,
            ..Fields::NONE
        }),
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].location.tree_id(), "Main");

    let explicit = search_project(&project, &Query::new("TakePicture"));
    assert_eq!(fields(&explicit), vec![(Field::Type, None, "TakePicture")]);
    assert_eq!(explicit[0].location.tree_id(), "Inspect");
}

#[test]
fn fields_can_be_turned_off() {
    let (project, _) = project();
    let all = search_project(&project, &Query::new("home"));
    assert_eq!(
        fields(&all),
        vec![
            (Field::Name, None, "home"),
            (Field::PortValue, Some("goal"), "home"),
            (Field::PortValue, Some("target"), "home"),
        ]
    );
    let names_only = search_project(
        &project,
        &Query::new("home").in_fields(Fields {
            name: true,
            ..Fields::NONE
        }),
    );
    assert_eq!(names_only.len(), 1);
    assert!(search_project(&project, &Query::new("home").in_fields(Fields::NONE)).is_empty());
}

#[test]
fn case_whole_word_and_wildcards() {
    let (project, _) = project();
    assert_eq!(search_project(&project, &Query::new("movebase")).len(), 1);
    assert!(search_project(&project, &Query::new("movebase").with_case_sensitive(true)).is_empty());

    // "target" is a port name, a port value and part of "{target}".
    let target = search_project(&project, &Query::new("target").with_whole_word(true));
    assert_eq!(
        fields(&target),
        vec![
            (Field::PortName, Some("target"), "target"),
            (Field::PortName, Some("target"), "target"),
            (Field::PortValue, Some("target"), "target"),
        ]
    );
    assert!(
        search_project(&project, &Query::new("home").with_whole_word(true))
            .iter()
            .all(|m| m.field != Field::Name)
    );

    let wild = search_project(&project, &Query::new("Move*").with_wildcard(true));
    // The pattern must match the whole value: "Move failed" matches too.
    assert_eq!(
        fields(&wild),
        vec![
            (Field::Type, None, "MoveBase"),
            (Field::PortValue, Some("message"), "Move failed"),
        ]
    );
    let wild_value = search_project(&project, &Query::new("{*_pose}").with_wildcard(true));
    assert_eq!(wild_value.len(), 2);
    // Without the flag, '*' is a literal character.
    assert!(search_project(&project, &Query::new("Move*")).is_empty());
}

#[test]
fn results_are_in_document_order() {
    let (project, file) = project();
    let found = search_project(&project, &Query::new("e"));
    let mut last = None;
    for m in &found {
        let tree_index = match &m.location {
            Location::Tree { index, .. } => *index,
            Location::Unconnected { .. } => panic!("no unconnected nodes here"),
        };
        let doc = project.file(file).expect("file").doc();
        let offset = doc.source_offset(m.node).expect("parsed node");
        // Fields of one node keep the node position; ports interleave
        // names and values, so only the node order is checked here.
        let key = (tree_index, offset);
        if let Some(previous) = last {
            assert!(previous <= key, "{previous:?} then {key:?}");
        }
        last = Some(key);
    }
    assert!(found.len() > 5);
}

#[test]
fn search_tree_limits_to_one_tree() {
    let (project, file_id) = project();
    let file = project.file(file_id).expect("file");
    let found = search_tree(file_id, file, 1, &Query::new("target"));
    assert!(!found.is_empty());
    assert!(found.iter().all(|m| m.location.tree_id() == "Inspect"));
    assert!(search_tree(file_id, file, 7, &Query::new("target")).is_empty());
}

#[test]
fn empty_text_matches_nothing() {
    let (project, _) = project();
    assert!(search_project(&project, &Query::new("")).is_empty());
}

#[test]
fn unconnected_nodes_are_searched_last() {
    let (mut project, file_id) = project();
    let node = {
        let file = project.file_mut(file_id).expect("file");
        let nodes = file
            .doc_mut()
            .parse_fragment(r#"<Sleep name="nap" msec="500"/>"#)
            .expect("fragment");
        file.reindex();
        nodes[0]
    };
    project.add_unconnected(file_id, "Main", node, [10.0, 20.0]);
    let found = search_project(&project, &Query::new("a"));
    let last = found.last().expect("matches");
    assert_eq!(
        last.location,
        Location::Unconnected {
            tree: "Main".to_owned()
        }
    );
    assert_eq!(last.node, node);
    // "nap" is also inside "snap"; a whole-word search finds only "nap".
    assert_eq!(search_project(&project, &Query::new("nap")).len(), 2);
    let nap = search_project(&project, &Query::new("nap").with_whole_word(true));
    assert_eq!(nap.len(), 1);
    assert_eq!(nap[0].node, node);
}

const RUNTIME: &str = r#"<root BTCPP_format="4">
    <BehaviorTree ID="Main" _fullpath="">
        <Sequence name="patrol" _uid="1">
            <MoveBase name="go_home" _uid="2" goal="{home_pose}"/>
            <SubTree ID="Inspect" _fullpath="inspect" _uid="3" target="{home_pose}"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Inspect" _fullpath="inspect">
        <AlwaysSuccess name="ok" _uid="4" _skipIf="flag"/>
    </BehaviorTree>
</root>"#;

#[test]
fn runtime_search() {
    let tree = RuntimeTree::parse(RUNTIME).expect("runtime XML");
    let found = search_runtime(&tree, &Query::new("home"));
    let summary: Vec<(u16, Field, Option<&str>)> = found
        .iter()
        .map(|m| {
            let uid = tree.node(m.node).uid.expect("uid").0;
            (uid, m.field, m.attribute.as_deref())
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (2, Field::Name, None),
            (2, Field::PortValue, Some("goal")),
            (3, Field::PortValue, Some("target")),
        ]
    );
    // Metadata is not searched; conditions are.
    assert!(search_runtime(&tree, &Query::new("_uid")).is_empty());
    assert!(
        search_runtime(
            &tree,
            &Query::new("inspect").in_fields(Fields {
                port_values: true,
                port_names: true,
                conditions: true,
                ..Fields::NONE
            })
        )
        .is_empty()
    );
    let condition = search_runtime(&tree, &Query::new("flag"));
    assert_eq!(condition.len(), 1);
    assert_eq!(condition[0].field, Field::Condition);
}

fn big_xml(nodes: usize) -> String {
    let mut xml =
        String::from(r#"<root BTCPP_format="4"><BehaviorTree ID="Big"><Sequence name="root">"#);
    for i in 1..nodes {
        xml.push_str(&format!(
            r#"<Action{} name="node_{i}" goal="{{key_{i}}}" _skipIf="x == {i}"/>"#,
            i % 7
        ));
    }
    xml.push_str("</Sequence></BehaviorTree></root>");
    xml
}

#[test]
fn ten_thousand_nodes_are_fast() {
    let file = BtFile::parse(&big_xml(10_000), None).expect("valid XML");
    let (project, _) = Project::with_file("big", file);
    let query = Query::new("node_9999");
    let best = (0..5)
        .map(|_| {
            let start = Instant::now();
            let found = search_project(&project, &query);
            let elapsed = start.elapsed();
            assert_eq!(found.len(), 1);
            elapsed
        })
        .min()
        .unwrap_or_default();
    eprintln!("search of 10,000 nodes: {best:?}");
    if !cfg!(debug_assertions) {
        assert!(best < Duration::from_millis(10), "search took {best:?}");
    }
}
