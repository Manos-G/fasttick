//! Tests of the indentation helpers.

use fasttick_core::xml::{Document, EditError, LineEnding, NodeId};

const TREE: &str = "<?xml version=\"1.0\"?>
<root BTCPP_format=\"4\">
    <BehaviorTree ID=\"Main\">
        <Sequence>
            <A/>
            <B/>
        </Sequence>
    </BehaviorTree>
</root>
";

fn find(doc: &Document, name: &str) -> NodeId {
    doc.descendants(doc.root())
        .find(|&n| doc.name(n) == Some(name))
        .unwrap_or_else(|| panic!("no element <{name}>"))
}

/// Inserts `<C/>` (or the parsed `fragment`) as element child `index` of
/// the element `parent`, checks the result, undoes, and checks the original
/// bytes.
fn check_insert(src: &str, parent: &str, index: usize, fragment: &str, expected: &str) {
    let mut doc = Document::parse(src).unwrap();
    let p = find(&doc, parent);
    let nodes = doc.parse_fragment(fragment).unwrap();
    let undo = doc.insert_element_pretty(p, index, nodes[0]).unwrap();
    assert_eq!(doc.to_string(), expected);
    let redo = doc.apply_all(undo).unwrap();
    assert_eq!(doc.to_string(), src);
    doc.apply_all(redo).unwrap();
    assert_eq!(doc.to_string(), expected);
}

#[test]
fn indent_unit_is_inferred() {
    assert_eq!(Document::parse(TREE).unwrap().indent_unit(), "    ");
    let doc = Document::parse("<a>\n\t<b>\n\t\t<c/>\n\t</b>\n</a>").unwrap();
    assert_eq!(doc.indent_unit(), "\t");
    let doc = Document::parse("<a>\n  <b/>\n</a>").unwrap();
    assert_eq!(doc.indent_unit(), "  ");
    let doc = Document::parse("<a><b/></a>").unwrap();
    assert_eq!(doc.indent_unit(), "    ");
}

#[test]
fn insert_between_siblings() {
    check_insert(
        TREE,
        "Sequence",
        1,
        "<C/>",
        &TREE.replace("<A/>\n", "<A/>\n            <C/>\n"),
    );
}

#[test]
fn insert_first() {
    check_insert(
        TREE,
        "Sequence",
        0,
        "<C/>",
        &TREE.replace("            <A/>\n", "            <C/>\n            <A/>\n"),
    );
}

#[test]
fn append_last() {
    check_insert(
        TREE,
        "Sequence",
        2,
        "<C/>",
        &TREE.replace("<B/>\n", "<B/>\n            <C/>\n"),
    );
}

#[test]
fn insert_into_empty_element_tag() {
    check_insert(
        TREE,
        "A",
        0,
        "<C/>",
        &TREE.replace(
            "            <A/>\n",
            "            <A>\n                <C/>\n            </A>\n",
        ),
    );
}

#[test]
fn insert_into_element_with_only_white_space() {
    check_insert(
        "<a>\n  <b>\n  </b>\n</a>",
        "b",
        0,
        "<c/>",
        "<a>\n  <b>\n    <c/>\n  </b>\n</a>",
    );
    check_insert(
        "<a>\n  <b></b>\n</a>",
        "b",
        0,
        "<c/>",
        "<a>\n  <b>\n    <c/>\n  </b>\n</a>",
    );
}

#[test]
fn inserted_subtree_is_reindented() {
    check_insert(
        TREE,
        "Sequence",
        2,
        "<Fallback>\n<X/>\n\n  <Y/>\n</Fallback>",
        &TREE.replace(
            "<B/>\n",
            "<B/>\n            <Fallback>\n                <X/>\n\n                <Y/>\n            </Fallback>\n",
        ),
    );
}

#[test]
fn comments_stay_with_the_next_element() {
    check_insert(
        "<s>\n  <!-- about b -->\n  <b/>\n</s>",
        "s",
        0,
        "<n/>",
        "<s>\n  <n/>\n  <!-- about b -->\n  <b/>\n</s>",
    );
    check_insert(
        "<s>\n  <a/>\n  <!-- about b -->\n  <b/>\n  <!-- end -->\n</s>",
        "s",
        2,
        "<n/>",
        "<s>\n  <a/>\n  <!-- about b -->\n  <b/>\n  <!-- end -->\n  <n/>\n</s>",
    );
}

#[test]
fn one_line_and_compact_styles_are_kept() {
    check_insert(
        "<a> <b/> <c/> </a>",
        "a",
        1,
        "<n/>",
        "<a> <b/> <n/> <c/> </a>",
    );
    check_insert(
        "<a> <b/> <c/> </a>",
        "a",
        2,
        "<n/>",
        "<a> <b/> <c/> <n/> </a>",
    );
    check_insert("<a><b/><c/></a>", "a", 1, "<n/>", "<a><b/><n/><c/></a>");
    check_insert("<a><b/><c/></a>", "a", 2, "<n/>", "<a><b/><c/><n/></a>");
}

