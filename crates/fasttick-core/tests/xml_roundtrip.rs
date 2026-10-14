//! Round-trip tests of the lossless XML model.
//!
//! - Every BT.CPP fixture under `tests/fixtures/btcpp` must give the same
//!   bytes after parse and write.
//! - Random well-formed documents must round-trip byte for byte.
//! - Any input that the parser accepts must round-trip byte for byte, and no
//!   input can cause a panic.

use std::path::{Path, PathBuf};

use fasttick_core::xml::{Change, Document, Node, NodeId};
use proptest::prelude::*;

/// Fixtures that are not well-formed XML on purpose. The parser must return
/// an error for them (and not panic).
const MALFORMED_FIXTURES: &[(&str, &str)] = &[
    (
        "inline/gtest_factory_16.xml",
        "the C++ test (MalformedXML_DeeplyNestedElements) adds the nested \
         elements and the end tags at run time; the literal alone has no end tags",
    ),
    (
        "inline/gtest_factory_17.xml",
        "the C++ test (MalformedXML_ModerateNestingIsOK) adds the nested \
         elements and the end tags at run time; the literal alone has no end tags",
    ),
    (
        "inline/gtest_switch_1.xml",
        "malformed in BT.CPP: the value of case_2 has no closing quote \
         (case_2=\"42 case_3=\"666\"); no test in gtest_switch.cpp uses this text",
    ),
];

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/btcpp")
}

