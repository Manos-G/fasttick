//! Node models: the built-in list, the harness model files and the
//! write-back of models.

use std::path::{Path, PathBuf};

use fasttick_core::bt::BtFile;
use fasttick_core::builtin::{self, BUILTIN_MODELS_XML};
use fasttick_core::models::{
    ModelIssueKind, ModelSource, NodeKind, NodeModel, PortDirection, PortModel, add_model,
    parse_models_document, parse_models_text, remove_model, replace_model, write_models_document,
};
use fasttick_core::xml::Document;

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(rel)
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

fn port(
    name: &str,
    direction: PortDirection,
    type_name: &str,
    default: Option<&str>,
    description: &str,
) -> PortModel {
    PortModel {
        name: name.into(),
        direction,
        type_name: Some(type_name.into()),
        default: default.map(str::to_owned),
        description: Some(description.into()),
    }
}

/// The custom models of the test harness, as `fasttick_dump_models` writes
/// them.
fn expected_custom_models() -> Vec<NodeModel> {
    use PortDirection::{InOut, Input, Output};
    let model = |id: &str, kind: NodeKind, ports: Vec<PortModel>| NodeModel {
        ports,
        ..NodeModel::new(id, kind)
    };
    let mut annotated = model(
        "AnnotatedAction",
        NodeKind::Action,
        vec![port(
            "weight",
            Input,
            "double",
            Some("1.500000"),
            "A double port with a default value",
        )],
    );
    annotated.metadata = vec![
        (
            "description".into(),
            "Action that carries metadata for the model tests".into(),
        ),
        ("category".into(), "fasttick-test".into()),
    ];
    vec![
        annotated,
        model(
            "CheckFlag",
            NodeKind::Condition,
            vec![port(
                "flag",
                Input,
                "bool",
                None,
                "Blackboard entry to check, for example {door_open}",
            )],
        ),
        model(
            "IncrementCounter",
            NodeKind::Action,
            vec![
                port(
                    "step",
                    Input,
                    "int",
                    Some("1"),
                    "Amount to add on each tick",
                ),
                port(
                    "counter",
                    InOut,
                    "int",
                    None,
                    "Value to increment. The node writes the new value back",
                ),
            ],
        ),
        model(
            "ReadPose",
            NodeKind::Action,
            vec![
                port(
                    "distance",
                    Output,
                    "double",
                    None,
                    "Distance of the pose from the origin",
                ),
                port(
                    "pose",
                    Input,
                    "Pose2D",
                    None,
                    "Pose to read. Text form: \"x;y;theta\"",
                ),
            ],
        ),
        model(
            "SayMessage",
            NodeKind::Action,
            vec![port(
                "message",
                Input,
                "std::string",
                Some("hello"),
                "Text that the node reads on each tick",
            )],
        ),
        model(
            "WaitTicks",
            NodeKind::Action,
            vec![port(
                "ticks",
                Input,
                "int",
                Some("3"),
                "Number of ticks in RUNNING before the node returns SUCCESS",
            )],
        ),
        model(
            "WriteValues",
            NodeKind::Action,
            vec![
                port(
                    "pose",
                    Output,
                    "Pose2D",
                    None,
                    "A Pose2D struct (JSON object)",
                ),
                port(
                    "vector_value",
                    Output,
                    "std::vector<int, std::allocator<int> >",
                    None,
                    "A vector of integers",
                ),
                port("string_value", Output, "std::string", None, "A text"),
                port(
                    "poses",
                    Output,
                    "std::vector<Pose2D, std::allocator<Pose2D> >",
                    None,
                    "A vector of Pose2D (JSON array)",
                ),
                port(
                    "double_value",
                    Output,
                    "double",
                    None,
                    "A floating-point number",
                ),
                port(
                    "int_value",
                    Output,
                    "int",
                    None,
                    "An integer (42 plus the tick count)",
                ),
            ],
        ),
    ]
}

#[test]
fn custom_models_parse_with_all_details() {
    let parsed =
        parse_models_text(&read("harness/custom_models.xml"), ModelSource::Unattached).unwrap();
    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    let models = parsed.into_models();
    assert_eq!(models, expected_custom_models());
    // Required ports (input or in/out without default).
    let required: Vec<(&str, &str)> = models
        .iter()
        .flat_map(|m| {
            m.required_ports()
                .map(move |p| (m.id.as_str(), p.name.as_str()))
        })
        .collect();
    assert_eq!(
        required,
        vec![
            ("CheckFlag", "flag"),
            ("IncrementCounter", "counter"),
            ("ReadPose", "pose"),
        ]
    );
}