#[test]
fn crlf_documents_get_crlf() {
    let src = "<a>\r\n  <b/>\r\n</a>\r\n";
    check_insert(src, "a", 1, "<c/>", "<a>\r\n  <b/>\r\n  <c/>\r\n</a>\r\n");
    check_insert(
        src,
        "b",
        0,
        "<c/>",
        "<a>\r\n  <b>\r\n    <c/>\r\n  </b>\r\n</a>\r\n",
    );
    assert_eq!(
        Document::parse(src).unwrap().line_ending(),
        LineEnding::CrLf
    );
}

#[test]
fn insert_into_empty_document() {
    let mut doc = Document::new();
    let root = doc.create_element("root").unwrap();
    let r = doc.root();
    doc.insert_element_pretty(r, 0, root).unwrap();
    let child = doc.create_element("BehaviorTree").unwrap();
    doc.insert_element_pretty(root, 0, child).unwrap();
    let leaf = doc.create_element("Leaf").unwrap();
    doc.insert_element_pretty(child, 0, leaf).unwrap();
    assert_eq!(
        doc.to_string(),
        "<root>\n    <BehaviorTree>\n        <Leaf/>\n    </BehaviorTree>\n</root>"
    );
    // A document with only a declaration.
    let src = "<?xml version=\"1.0\"?>\n";
    let mut doc = Document::parse(src).unwrap();
    let root = doc.create_element("root").unwrap();
    let r = doc.root();
    let undo = doc.insert_element_pretty(r, 0, root).unwrap();
    assert_eq!(doc.to_string(), "<?xml version=\"1.0\"?>\n<root/>\n");
    doc.apply_all(undo).unwrap();
    assert_eq!(doc.to_string(), src);
}

#[test]
fn remove_pretty_and_undo() {
    let mut doc = Document::parse(TREE).unwrap();
    let a = find(&doc, "A");
    let undo = doc.remove_element_pretty(a).unwrap();
    assert_eq!(doc.to_string(), TREE.replace("            <A/>\n", ""));
    doc.apply_all(undo).unwrap();
    assert_eq!(doc.to_string(), TREE);

    let b = find(&doc, "B");
    let undo = doc.remove_element_pretty(b).unwrap();
    assert_eq!(doc.to_string(), TREE.replace("            <B/>\n", ""));
    let a = find(&doc, "A");
    let undo_a = doc.remove_element_pretty(a).unwrap();
    assert_eq!(
        doc.to_string(),
        TREE.replace("            <A/>\n            <B/>\n", "")
    );
    doc.apply_all(undo_a).unwrap();
    doc.apply_all(undo).unwrap();
    assert_eq!(doc.to_string(), TREE);
}

#[test]
fn remove_pretty_edge_cases() {
    let cases = [
        ("<a> <b/> <c/> </a>", "b", "<a> <c/> </a>"),
        ("<a><b/>\n  <c/>\n</a>", "b", "<a><c/>\n</a>"),
        ("<a><b/></a>", "b", "<a></a>"),
        (
            "<a>\n  <!-- x -->\n  <b/>\n</a>",
            "b",
            "<a>\n  <!-- x -->\n</a>",
        ),
        ("<a>text<b/>more</a>", "b", "<a>textmore</a>"),
    ];
    for (src, name, expected) in cases {
        let mut doc = Document::parse(src).unwrap();
        let node = find(&doc, name);
        let undo = doc.remove_element_pretty(node).unwrap();
        assert_eq!(doc.to_string(), expected, "{src}");
        doc.apply_all(undo).unwrap();
        assert_eq!(doc.to_string(), src);
    }
}

#[test]
fn move_with_pretty_helpers() {
    let mut doc = Document::parse(TREE).unwrap();
    let a = find(&doc, "A");
    let tree = find(&doc, "BehaviorTree");
    let mut undo = doc.remove_element_pretty(a).unwrap();
    let mut undo_insert = doc.insert_element_pretty(tree, 1, a).unwrap();
    assert_eq!(
        doc.to_string(),
        TREE.replace("            <A/>\n", "").replace(
            "        </Sequence>\n",
            "        </Sequence>\n        <A/>\n"
        )
    );
    undo_insert.append(&mut undo);
    doc.apply_all(undo_insert).unwrap();
    assert_eq!(doc.to_string(), TREE);
}

#[test]
fn pretty_errors() {
    let mut doc = Document::parse(TREE).unwrap();
    let seq = find(&doc, "Sequence");
    let a = find(&doc, "A");
    let c = doc.create_element("C").unwrap();
    assert_eq!(
        doc.insert_element_pretty(seq, 3, c),
        Err(EditError::IndexOutOfRange { index: 3, len: 2 })
    );
    assert_eq!(
        doc.insert_element_pretty(seq, 0, a),
        Err(EditError::AlreadyAttached(a))
    );
    assert_eq!(doc.remove_element_pretty(c), Err(EditError::NotAttached(c)));
    let root = doc.root();
    assert_eq!(
        doc.remove_element_pretty(root),
        Err(EditError::DocumentNode)
    );
    let text = doc.children(seq)[0];
    assert_eq!(
        doc.remove_element_pretty(text),
        Err(EditError::WrongKind(text))
    );
    assert_eq!(doc.to_string(), TREE);
}