fn fixture_files() -> Vec<(String, Vec<u8>)> {
    let root = fixture_root();
    let mut stack = vec![root.clone()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "xml") {
                let rel = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push((rel, std::fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

fn is_malformed(rel: &str) -> bool {
    MALFORMED_FIXTURES.iter().any(|(name, _)| *name == rel)
}

#[test]
fn fixture_set_is_complete() {
    let files = fixture_files();
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    // 19 XML files and 298 raw strings of BT.CPP 4.9.0.
    assert_eq!(files.len(), 19 + 298, "{names:?}");
    for expected in [
        "package.xml",
        "examples/test_files/subtrees/Talk.xml",
        "fuzzing/corpus/bt_corpus/corpus1.xml",
        "tests/trees/child/child/child_child_no_include.xml",
        "inline/t01_build_your_first_tree_1.xml",
        "inline/gtest_subtree_26.xml",
    ] {
        assert!(names.contains(&expected), "missing fixture {expected}");
    }
    for (name, reason) in MALFORMED_FIXTURES {
        assert!(
            names.contains(name),
            "skip list names a missing file {name}"
        );
        assert!(!reason.is_empty());
    }
}

#[test]
fn fixtures_round_trip_byte_identical() {
    let mut checked = 0;
    for (name, bytes) in fixture_files() {
        if is_malformed(&name) {
            continue;
        }
        let doc = Document::parse_bytes(&bytes)
            .unwrap_or_else(|e| panic!("{name}: unexpected parse error: {e}"));
        assert!(
            doc.to_string().as_bytes() == bytes.as_slice(),
            "{name}: output differs from input"
        );
        assert!(
            doc.write_regenerated().as_bytes() == bytes.as_slice(),
            "{name}: regenerated start tags differ from the input"
        );
        let root = doc.document_element().expect("document element");
        // Every BT.CPP document element in the fixtures is <root>, except the
        // ROS package manifest.
        if name != "package.xml" {
            assert_eq!(doc.name(root), Some("root"), "{name}");
        }
        checked += 1;
    }
    assert_eq!(checked, 19 + 298 - MALFORMED_FIXTURES.len());
}

#[test]
fn malformed_fixtures_are_errors() {
    for (name, bytes) in fixture_files() {
        if !is_malformed(&name) {
            continue;
        }
        let err = Document::parse_bytes(&bytes).expect_err(&name);
        assert!(err.line >= 1 && err.column >= 1, "{name}: {err}");
    }
}

#[test]
fn fixture_subtrees_copy_and_paste_identically() {
    for (name, bytes) in fixture_files() {
        if is_malformed(&name) {
            continue;
        }
        let text = String::from_utf8(bytes).unwrap();
        let mut doc = Document::parse(&text).unwrap();
        let elements: Vec<NodeId> = doc
            .descendants(doc.root())
            .filter(|&n| doc.is_element(n))
            .collect();
        for element in elements {
            let offset = doc.source_offset(element).unwrap();
            let written = doc.write_node(element);
            // The written subtree is the exact source text of the element.
            assert!(text[offset..].starts_with(&written), "{name}");
            // A copy writes the same text.
            let copy = doc.clone_subtree(element).unwrap();
            assert_eq!(doc.write_node(copy), written, "{name}");
            // Paste: parse the text as a fragment.
            let pasted = doc.parse_fragment(&written).unwrap();
            assert_eq!(pasted.len(), 1);
            assert_eq!(doc.write_node(pasted[0]), written, "{name}");
        }
        assert_eq!(doc.to_string(), text, "{name}: copies changed the document");
    }
}

#[test]
fn fixture_edits_undo_to_identical_bytes() {
    for (name, bytes) in fixture_files() {
        if is_malformed(&name) {
            continue;
        }
        let text = String::from_utf8(bytes).unwrap();
        let mut doc = Document::parse(&text).unwrap();
        let elements: Vec<NodeId> = doc
            .descendants(doc.root())
            .filter(|&n| doc.is_element(n))
            .collect();
        let document_element = doc.document_element().unwrap();
        for &element in &elements {
            // Attribute changes.
            let change = doc
                .set_attribute(element, "name", "a \"b\" & <c>\n\td")
                .unwrap();
            assert_ne!(doc.to_string(), text);
            let reparsed = Document::parse(&doc.to_string()).unwrap();
            assert_eq!(
                reparsed.attribute(element_at(&reparsed, &doc, element), "name"),
                Some("a \"b\" & <c>\n\td"),
                "{name}"
            );
            doc.apply(change.inverse()).unwrap();
            assert_eq!(doc.to_string(), text, "{name}: set_attribute undo");

            // Rename.
            let old = doc.rename_element(element, "Renamed").unwrap();
            doc.rename_element(element, &old).unwrap();
            assert_eq!(doc.to_string(), text, "{name}: rename undo");

            if element == document_element {
                continue;
            }
            // Pretty removal and its undo.
            let undo = doc.remove_element_pretty(element).unwrap();
            let removed = doc.to_string();
            assert!(Document::parse(&removed).is_ok(), "{name}");
            let redo = doc.apply_all(undo).unwrap();
            assert_eq!(doc.to_string(), text, "{name}: remove_element_pretty undo");
            let undo = doc.apply_all(redo).unwrap();
            assert_eq!(doc.to_string(), removed, "{name}: redo");
            doc.apply_all(undo).unwrap();
            assert_eq!(doc.to_string(), text, "{name}: second undo");
        }
    }
}

/// Finds the element of `other` at the same document-order position as
/// `element` in `doc`.
fn element_at(other: &Document, doc: &Document, element: NodeId) -> NodeId {
    let index = doc
        .descendants(doc.root())
        .filter(|&n| doc.is_element(n))
        .position(|n| n == element)
        .unwrap();
    other
        .descendants(other.root())
        .filter(|&n| other.is_element(n))
        .nth(index)
        .unwrap()
}

// ---------------------------------------------------------------------------
// Random well-formed documents.

/// A piece of attribute or text content: the source text and the value that
/// tinyxml2 reads.
type Piece = (String, String);

#[derive(Debug, Clone)]
struct GenAttr {
    leading: String,
    equals: String,
    quote: char,
    pieces: Vec<Piece>,
}

#[derive(Debug, Clone)]
enum GenNode {
    Element(GenElement),
    Text(Vec<Piece>),
    Comment(String),
    CData(String),
}

#[derive(Debug, Clone)]
struct GenElement {
    name: String,
    attributes: Vec<GenAttr>,
    tail: String,
    self_closing: bool,
    end_ws: String,
    children: Vec<GenNode>,
}

#[derive(Debug, Clone)]
struct GenDocument {
    bom: bool,
    prolog: Vec<String>,
    root: GenElement,
    epilog: Vec<String>,
}

fn name_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        "[A-Za-z_][A-Za-z0-9_.:-]{0,8}",
        Just("Sequence".to_owned()),
        Just("ñame".to_owned()),
        Just("中文".to_owned()),
    ]
}

fn ws_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(" ".to_owned()),
        Just("  ".to_owned()),
        Just("\t".to_owned()),
        Just("\n    ".to_owned()),
        Just("\r\n  ".to_owned()),
        Just("\n\n".to_owned()),
    ]
}