#[test]
fn all_models_are_builtin_plus_custom() {
    let parsed =
        parse_models_text(&read("harness/all_models.xml"), ModelSource::Unattached).unwrap();
    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    let models = parsed.into_models();
    assert_eq!(models.len(), 42 + 7);
    let custom = expected_custom_models();
    for model in &models {
        if let Some(expected) = custom.iter().find(|c| c.id == model.id) {
            assert_eq!(model, expected);
        } else {
            let builtin =
                builtin::model(&model.id).unwrap_or_else(|| panic!("{} is not built-in", model.id));
            assert!(builtin.same_content(model), "{}", model.id);
        }
    }
    // The file is sorted by ID, as writeTreeNodesModelXML writes it.
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

#[test]
fn builtin_resource_is_current() {
    // The embedded file must be the output of the installed BT.CPP.
    let harness = fasttick_testkit::require_harness!();
    let dumped = harness.dump_models(true).unwrap();
    let dumped = parse_models_text(&dumped, ModelSource::Builtin)
        .unwrap()
        .into_models();
    let builtin_ids: Vec<&str> = builtin::models().iter().map(|m| m.id.as_str()).collect();
    for model in builtin::models() {
        let other = dumped
            .iter()
            .find(|d| d.id == model.id)
            .unwrap_or_else(|| panic!("{} is not in the dump", model.id));
        // BT.CPP writes the ports from an unordered map; compare as sets.
        assert_eq!(other.kind, model.kind, "{}", model.id);
        let mut a = other.ports.clone();
        let mut b = model.ports.clone();
        a.sort_by(|x, y| x.name.cmp(&y.name));
        b.sort_by(|x, y| x.name.cmp(&y.name));
        assert_eq!(a, b, "{}", model.id);
    }
    // The dump has the built-ins and the harness models, nothing else.
    let custom = expected_custom_models();
    for model in &dumped {
        assert!(
            builtin_ids.contains(&model.id.as_str()) || custom.iter().any(|c| c.id == model.id),
            "unexpected model {}",
            model.id
        );
    }
    assert_eq!(BUILTIN_MODELS_XML.matches(" ID=\"").count(), 42);
}

/// Adds all models to `doc` one by one, checks that each parse gives the
/// models back, then undoes all additions.
fn add_all_and_undo(text: &str, models: &[NodeModel]) {
    let mut doc = Document::parse(text).unwrap();
    let before = parse_models_document(&doc, ModelSource::Unattached).into_models();
    let mut undo_stack = Vec::new();
    for model in models {
        undo_stack.push(add_model(&mut doc, model).unwrap());
        let parsed = parse_models_document(&doc, ModelSource::Unattached);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        let back = parsed.get(&model.id).unwrap();
        assert_eq!(&back.model, model);
    }
    // The text parses again from scratch to the same models.
    let reparsed = parse_models_text(&doc.to_string(), ModelSource::Unattached).unwrap();
    assert_eq!(reparsed.models.len(), before.len() + models.len());
    for model in models {
        assert_eq!(&reparsed.get(&model.id).unwrap().model, model);
    }
    while let Some(undo) = undo_stack.pop() {
        doc.apply_all(undo).unwrap();
    }
    assert_eq!(doc.to_string(), text);
}

#[test]
fn write_back_round_trip() {
    let all = parse_models_text(&read("harness/all_models.xml"), ModelSource::Unattached)
        .unwrap()
        .into_models();
    // Into a file without a model section, with other formatting.
    add_all_and_undo(
        "<?xml version=\"1.0\"?>\r\n<root BTCPP_format=\"4\">\r\n\t<BehaviorTree ID=\"Main\">\r\n\t\t<AlwaysSuccess/>\r\n\t</BehaviorTree>\r\n</root>\r\n",
        &all,
    );
    // Into an empty section, and into a file on one line.
    add_all_and_undo("<root><TreeNodesModel/></root>", &all);
    // Into the harness tree file, which has a section with SubTree models.
    add_all_and_undo(
        &read("../../test_harness/trees/main.xml"),
        &expected_custom_models(),
    );
}

#[test]
fn replace_each_model_and_undo() {
    let text = read("harness/custom_models.xml");
    let mut doc = Document::parse(&text).unwrap();
    let models = expected_custom_models();
    let mut undo_stack = Vec::new();
    for model in &models {
        // Replace with the same content: the parse must give the same model.
        undo_stack.push(replace_model(&mut doc, &model.id, model).unwrap());
        // Then with a changed model.
        let mut changed = model.clone();
        changed
            .ports
            .push(PortModel::new("extra", PortDirection::InOut));
        changed.metadata.push(("note".into(), "a & b < c".into()));
        changed.kind = NodeKind::Decorator;
        undo_stack.push(replace_model(&mut doc, &model.id, &changed).unwrap());
        let parsed = parse_models_document(&doc, ModelSource::Unattached);
        assert_eq!(parsed.get(&model.id).unwrap().model, changed);
    }
    // Model order stays the same.
    let ids: Vec<String> = parse_models_document(&doc, ModelSource::Unattached)
        .into_models()
        .into_iter()
        .map(|m| m.id)
        .collect();
    let expected_ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
    assert_eq!(ids, expected_ids);
    while let Some(undo) = undo_stack.pop() {
        doc.apply_all(undo).unwrap();
    }
    assert_eq!(doc.to_string(), text);

    // Remove all, then undo.
    let mut undo_stack = Vec::new();
    for model in &models {
        undo_stack.push(remove_model(&mut doc, &model.id).unwrap());
    }
    assert!(
        parse_models_document(&doc, ModelSource::Unattached)
            .models
            .is_empty()
    );
    while let Some(undo) = undo_stack.pop() {
        doc.apply_all(undo).unwrap();
    }
    assert_eq!(doc.to_string(), text);
}

#[test]
fn whole_models_document_like_btcpp() {
    let all = parse_models_text(&read("harness/all_models.xml"), ModelSource::Unattached)
        .unwrap()
        .into_models();
    let text = write_models_document(&all).unwrap();
    let back = parse_models_text(&text, ModelSource::Unattached).unwrap();
    assert!(back.issues.is_empty());
    assert_eq!(back.into_models(), all);
    // The format is the format of writeTreeNodesModelXML: for the built-in
    // models, the text is the embedded file byte for byte (the parse keeps
    // the port order). The embedded file has one more line end at the end.
    let builtin_text = write_models_document(builtin::models()).unwrap();
    assert_eq!(format!("{builtin_text}\n"), BUILTIN_MODELS_XML);
}

#[test]
fn subtree_model_ports_follow_recursively_create_subtree() {
    let file = BtFile::load(fixture("btcpp/inline/gtest_subtree_15.xml")).unwrap();
    let model = &file.index().model("MySub").unwrap().model;
    assert_eq!(model.kind, NodeKind::SubTree);
    let required: Vec<&str> = model.required_ports().map(|p| p.name.as_str()).collect();
    // Every port without a default, also an output port.
    assert_eq!(required, vec!["in_name", "out_state"]);
    // The fixture remaps exactly these ports.
    let main = file.tree("MainTree").unwrap();
    let seq = file.node(main.root.unwrap()).unwrap();
    let sub = seq.children().find(|c| c.is_subtree()).unwrap();
    let remapped: Vec<&str> = sub.ports().map(|(name, _)| name).collect();
    assert_eq!(remapped, vec!["in_name", "out_state"]);
}

#[test]
fn model_issues_in_fixtures() {
    let file = BtFile::load(fixture("btcpp/inline/gtest_xml_null_subtree_id_1.xml")).unwrap();
    let issues: Vec<&ModelIssueKind> = file.index().models.issues.iter().map(|i| &i.kind).collect();
    assert_eq!(issues, vec![&ModelIssueKind::MissingId]);

    // Port names that BT.CPP rejects still parse; the validator reports them.
    let file = BtFile::load(fixture("btcpp/inline/gtest_name_validation_15.xml")).unwrap();
    assert!(file.index().models.issues.is_empty());
    let model = &file.index().model("MySubTree").unwrap().model;
    assert_eq!(model.ports.len(), 1);

    // Extra attributes on a model element are kept in the XML and ignored.
    let file = BtFile::load(fixture("btcpp/examples/test_files/log_test.xml")).unwrap();
    let sleep = &file.index().model("Sleep").unwrap().model;
    assert_eq!(
        sleep.ports,
        vec![PortModel::new("msec", PortDirection::Input)]
    );
}
