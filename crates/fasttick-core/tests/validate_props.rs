//! Random documents: the validator does not panic, its result is the same
//! each time, and each problem is complete.

use proptest::prelude::*;

use fasttick_core::bt::BtFile;
use fasttick_core::project::Project;
use fasttick_core::validate::{Problem, validate_file, validate_project};

/// Node tags: built-in nodes with special child rules, harness nodes, the
/// explicit forms, SubTrees, reserved and unknown names.
const TAGS: &[&str] = &[
    "Sequence",
    "Fallback",
    "Inverter",
    "AlwaysSuccess",
    "Sleep",
    "SubTree",
    "SubTree",
    "SayMessage",
    "WriteValues",
    "Action",
    "Condition",
    "Root",
    "IfThenElse",
    "Switch2",
    "TryCatch",
    "ReactiveSequence",
    "AsyncSequence",
    "Parallel",
    "Main",
    "NoSuchNode",
    "BehaviorTree",
];

const ATTRIBUTES: &[&str] = &[
    "name",
    "ID",
    "msec",
    "message",
    "int_value",
    "goal",
    "_skipIf",
    "_post",
    "_autoremap",
    "_uid",
    "x.y",
    "ns:port",
];

/// Parts of attribute values. The values can contain bare `&`, partial
/// references, braces and control characters.
const VALUE_PARTS: &[&str] = &[
    "", "Main", "Sub", "A", "{", "}", "{x}", "{=}", "=", "@", "&", "&amp;", "&#1;", "&#x41;",
    "&#0;", "&lt;", " ", "\t", "-1", "42", "abc", "true", "yes", "1e999", "&#", "&#x;", ";",
];

const TREE_IDS: &[&str] = &["Main", "Sub", "A", "Sequence", "Root", ""];

#[derive(Debug, Clone)]
struct GenNode {
    tag: &'static str,
    attributes: Vec<(&'static str, String)>,
    children: Vec<GenNode>,
}

fn value() -> impl Strategy<Value = String> {
    proptest::collection::vec(proptest::sample::select(VALUE_PARTS), 0..4)
        .prop_map(|parts| parts.concat())
}

fn attributes() -> impl Strategy<Value = Vec<(&'static str, String)>> {
    proptest::collection::vec((proptest::sample::select(ATTRIBUTES), value()), 0..3).prop_map(
        |mut list| {
            // XML does not allow the same attribute twice.
            let mut seen = Vec::new();
            list.retain(|(name, _)| {
                let new = !seen.contains(name);
                seen.push(*name);
                new
            });
            list
        },
    )
}

fn node() -> impl Strategy<Value = GenNode> {
    let leaf =
        (proptest::sample::select(TAGS), attributes()).prop_map(|(tag, attributes)| GenNode {
            tag,
            attributes,
            children: Vec::new(),
        });
    leaf.prop_recursive(5, 40, 4, |inner| {
        (
            proptest::sample::select(TAGS),
            attributes(),
            proptest::collection::vec(inner, 0..4),
        )
            .prop_map(|(tag, attributes, children)| GenNode {
                tag,
                attributes,
                children,
            })
    })
}

fn write_node(node: &GenNode, out: &mut String) {
    out.push('<');
    out.push_str(node.tag);
    for (name, value) in &node.attributes {
        out.push_str(&format!(" {name}=\"{value}\""));
    }
    if node.children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for child in &node.children {
        write_node(child, out);
    }
    out.push_str(&format!("</{}>", node.tag));
}

#[derive(Debug, Clone)]
struct GenDoc {
    format: Option<&'static str>,
    main: Option<&'static str>,
    models: bool,
    trees: Vec<(Option<&'static str>, Vec<GenNode>)>,
}

fn document() -> impl Strategy<Value = GenDoc> {
    (
        proptest::option::of(proptest::sample::select(&["4", "3"][..])),
        proptest::option::of(proptest::sample::select(TREE_IDS)),
        any::<bool>(),
        proptest::collection::vec(
            (
                proptest::option::of(proptest::sample::select(TREE_IDS)),
                proptest::collection::vec(node(), 0..3),
            ),
            0..4,
        ),
    )
        .prop_map(|(format, main, models, trees)| GenDoc {
            format,
            main,
            models,
            trees,
        })
}

fn write_document(doc: &GenDoc) -> String {
    let mut out = String::from("<root");
    if let Some(format) = doc.format {
        out.push_str(&format!(" BTCPP_format=\"{format}\""));
    }
    if let Some(main) = doc.main {
        out.push_str(&format!(" main_tree_to_execute=\"{main}\""));
    }
    out.push_str(">\n");
    if doc.models {
        out.push_str(
            "<TreeNodesModel><SubTree ID=\"Sub\"><input_port name=\"goal\"/>\
             <output_port name=\"out\" default=\"\"/></SubTree>\
             <Action ID=\"NoSuchNode\"><input_port name=\"msec\" type=\"int\"/></Action>\
             </TreeNodesModel>\n",
        );
    }
    for (id, roots) in &doc.trees {
        match id {
            Some(id) => out.push_str(&format!("<BehaviorTree ID=\"{id}\">")),
            None => out.push_str("<BehaviorTree>"),
        }
        for root in roots {
            write_node(root, &mut out);
        }
        out.push_str("</BehaviorTree>\n");
    }
    out.push_str("</root>\n");
    out
}

fn harness_models() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/harness/all_models.xml")
}

fn check(problems: &[Problem], project: &Project) {
    for p in problems {
        // Quoted names and values can contain any text, but the message is
        // one line.
        assert!(p.message.ends_with('.'), "{}", p.message);
        assert!(!p.message.chars().any(char::is_control), "{}", p.message);
        let file = project.file(p.file).expect("the file of a problem exists");
        if let Some(node) = p.node {
            assert!(file.doc().is_element(node), "{p}");
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(256),
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_documents(doc in document()) {
        let text = write_document(&doc);
        // The parser rejects some generated texts (for example a `<` in a
        // value cannot occur, but a reference to U+0000 can make a value
        // that BT.CPP and the parser treat in a special way). Such texts
        // are not part of this test.
        let Ok(file) = BtFile::parse(&text, None) else {
            return Ok(());
        };
        let (mut project, id) = Project::with_file("random", file);
        project.add_model_file(harness_models()).unwrap();
        let first = validate_project(&project);
        check(&first, &project);
        prop_assert_eq!(&validate_project(&project), &first);
        let of_file = validate_file(&project, id);
        prop_assert!(of_file.iter().all(|p| p.file == id));
        prop_assert!(first.starts_with(&of_file));
    }
}