fn opt_ws_strategy() -> impl Strategy<Value = String> {
    prop_oneof![Just(String::new()), ws_strategy()]
}

/// Content pieces that are valid in a value quoted with `quote`.
fn value_piece(quote: char) -> impl Strategy<Value = Piece> {
    let other = if quote == '"' { '\'' } else { '"' };
    prop_oneof![
        4 => "[a-zA-Z0-9 _{}=.,;:!?/*+-]{1,6}".prop_map(|s| (s.clone(), s)),
        1 => "[äöü€中😀]{1,2}".prop_map(|s| (s.clone(), s)),
        1 => Just(("&amp;".to_owned(), "&".to_owned())),
        1 => Just(("&lt;".to_owned(), "<".to_owned())),
        1 => Just(("&gt;".to_owned(), ">".to_owned())),
        1 => Just(("&quot;".to_owned(), "\"".to_owned())),
        1 => Just(("&apos;".to_owned(), "'".to_owned())),
        1 => Just(("&#65;".to_owned(), "A".to_owned())),
        1 => Just(("&#x4e2d;".to_owned(), "中".to_owned())),
        1 => Just(("&#10;".to_owned(), "\n".to_owned())),
        1 => Just(("&#9;".to_owned(), "\t".to_owned())),
        1 => Just((">".to_owned(), ">".to_owned())),
        1 => Just((other.to_string(), other.to_string())),
        1 => Just(("\n".to_owned(), "\n".to_owned())),
        1 => Just(("\r\n".to_owned(), "\n".to_owned())),
        1 => Just(("\t".to_owned(), "\t".to_owned())),
    ]
}

fn attr_strategy() -> impl Strategy<Value = GenAttr> {
    (
        ws_strategy(),
        prop_oneof![
            Just("=".to_owned()),
            Just(" = ".to_owned()),
            Just("\n=\t".to_owned())
        ],
        prop_oneof![Just('"'), Just('\'')],
    )
        .prop_flat_map(|(leading, equals, quote)| {
            proptest::collection::vec(value_piece(quote), 0..5).prop_map(move |pieces| GenAttr {
                leading: leading.clone(),
                equals: equals.clone(),
                quote,
                pieces,
            })
        })
}

fn text_piece() -> impl Strategy<Value = Piece> {
    prop_oneof![
        4 => "[a-zA-Z0-9 _{}=.,;:!?/*+'\"-]{1,8}".prop_map(|s| (s.clone(), s)),
        2 => prop_oneof![
            Just("\n".to_owned()),
            Just("\r\n".to_owned()),
            Just("\n    ".to_owned()),
            Just("\r\n\t".to_owned())
        ]
        .prop_map(|s| (s.clone(), s.replace("\r\n", "\n"))),
        1 => Just(("&amp;".to_owned(), "&".to_owned())),
        1 => Just(("&lt;".to_owned(), "<".to_owned())),
        1 => Just(("&#x42;".to_owned(), "B".to_owned())),
        1 => Just((">".to_owned(), ">".to_owned())),
        1 => Just(("中".to_owned(), "中".to_owned())),
    ]
}

fn leaf_strategy() -> impl Strategy<Value = GenNode> {
    prop_oneof![
        4 => proptest::collection::vec(text_piece(), 1..4).prop_map(GenNode::Text),
        1 => "[a-z <>&'\"\n]{0,10}"
            .prop_filter("no --", |s| !s.contains("--") && !s.ends_with('-'))
            .prop_map(GenNode::Comment),
        1 => "[a-z <>&'\"\n\\]]{0,10}"
            .prop_filter("no ]]>", |s| !s.contains("]]>") && !s.ends_with(']'))
            .prop_map(GenNode::CData),
        3 => element_strategy(Just(Vec::new()).boxed()).prop_map(GenNode::Element),
    ]
}

