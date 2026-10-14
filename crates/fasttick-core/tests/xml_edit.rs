//! Editing tests: each edit gives the expected text, and its inverse gives
//! the original bytes again.

use fasttick_core::xml::{Attribute, Change, Document, EditError, Node, NodeId, Quote};

fn parse(text: &str) -> Document {
    Document::parse(text).unwrap()
}

/// Returns the element children of `id` with the given name.
fn child(doc: &Document, id: NodeId, name: &str) -> NodeId {
    doc.element_children(id)
        .find(|&c| doc.name(c) == Some(name))
        .unwrap_or_else(|| panic!("no child <{name}>"))
}

/// Finds the first element with this name in document order.
fn find(doc: &Document, name: &str) -> NodeId {
    doc.descendants(doc.root())
        .find(|&n| doc.name(n) == Some(name))
        .unwrap_or_else(|| panic!("no element <{name}>"))
}

#[test]
fn set_existing_attribute_keeps_position_quote_and_whitespace() {
    let src = "<a  x='1'\n   y=\"2\" />";
    let mut doc = parse(src);
    let a = doc.document_element().unwrap();
    let change = doc.set_attribute(a, "x", "a'b\"<&>").unwrap();
    assert_eq!(change.index, 0);
    assert_eq!(change.old.as_ref().unwrap().raw_value(), "1");
    assert_eq!(doc.to_string(), "<a  x='a&apos;b\"&lt;&amp;&gt;'\n   y=\"2\" />");
    assert_eq!(doc.attribute(a, "x"), Some("a'b\"<&>"));
    assert!(doc.element(a).unwrap().is_dirty());
    doc.apply(change.inverse()).unwrap();
    assert_eq!(doc.to_string(), src);
}

#[test]
fn set_attribute_to_the_same_value_changes_nothing() {
    let src = "<a x='&#65;'/>";
    let mut doc = parse(src);
    let a = doc.document_element().unwrap();
    doc.set_attribute(a, "x", "A").unwrap();
    assert_eq!(doc.to_string(), src);
    assert!(!doc.element(a).unwrap().is_dirty());
}

#[test]
fn new_attribute_follows_the_style_of_the_last_one() {
    let mut doc = parse("<a x='1'/>");
    let a = doc.document_element().unwrap();
    let change = doc.set_attribute(a, "z", "3").unwrap();
    assert_eq!((change.index, change.old.is_none()), (1, true));
    assert_eq!(doc.to_string(), "<a x='1' z='3'/>");

    let src = "<Action\n    ID=\"A\"\n    name=\"n\"/>";
    let mut doc = parse(src);
    let a = doc.document_element().unwrap();
    let change = doc.set_attribute(a, "port", "{bb}").unwrap();
    assert_eq!(
        doc.to_string(),
        "<Action\n    ID=\"A\"\n    name=\"n\"\n    port=\"{bb}\"/>"
    );
    doc.apply(change.inverse()).unwrap();
    assert_eq!(doc.to_string(), src);

    let mut doc = parse("<a></a>");
    let a = doc.document_element().unwrap();
    doc.set_attribute(a, "k", "line1\nline2\tx").unwrap();
    assert_eq!(doc.to_string(), "<a k=\"line1&#10;line2&#9;x\"></a>");
}

#[test]
fn remove_attribute_and_undo() {
    let src = "<a x=\"1\"   y='2' z=\"3\"/>";
    let mut doc = parse(src);
    let a = doc.document_element().unwrap();
    let (index, old) = doc.remove_attribute(a, "y").unwrap().unwrap();
    assert_eq!(index, 1);
    assert_eq!(old.value(), "2");
    assert_eq!(old.quote(), Quote::Single);
    assert_eq!(old.leading_whitespace(), "   ");
    assert_eq!(doc.to_string(), "<a x=\"1\" z=\"3\"/>");
    assert_eq!(doc.remove_attribute(a, "nope").unwrap(), None);
    doc.insert_attribute(a, index, old).unwrap();
    assert_eq!(doc.to_string(), src);
}