fn element_strategy(children: BoxedStrategy<Vec<GenNode>>) -> impl Strategy<Value = GenElement> {
    (
        name_strategy(),
        proptest::collection::vec(attr_strategy(), 0..4),
        opt_ws_strategy(),
        any::<bool>(),
        prop_oneof![
            Just(String::new()),
            Just(" ".to_owned()),
            Just("\n".to_owned())
        ],
        children,
    )
        .prop_map(
            |(name, attributes, tail, self_closing, end_ws, children)| GenElement {
                name,
                attributes,
                tail,
                self_closing: self_closing && children.is_empty(),
                end_ws,
                children,
            },
        )
}

fn tree_strategy() -> impl Strategy<Value = GenElement> {
    let node = leaf_strategy().prop_recursive(4, 40, 6, |inner| {
        element_strategy(proptest::collection::vec(inner, 0..6).boxed()).prop_map(GenNode::Element)
    });
    element_strategy(proptest::collection::vec(node, 0..6).boxed())
}

fn misc_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("\n".to_owned()),
        Just("\r\n".to_owned()),
        Just("<!-- top comment -->".to_owned()),
    ]
}

/// Processing instructions. tinyxml2 reads them as declarations, so they
/// can only come at the start, before all other content
/// (`XMLNode::ParseDeep`, "wellLocated").
fn pi_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("<?xml-stylesheet href=\"a.xsl\"?>".to_owned()),
        Just("<?pi?>".to_owned()),
        "[a-z <>&'\"=]{0,10}"
            .prop_filter("no ?>", |s| !s.contains("?>"))
            .prop_map(|s| format!("<?target {s}?>")),
    ]
}

fn document_strategy() -> impl Strategy<Value = GenDocument> {
    (
        any::<bool>(),
        prop_oneof![
            Just(None),
            Just(Some("<?xml version=\"1.0\" encoding=\"UTF-8\"?>")),
            Just(Some("<?xml version='1.0'?>")),
        ],
        prop_oneof![
            Just(None),
            Just(Some("<!DOCTYPE root>")),
            Just(Some(
                "<!DOCTYPE root [\n  <!ENTITY e \"x>y\">\n  <!-- ] -->\n]>"
            )),
        ],
        proptest::collection::vec(misc_strategy(), 0..3),
        tree_strategy(),
        proptest::collection::vec(misc_strategy(), 0..3),
        proptest::collection::vec((pi_strategy(), opt_ws_strategy()), 0..3),
    )
        .prop_map(|(bom, decl, doctype, mut misc, root, epilog, pis)| {
            let mut prolog = Vec::new();
            if let Some(d) = decl {
                prolog.push(d.to_owned());
                prolog.push("\n".to_owned());
            }
            for (pi, ws) in pis {
                prolog.push(pi);
                prolog.push(ws);
            }
            if let Some(d) = doctype {
                prolog.push(d.to_owned());
            }
            prolog.append(&mut misc);
            GenDocument {
                bom,
                prolog,
                root,
                epilog,
            }
        })
}