#[test]
fn first_attribute_always_has_white_space_before_it() {
    // tinyxml2 accepts attributes without white space between them.
    let src = "<a x=\"1\"y=\"2\"/>";
    let mut doc = parse(src);
    assert_eq!(doc.to_string(), src);
    let a = doc.document_element().unwrap();
    doc.remove_attribute(a, "x").unwrap();
    assert_eq!(doc.to_string(), "<a y=\"2\"/>");
    assert!(Document::parse(&doc.to_string()).is_ok());
}

#[test]
fn attribute_changes_through_apply() {
    let src = "<a x=\"1\"/>";
    let mut doc = parse(src);
    let a = doc.document_element().unwrap();
    let undo = doc
        .apply(Change::InsertAttribute {
            node: a,
            index: 0,
            attribute: Attribute::new("w", "<0>"),
        })
        .unwrap();
    assert_eq!(doc.to_string(), "<a w=\"&lt;0&gt;\" x=\"1\"/>");
    let redo = doc.apply(undo).unwrap();
    assert_eq!(doc.to_string(), src);
    doc.apply(redo).unwrap();
    let undo = doc
        .apply(Change::ReplaceAttribute {
            node: a,
            index: 1,
            attribute: Attribute::new("y", "2"),
        })
        .unwrap();
    assert_eq!(doc.to_string(), "<a w=\"&lt;0&gt;\" y=\"2\"/>");
    doc.apply(undo).unwrap();
    assert_eq!(doc.to_string(), "<a w=\"&lt;0&gt;\" x=\"1\"/>");
    assert_eq!(
        doc.apply(Change::InsertAttribute {
            node: a,
            index: 0,
            attribute: Attribute::new("x", "dup"),
        }),
        Err(EditError::DuplicateAttribute("x".to_owned()))
    );
    assert_eq!(
        doc.apply(Change::ReplaceAttribute {
            node: a,
            index: 0,
            attribute: Attribute::new("x", "dup"),
        }),
        Err(EditError::DuplicateAttribute("x".to_owned()))
    );
}

#[test]
fn rename_keeps_end_tag_white_space() {
    let src = "<a  x=\"1\">t</a >";
    let mut doc = parse(src);
    let a = doc.document_element().unwrap();
    let old = doc.rename_element(a, "b").unwrap();
    assert_eq!(old, "a");
    assert_eq!(doc.to_string(), "<b  x=\"1\">t</b >");
    let undo = doc
        .apply(Change::Rename {
            node: a,
            name: old,
        })
        .unwrap();
    assert_eq!(doc.to_string(), src);
    assert_eq!(
        undo,
        Change::Rename {
            node: a,
            name: "b".to_owned()
        }
    );
}

#[test]
fn insert_into_empty_element_tag_and_undo() {
    let src = "<root><a x=\"1\" /></root>";
    let mut doc = parse(src);
    let a = find(&doc, "a");
    let b = doc.create_element("b").unwrap();
    doc.insert_child(a, 0, b).unwrap();
    assert_eq!(doc.to_string(), "<root><a x=\"1\"><b/></a></root>");
    let pos = doc.detach(b).unwrap();
    assert_eq!((pos.parent, pos.index), (a, 0));
    assert_eq!(doc.to_string(), src);
    // The detached node can go back.
    doc.append_child(a, b).unwrap();
    assert_eq!(doc.to_string(), "<root><a x=\"1\"><b/></a></root>");
}

#[test]
fn removing_all_children_keeps_the_end_tag() {
    let src = "<a><b/></a>";
    let mut doc = parse(src);
    let b = find(&doc, "b");
    doc.remove(b).unwrap();
    assert_eq!(doc.to_string(), "<a></a>");
    assert!(!doc.is_attached(b));
}

#[test]
fn new_elements_text_comments_and_cdata() {
    let mut doc = Document::new();
    let root = doc.create_element("root").unwrap();
    doc.append_child(doc.root(), root).unwrap();
    assert_eq!(doc.to_string(), "<root/>");
    let text = doc.create_text("1 < 2 & 3 > 2\n").unwrap();
    let comment = doc.create_comment(" note ").unwrap();
    let cdata = doc.create_cdata("a<b").unwrap();
    doc.append_child(root, text).unwrap();
    doc.append_child(root, comment).unwrap();
    doc.append_child(root, cdata).unwrap();
    doc.set_attribute(root, "BTCPP_format", "4").unwrap();
    assert_eq!(
        doc.to_string(),
        "<root BTCPP_format=\"4\">1 &lt; 2 &amp; 3 &gt; 2\n<!-- note --><![CDATA[a<b]]></root>"
    );
    assert_eq!(doc.text_content(root), "1 < 2 & 3 > 2\na<b");
    let reparsed = parse(&doc.to_string());
    assert_eq!(reparsed.to_string(), doc.to_string());

    assert!(matches!(
        doc.create_comment("a--b"),
        Err(EditError::InvalidContent(_))
    ));
    assert!(matches!(
        doc.create_comment("a-"),
        Err(EditError::InvalidContent(_))
    ));
    assert!(matches!(
        doc.create_cdata("]]>"),
        Err(EditError::InvalidContent(_))
    ));
    assert!(matches!(
        doc.create_text("a\0"),
        Err(EditError::InvalidContent(_))
    ));
    assert_eq!(
        doc.create_element("1abc"),
        Err(EditError::InvalidName("1abc".to_owned()))
    );
}

#[test]
fn set_text_and_undo() {
    let src = "<a>x &amp; y<!--c--><![CDATA[z]]></a>";
    let mut doc = parse(src);
    let a = doc.document_element().unwrap();
    let kids = doc.children(a).to_vec();
    let old_text = doc.set_text(kids[0], "1 < 2").unwrap();
    let old_comment = doc.set_text(kids[1], " new ").unwrap();
    let old_cdata = doc.set_text(kids[2], "<raw>").unwrap();
    assert_eq!(doc.to_string(), "<a>1 &lt; 2<!-- new --><![CDATA[<raw>]]></a>");
    doc.set_raw(kids[0], old_text).unwrap();
    doc.set_raw(kids[1], old_comment).unwrap();
    doc.set_raw(kids[2], old_cdata).unwrap();
    assert_eq!(doc.to_string(), src);
    assert_eq!(doc.set_text(a, "x"), Err(EditError::WrongKind(a)));
    assert!(doc.set_raw(kids[0], "a<b".to_owned()).is_err());
    assert!(doc.set_raw(kids[1], "<!-- a --> -->".to_owned()).is_err());
    assert!(doc.set_raw(kids[2], "<![CDATA[x]]>]]>".to_owned()).is_err());
    assert_eq!(doc.to_string(), src);
}

#[test]
fn set_text_uses_the_document_line_end() {
    let mut doc = parse("<a>\r\n</a>\r\n");
    let a = doc.document_element().unwrap();
    let t = doc.children(a)[0];
    doc.set_text(t, "x\ny\rz").unwrap();
    assert_eq!(doc.to_string(), "<a>x\r\ny&#13;z</a>\r\n");
}

#[test]
fn move_node_and_undo() {
    let src = "<r><a/><b/><c/><d><e/></d></r>";
    let mut doc = parse(src);
    let r = doc.document_element().unwrap();
    let a = child(&doc, r, "a");
    let d = child(&doc, r, "d");
    // Inside the same parent: index after the removal.
    let old = doc.move_node(a, r, 2).unwrap();
    assert_eq!(doc.to_string(), "<r><b/><c/><a/><d><e/></d></r>");
    doc.move_node(a, old.parent, old.index).unwrap();
    assert_eq!(doc.to_string(), src);
    // To another parent.
    let undo = doc
        .apply(Change::Move {
            node: a,
            parent: d,
            index: 1,
        })
        .unwrap();
    assert_eq!(doc.to_string(), "<r><b/><c/><d><e/><a/></d></r>");
    doc.apply(undo).unwrap();
    assert_eq!(doc.to_string(), src);
    // Errors.
    assert_eq!(doc.move_node(r, d, 0), Err(EditError::Cycle(r)));
    assert_eq!(doc.move_node(d, d, 0), Err(EditError::Cycle(d)));
    assert_eq!(
        doc.move_node(a, r, 4),
        Err(EditError::IndexOutOfRange { index: 4, len: 3 })
    );
    let loose = doc.create_element("x").unwrap();
    assert_eq!(doc.move_node(loose, r, 0), Err(EditError::NotAttached(loose)));
    assert_eq!(doc.to_string(), src);
}