fn render_element(e: &GenElement, out: &mut String) {
    out.push('<');
    out.push_str(&e.name);
    for (i, a) in e.attributes.iter().enumerate() {
        out.push_str(&a.leading);
        out.push_str(&format!("a{i}"));
        out.push_str(&a.equals);
        out.push(a.quote);
        for (raw, _) in &a.pieces {
            out.push_str(raw);
        }
        out.push(a.quote);
    }
    out.push_str(&e.tail);
    if e.self_closing {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for child in &e.children {
        match child {
            GenNode::Element(c) => render_element(c, out),
            GenNode::Text(pieces) => {
                for (raw, _) in pieces {
                    out.push_str(raw);
                }
            }
            GenNode::Comment(c) => {
                out.push_str("<!--");
                out.push_str(c);
                out.push_str("-->");
            }
            GenNode::CData(c) => {
                out.push_str("<![CDATA[");
                out.push_str(c);
                out.push_str("]]>");
            }
        }
    }
    out.push_str("</");
    out.push_str(&e.name);
    out.push_str(&e.end_ws);
    out.push('>');
}

fn render(doc: &GenDocument) -> String {
    let mut out = String::new();
    if doc.bom {
        out.push('\u{feff}');
    }
    for p in &doc.prolog {
        out.push_str(p);
    }
    render_element(&doc.root, &mut out);
    for p in &doc.epilog {
        out.push_str(p);
    }
    out
}

/// Checks the names and the decoded attribute values of the parsed
/// elements against the generated tree.
fn check_element(doc: &Document, id: NodeId, e: &GenElement) -> Result<(), TestCaseError> {
    prop_assert_eq!(doc.name(id), Some(e.name.as_str()));
    let attributes = doc.attributes(id);
    prop_assert_eq!(attributes.len(), e.attributes.len());
    for (i, (a, g)) in attributes.iter().zip(&e.attributes).enumerate() {
        let expected: String = g.pieces.iter().map(|(_, v)| v.as_str()).collect();
        let raw: String = g.pieces.iter().map(|(r, _)| r.as_str()).collect();
        prop_assert_eq!(a.name(), format!("a{i}"));
        prop_assert_eq!(a.value(), expected.as_str());
        prop_assert_eq!(a.raw_value(), raw.as_str());
        prop_assert_eq!(a.quote().as_char(), g.quote);
        prop_assert_eq!(a.leading_whitespace(), g.leading.as_str());
    }
    let element = doc.element(id).unwrap();
    prop_assert_eq!(element.is_self_closing(), e.self_closing);
    let child_elements: Vec<NodeId> = doc.element_children(id).collect();
    let gen_elements: Vec<&GenElement> = e
        .children
        .iter()
        .filter_map(|c| match c {
            GenNode::Element(g) => Some(g),
            _ => None,
        })
        .collect();
    prop_assert_eq!(child_elements.len(), gen_elements.len());
    for (c, g) in child_elements.into_iter().zip(gen_elements) {
        check_element(doc, c, g)?;
    }
    Ok(())
}

fn expected_text(e: &GenElement, out: &mut String) {
    for child in &e.children {
        match child {
            GenNode::Element(c) => expected_text(c, out),
            GenNode::Text(pieces) => {
                for (_, v) in pieces {
                    out.push_str(v);
                }
            }
            GenNode::CData(c) => out.push_str(&c.replace("\r\n", "\n")),
            _ => {}
        }
    }
}

/// 512 cases, or the number in the `PROPTEST_CASES` environment variable.
fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(512);
    ProptestConfig {
        cases,
        ..ProptestConfig::default()
    }
}

/// One random edit. The indexes select nodes, positions and attributes.
#[derive(Debug, Clone)]
struct EditOp {
    kind: u8,
    a: proptest::sample::Index,
    b: proptest::sample::Index,
    c: proptest::sample::Index,
    name: String,
    value: String,
}

fn edit_op_strategy() -> impl Strategy<Value = EditOp> {
    (
        0u8..12,
        any::<proptest::sample::Index>(),
        any::<proptest::sample::Index>(),
        any::<proptest::sample::Index>(),
        prop_oneof![name_strategy(), Just("bad name".to_owned())],
        "[^\u{0}]{0,8}",
    )
        .prop_map(|(kind, a, b, c, name, value)| EditOp {
            kind,
            a,
            b,
            c,
            name,
            value,
        })
}