#[test]
fn insert_errors_do_not_change_the_document() {
    let src = "<r><a/></r>";
    let mut doc = parse(src);
    let r = doc.document_element().unwrap();
    let a = child(&doc, r, "a");
    let t = doc.create_text("x").unwrap();
    let e = doc.create_element("e").unwrap();
    assert_eq!(doc.insert_child(r, 0, a), Err(EditError::AlreadyAttached(a)));
    assert_eq!(doc.insert_child(t, 0, e), Err(EditError::NotAContainer(t)));
    assert_eq!(
        doc.insert_child(r, 5, e),
        Err(EditError::IndexOutOfRange { index: 5, len: 1 })
    );
    assert_eq!(doc.insert_child(e, 0, e), Err(EditError::Cycle(e)));
    let root = doc.root();
    assert_eq!(doc.insert_child(r, 0, root), Err(EditError::DocumentNode));
    assert_eq!(doc.detach(root), Err(EditError::DocumentNode));
    assert_eq!(doc.detach(e), Err(EditError::NotAttached(e)));
    let unknown = {
        let other = parse("<a><b/><c/><d/><e/><f/><g/></a>");
        other.descendants(other.root()).last().unwrap()
    };
    assert_eq!(
        doc.insert_child(r, 0, unknown),
        Err(EditError::UnknownNode(unknown))
    );
    assert_eq!(doc.set_attribute(t, "a", "b"), Err(EditError::NotAnElement(t)));
    assert_eq!(
        doc.set_attribute(a, "bad name", "b"),
        Err(EditError::InvalidName("bad name".to_owned()))
    );
    assert!(matches!(
        doc.set_attribute(a, "x", "nul\0"),
        Err(EditError::InvalidContent(_))
    ));
    assert_eq!(doc.rename_element(a, ""), Err(EditError::InvalidName(String::new())));
    // A declaration can only be at the top level.
    let decl_doc = parse("<?xml version=\"1.0\"?><x/>");
    let decl = decl_doc.children(decl_doc.root())[0];
    let decl = doc.import_subtree(&decl_doc, decl).unwrap();
    assert_eq!(doc.insert_child(r, 0, decl), Err(EditError::WrongKind(decl)));
    assert_eq!(doc.to_string(), src);
}

#[test]
fn apply_all_rolls_back_on_error() {
    let src = "<r><a/><b/></r>";
    let mut doc = parse(src);
    let r = doc.document_element().unwrap();
    let a = child(&doc, r, "a");
    let b = child(&doc, r, "b");
    let result = doc.apply_all(vec![
        Change::Detach { node: a },
        Change::Rename {
            node: b,
            name: "bb".to_owned(),
        },
        Change::Detach { node: a },
    ]);
    assert_eq!(result, Err(EditError::NotAttached(a)));
    assert_eq!(doc.to_string(), src);
}

#[test]
fn clone_import_and_write_node() {
    let src = "<r>\n  <a x='1'>\n    <!-- c -->\n    <b>t &amp; u</b>\n  </a>\n</r>";
    let mut doc = parse(src);
    let a = find(&doc, "a");
    let text = doc.write_node(a);
    assert_eq!(
        text,
        "<a x='1'>\n    <!-- c -->\n    <b>t &amp; u</b>\n  </a>"
    );
    let copy = doc.clone_subtree(a).unwrap();
    assert_eq!(doc.write_node(copy), text);
    assert_eq!(doc.parent(copy), None);
    assert_eq!(doc.source_offset(copy), None);
    assert_eq!(doc.source_offset(a), Some(6));
    let r = doc.document_element().unwrap();
    doc.append_child(r, copy).unwrap();
    assert_eq!(doc.to_string(), format!("{}{text}</r>", &src[..src.len() - 4]));

    let mut other = Document::new();
    let imported = other.import_subtree(&doc, a).unwrap();
    other.append_child(other.root(), imported).unwrap();
    assert_eq!(other.to_string(), text);
    assert_eq!(
        other.import_subtree(&doc, doc.root()),
        Err(EditError::DocumentNode)
    );
    assert_eq!(doc.write_node(NodeId::clone(&doc.root())), doc.to_string());
}

#[test]
fn parse_fragment_gives_detached_nodes() {
    let mut doc = parse("<r/>");
    let nodes = doc
        .parse_fragment("\n<A x=\"1\"/>\n<!-- c -->\n<B>t</B>\n")
        .unwrap();
    assert_eq!(nodes.len(), 3);
    assert_eq!(doc.name(nodes[0]), Some("A"));
    assert!(matches!(doc.node(nodes[1]), Some(Node::Comment(_))));
    assert_eq!(doc.name(nodes[2]), Some("B"));
    assert!(nodes.iter().all(|&n| doc.parent(n).is_none()));
    // A pasted file with a declaration.
    let nodes = doc
        .parse_fragment("<?xml version=\"1.0\"?>\n<root><X/></root>")
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(doc.write_node(nodes[0]), "<root><X/></root>");
    // Errors.
    let err = doc.parse_fragment("<A/> text <B/>").unwrap_err();
    assert_eq!((err.line, err.column), (1, 6));
    assert!(err.message.contains("text outside"), "{err}");
    let err = doc.parse_fragment("<!-- only a comment -->").unwrap_err();
    assert!(err.message.contains("no element"), "{err}");
    let err = doc.parse_fragment("  ").unwrap_err();
    assert!(err.message.contains("no element"), "{err}");
    let err = doc.parse_fragment("<A>\n<B></A>").unwrap_err();
    assert_eq!((err.line, err.column), (2, 4));
    assert_eq!(doc.to_string(), "<r/>");
}

#[test]
fn queries() {
    let src = "<?xml version=\"1.0\"?>\n<!-- top -->\n<root a=\"&lt;1&gt;\">\n  <x/>text<y><z/></y>\n</root>\n";
    let doc = parse(src);
    let root = doc.document_element().unwrap();
    assert_eq!(doc.name(root), Some("root"));
    assert_eq!(doc.attribute(root, "a"), Some("<1>"));
    assert_eq!(doc.attributes(root)[0].raw_value(), "&lt;1&gt;");
    assert_eq!(doc.attribute(root, "b"), None);
    let names: Vec<&str> = doc
        .descendants(root)
        .filter_map(|n| doc.name(n))
        .collect();
    assert_eq!(names, ["root", "x", "y", "z"]);
    let elements: Vec<NodeId> = doc.element_children(root).collect();
    assert_eq!(elements.len(), 2);
    assert_eq!(doc.parent(elements[0]), Some(root));
    assert_eq!(doc.parent(root), Some(doc.root()));
    assert_eq!(doc.parent(doc.root()), None);
    assert_eq!(doc.text_content(root), "\n  text\n");
    let y = elements[1];
    assert_eq!(doc.node_line_col(y), Some((4, 11)));
    assert_eq!(doc.line_col(doc.source_offset(root).unwrap()), (3, 1));
    assert_eq!(doc.children(doc.root()).len(), 6);
    assert_eq!(doc.index_in_parent(y), Some(3));
    assert_eq!(doc.previous_sibling(y).map(|n| doc.kind(n)), Some(doc.kind(doc.children(root)[2])));
    assert!(doc.is_attached(y));
    let decl = match doc.node(doc.children(doc.root())[0]) {
        Some(Node::Declaration(d)) => d.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(decl.pseudo_attribute("version"), Some("1.0"));
    assert_eq!(doc.ancestors(elements[1]).count(), 2);
}