/// Applies one random edit. Returns the changes that undo it, or `None`
/// when the edit was refused.
fn apply_edit(doc: &mut Document, op: &EditOp) -> Option<Vec<Change>> {
    let nodes: Vec<NodeId> = doc.descendants(doc.root()).collect();
    let elements: Vec<NodeId> = nodes
        .iter()
        .copied()
        .filter(|&n| doc.is_element(n))
        .collect();
    let containers: Vec<NodeId> = std::iter::once(doc.root())
        .chain(elements.iter().copied())
        .collect();
    let node = *op.a.get(&nodes);
    let parent = *op.b.get(&containers);
    let index = op.c.index(doc.children(parent).len() + 1);
    match op.kind {
        0 => {
            let e = doc.create_element(&op.name).ok()?;
            doc.insert_child(parent, index, e).ok()?;
            Some(vec![Change::Detach { node: e }])
        }
        1 => {
            let t = doc.create_text(&op.value).ok()?;
            doc.insert_child(parent, index, t).ok()?;
            Some(vec![Change::Detach { node: t }])
        }
        2 => {
            let c = doc.create_comment(&op.value).ok()?;
            doc.insert_child(parent, index, c).ok()?;
            Some(vec![Change::Detach { node: c }])
        }
        3 => {
            let old = doc.detach(node).ok()?;
            Some(vec![Change::Insert {
                parent: old.parent,
                index: old.index,
                node,
            }])
        }
        4 => {
            // Out-of-range indexes test the error path.
            let old = doc.move_node(node, parent, index).ok()?;
            Some(vec![Change::Move {
                node,
                parent: old.parent,
                index: old.index,
            }])
        }
        5 => Some(vec![
            doc.set_attribute(node, &op.name, &op.value).ok()?.inverse(),
        ]),
        6 => {
            let name = doc.attributes(node).first().map(|a| a.name().to_owned())?;
            let (index, attribute) = doc.remove_attribute(node, &name).ok()??;
            Some(vec![Change::InsertAttribute {
                node,
                index,
                attribute,
            }])
        }
        7 => {
            let old = doc.rename_element(node, &op.name).ok()?;
            Some(vec![Change::Rename { node, name: old }])
        }
        8 => {
            let old = doc.set_text(node, &op.value).ok()?;
            Some(vec![Change::SetRaw { node, raw: old }])
        }
        9 => {
            let copy = doc.clone_subtree(node).ok()?;
            let count = doc.element_children(parent).count();
            doc.insert_element_pretty(parent, op.c.index(count + 1), copy)
                .ok()
        }
        10 => doc.remove_element_pretty(node).ok(),
        _ => {
            let written = doc.write_node(node);
            let pasted = doc.parse_fragment(&written).ok()?;
            let first = *pasted.first()?;
            doc.insert_child(parent, index, first).ok()?;
            Some(vec![Change::Detach { node: first }])
        }
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn random_edit_sequences_reparse_and_undo(
        generated in document_strategy(),
        ops in proptest::collection::vec(edit_op_strategy(), 1..12),
    ) {
        let text = render(&generated);
        let mut doc = Document::parse(&text).unwrap();
        let mut undo: Vec<Vec<Change>> = Vec::new();
        for op in &ops {
            let before = doc.to_string();
            match apply_edit(&mut doc, op) {
                Some(changes) => {
                    let after = doc.to_string();
                    // Every edit gives a document that parses again to the
                    // same bytes.
                    let reparsed = Document::parse(&after);
                    prop_assert!(reparsed.is_ok(), "{:?} after {:?}\n{}", reparsed.err(), op, after);
                    prop_assert_eq!(reparsed.unwrap().to_string(), after);
                    undo.push(changes);
                }
                // A refused edit does not change the document.
                None => prop_assert_eq!(doc.to_string(), before, "{:?}", op),
            }
        }
        for changes in undo.into_iter().rev() {
            doc.apply_all(changes).unwrap();
        }
        prop_assert_eq!(doc.to_string(), text);
    }

    #[test]
    fn random_documents_round_trip(generated in document_strategy()) {
        let text = render(&generated);
        let doc = Document::parse(&text);
        prop_assert!(doc.is_ok(), "{:?}\n{}", doc.err(), text);
        let doc = doc.unwrap();
        prop_assert_eq!(doc.to_string(), text.clone());
        prop_assert_eq!(doc.write_regenerated(), text.clone());
        prop_assert_eq!(doc.has_bom(), generated.bom);
        let root = doc.document_element().unwrap();
        check_element(&doc, root, &generated.root)?;
        let mut expected = String::new();
        expected_text(&generated.root, &mut expected);
        prop_assert_eq!(doc.text_content(root), expected);
    }

    #[test]
    fn random_documents_survive_copy_and_paste(generated in document_strategy()) {
        let text = render(&generated);
        let mut doc = Document::parse(&text).unwrap();
        let root = doc.document_element().unwrap();
        let written = doc.write_node(root);
        let pasted = doc.parse_fragment(&written).unwrap();
        prop_assert_eq!(pasted.len(), 1);
        prop_assert_eq!(doc.write_node(pasted[0]), written);
        let mut other = Document::new();
        let imported = other.import_subtree(&doc, root).unwrap();
        other.append_child(other.root(), imported).unwrap();
        prop_assert_eq!(other.to_string(), doc.write_node(root));
    }

    #[test]
    fn random_documents_undo_attribute_edits(generated in document_strategy(), value in "[^\u{0}]{0,12}") {
        let text = render(&generated);
        let mut doc = Document::parse(&text).unwrap();
        let elements: Vec<NodeId> =
            doc.descendants(doc.root()).filter(|&n| doc.is_element(n)).collect();
        let mut undo = Vec::new();
        for (i, &e) in elements.iter().enumerate() {
            let name = if i % 2 == 0 { "a0" } else { "new_attr" };
            undo.push(doc.set_attribute(e, name, &value).unwrap().inverse());
        }
        let edited = doc.to_string();
        let reparsed = Document::parse(&edited).unwrap();
        prop_assert_eq!(reparsed.to_string(), edited.clone());
        for (i, e) in reparsed
            .descendants(reparsed.root())
            .filter(|&n| reparsed.is_element(n))
            .enumerate()
        {
            let name = if i % 2 == 0 { "a0" } else { "new_attr" };
            prop_assert_eq!(reparsed.attribute(e, name), Some(value.as_str()));
        }
        undo.reverse();
        doc.apply_all(undo).unwrap();
        prop_assert_eq!(doc.to_string(), text);
    }

    #[test]
    fn arbitrary_strings_never_panic(s in any::<String>()) {
        if let Ok(doc) = Document::parse(&s) {
            prop_assert_eq!(doc.to_string(), s.clone());
            prop_assert_eq!(doc.write_regenerated(), s);
        }
    }

    #[test]
    fn token_soup_never_panics(tokens in proptest::collection::vec(
        prop_oneof![
            Just("<a"), Just("<b"), Just(">"), Just("/>"), Just("</a>"), Just("</b>"),
            Just(" x=\"1\""), Just(" y='&amp;'"), Just(" z = \"<'>\""), Just(" x"), Just("="),
            Just("<!--"), Just("-->"), Just("<![CDATA["), Just("]]>"), Just("<?p "), Just("?>"),
            Just("<?xml version='1.0'?>"), Just("<!DOCTYPE a ["), Just("]>"), Just("&"),
            Just("&#65;"), Just("&#;"), Just("text"), Just(" "), Just("\n"), Just("\r\n"),
            Just("\u{feff}"), Just("\""), Just("'"), Just("ü"),
        ],
        0..30,
    )) {
        let s: String = tokens.concat();
        if let Ok(doc) = Document::parse(&s) {
            prop_assert_eq!(doc.to_string(), s.clone());
            prop_assert_eq!(doc.write_regenerated(), s);
        }
    }

    #[test]
    fn random_pretty_edits_undo(
        generated in document_strategy(),
        pick in any::<proptest::sample::Index>(),
        target in any::<proptest::sample::Index>(),
        position in any::<proptest::sample::Index>(),
    ) {
        let text = render(&generated);
        let mut doc = Document::parse(&text).unwrap();
        let elements: Vec<NodeId> =
            doc.descendants(doc.root()).filter(|&n| doc.is_element(n)).collect();
        let source = *pick.get(&elements);
        let parent = *target.get(&elements);
        let copy = doc.clone_subtree(source).unwrap();
        let count = doc.element_children(parent).count();
        let index = position.index(count + 1);
        let undo = doc.insert_element_pretty(parent, index, copy).unwrap();
        let edited = doc.to_string();
        let reparsed = Document::parse(&edited);
        prop_assert!(reparsed.is_ok(), "{:?}\n{}", reparsed.err(), edited);
        prop_assert_eq!(doc.element_children(parent).nth(index), Some(copy));
        let redo = doc.apply_all(undo).unwrap();
        prop_assert_eq!(doc.to_string(), text.clone());
        doc.apply_all(redo).unwrap();
        prop_assert_eq!(doc.to_string(), edited);

        // Remove an element (not the document element) with its white space.
        let doc_element = doc.document_element().unwrap();
        if let Some(&victim) = elements.iter().find(|&&e| e != doc_element && doc.is_attached(e)) {
            let before = doc.to_string();
            let undo = doc.remove_element_pretty(victim).unwrap();
            prop_assert!(Document::parse(&doc.to_string()).is_ok());
            doc.apply_all(undo).unwrap();
            prop_assert_eq!(doc.to_string(), before);
        }
    }

    #[test]
    fn xml_like_strings_never_panic(s in "[<>/=\"' ab&;#x0-9!?\\-\\[\\]A-D\r\n\t]{0,80}") {
        if let Ok(doc) = Document::parse(&s) {
            prop_assert_eq!(doc.to_string(), s.clone());
            prop_assert_eq!(doc.write_regenerated(), s);
        }
    }

    #[test]
    fn xml_like_fragments_never_panic(s in "(<a( b='[<&>x]{0,3}')?>|</a>|<a/>|<!--x-->|<!\\[CDATA\\[y\\]\\]>|&amp;|&#6[0-9];|&| |\n|t){0,12}") {
        let mut doc = Document::new();
        if let Ok(nodes) = doc.parse_fragment(&s) {
            prop_assert!(!nodes.is_empty());
        }
        if let Ok(doc) = Document::parse(&s) {
            prop_assert_eq!(doc.to_string(), s);
        }
    }

    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..120)) {
        if let Ok(doc) = Document::parse_bytes(&bytes) {
            prop_assert_eq!(doc.to_string().into_bytes(), bytes);
        }
    }

    #[test]
    fn set_attribute_values_survive_a_reparse(value in "[^\u{0}]{0,24}", single in any::<bool>()) {
        let text = if single { "<a b='x'/>" } else { "<a b=\"x\"/>" };
        let mut doc = Document::parse(text).unwrap();
        let a = doc.document_element().unwrap();
        doc.set_attribute(a, "b", &value).unwrap();
        doc.set_attribute(a, "c", &value).unwrap();
        let reparsed = Document::parse(&doc.to_string()).unwrap();
        let a2 = reparsed.document_element().unwrap();
        prop_assert_eq!(reparsed.attribute(a2, "b"), Some(value.as_str()));
        prop_assert_eq!(reparsed.attribute(a2, "c"), Some(value.as_str()));
    }

    #[test]
    fn set_text_values_survive_a_reparse(value in "[^\u{0}]{0,24}", crlf in any::<bool>()) {
        let text = if crlf { "<a>\r\n</a>" } else { "<a>\n</a>" };
        let mut doc = Document::parse(text).unwrap();
        let a = doc.document_element().unwrap();
        let t = doc.children(a)[0];
        doc.set_text(t, &value).unwrap();
        let reparsed = Document::parse(&doc.to_string()).unwrap();
        let a2 = reparsed.document_element().unwrap();
        prop_assert_eq!(reparsed.text_content(a2), value.clone());
        let decoded = match reparsed.node(reparsed.children(a2).first().copied().unwrap_or(a2)) {
            Some(Node::Text(t)) => t.decoded().into_owned(),
            _ => String::new(),
        };
        prop_assert_eq!(decoded, value);
    }
}

#[test]
fn ten_thousand_elements_round_trip() {
    let mut text = String::from("<?xml version=\"1.0\"?>\n<root BTCPP_format=\"4\">\n");
    for t in 0..100 {
        text.push_str(&format!(
            "  <BehaviorTree ID=\"Tree{t}\">\n    <Sequence name=\"s{t}\">\n"
        ));
        for i in 0..98 {
            text.push_str(&format!(
                "      <Action ID=\"A{i}\" name='n &amp; {i}' value=\"{{bb_{i}}}\" _skipIf=\"x &lt; {i}\"/>\n"
            ));
        }
        text.push_str("    </Sequence>\n  </BehaviorTree>\n");
    }
    text.push_str("</root>\n");
    let doc = Document::parse(&text).unwrap();
    let elements = doc
        .descendants(doc.root())
        .filter(|&n| doc.is_element(n))
        .count();
    assert_eq!(elements, 1 + 100 * 100);
    assert_eq!(doc.to_string(), text);
    assert_eq!(doc.write_regenerated(), text);
}

#[test]
#[ignore]
fn print_samples() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    for _ in 0..3 {
        let tree = document_strategy().new_tree(&mut runner).unwrap();
        println!("-----\n{}", render(&tree.current()));
    }
}
