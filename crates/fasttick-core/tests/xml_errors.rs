//! Parse errors: kinds and positions. Also the inputs that tinyxml2 (and so
//! BT.CPP) accepts although they are not well-formed XML.

use fasttick_core::xml::{Document, MAX_ELEMENT_DEPTH, Node};

/// Parses `text`, expects an error, and returns (line, column, message).
fn error(text: &str) -> (usize, usize, String) {
    let e = Document::parse(text).expect_err(text);
    assert_eq!(
        format!("{e}"),
        format!("line {}, column {}: {}", e.line, e.column, e.message)
    );
    (e.line, e.column, e.message)
}

#[track_caller]
fn assert_error(text: &str, line: usize, column: usize, contains: &str) {
    let (l, c, m) = error(text);
    assert_eq!((l, c), (line, column), "{text:?}: {m}");
    assert!(
        m.contains(contains),
        "{text:?}: {m:?} does not contain {contains:?}"
    );
}

#[test]
fn mismatched_end_tag() {
    assert_error(
        "<a>\n  <b>\n  </c>\n</a>",
        3,
        3,
        "the end tag </c> does not match the start tag <b> at line 2, column 3",
    );
}

#[test]
fn unclosed_elements() {
    assert_error("<a>\n<b>", 2, 1, "<b> has no end tag");
    assert_error("<a><b/>", 1, 1, "<a> has no end tag");
    assert_error("<a>text", 1, 1, "<a> has no end tag");
}

#[test]
fn unmatched_end_tag() {
    assert_error("<a/></b>", 1, 5, "</b> has no matching start tag");
    assert_error("</b>", 1, 1, "</b> has no matching start tag");
}

#[test]
fn malformed_start_tags() {
    assert_error("<a x=\"1\" x=\"2\"/>", 1, 10, "duplicate attribute \"x\"");
    assert_error("<a x=1/>", 1, 6, "must be in quotes");
    assert_error("<a x/>", 1, 5, "expected '='");
    assert_error("<a x=\"1\" =\"2\"/>", 1, 10, "unexpected character '='");
    assert_error("< a/>", 1, 2, "expected an element name");
    assert_error("<1a/>", 1, 2, "expected an element name");
    assert_error("<a/ >", 1, 3, "'/' must be directly before '>'");
    assert_error("<a></a b>", 1, 8, "unexpected text in the end tag");
    assert_error("<a></ a>", 1, 6, "expected an element name after '</'");
    assert_error("<a>x < y</a>", 1, 7, "expected an element name");
}

#[test]
fn unclosed_markup() {
    assert_error("<a><!-- x", 1, 4, "comment not closed");
    assert_error("<a><![CDATA[ x", 1, 4, "CDATA not closed");
    assert_error("<a><?pi x", 1, 4, "processing instruction not closed");
    assert_error("<!DOCTYPE a [ <!ENTITY x 'y'>", 1, 1, "DOCTYPE not closed");
    assert_error("<a", 1, 1, "tag not closed");
    assert_error("<a b=\"1", 1, 6, "has no closing quote");
    assert_error("<a b='1", 1, 6, "has no closing quote");
    assert_error("<a><!x></a>", 1, 4, "unknown or missed symbol");
}

#[test]
fn wrong_quote_is_found() {
    // The quote of case_2 is missing; quick-xml alone reports an unclosed tag
    // at '<'. The parser finds the real position.
    let text = "<root>\n  <S case_1=\"1\" case_2=\"42 case_3=\"666\" >\n  </S>\n</root>";
    assert_error(
        text,
        2,
        36,
        "unexpected character '6' in the start tag of <S>",
    );
}

#[test]
fn processing_instructions() {
    assert_error("<a><??></a>", 1, 6, "target name");
    assert_error("<a><?=x?></a>", 1, 6, "target name");
    assert_error("<a><?t\"x\"?></a>", 1, 7, "white space after");
    let doc = Document::parse("<a><?t x?><?t?></a>").unwrap();
    assert_eq!(doc.to_string(), "<a><?t x?><?t?></a>");
}

#[test]
fn declaration_position() {
    assert_error("<a/><?xml version='1.0'?>", 1, 5, "XML declaration");
    assert_error(
        "<!-- c --><?xml version='1.0'?><a/>",
        1,
        11,
        "XML declaration",
    );
    assert_error("<a><?xml version='1.0'?></a>", 1, 4, "XML declaration");
    // White space before the declaration is accepted (as by tinyxml2).
    let text = "\n  <?xml version='1.0'?><a/>";
    assert_eq!(Document::parse(text).unwrap().to_string(), text);
}

#[test]
fn empty_documents() {
    assert_error("", 1, 1, "empty");
    assert_error(" \r\n\t", 1, 1, "empty");
    assert_error("\u{feff}", 1, 1, "empty");
    // A document without elements is accepted, as by tinyxml2.
    let doc = Document::parse("<!-- only -->").unwrap();
    assert_eq!(doc.document_element(), None);
}

#[test]
fn columns_count_characters_and_skip_the_bom() {
    assert_error("<ä>\n<ö></ü></ä>", 2, 4, "</ü>");
    assert_error("\u{feff}<a></b>", 1, 4, "</b>");
    assert_error("\u{feff}\n<a></b>", 2, 4, "</b>");
}

#[test]
fn a_second_byte_order_mark_is_text() {
    for text in [
        "\u{feff}\u{feff}",
        "\u{feff}\u{feff}\u{feff}<a/>",
        "\u{feff}\u{feff}text<a/>",
    ] {
        let doc = Document::parse(text).unwrap();
        assert!(doc.has_bom());
        assert_eq!(doc.to_string(), text);
        let first = doc.children(doc.root())[0];
        assert!(matches!(doc.node(first), Some(Node::Text(_))));
    }
    // The extra mark is content, so a declaration cannot follow it.
    assert_error(
        "\u{feff}\u{feff}<?xml version='1.0'?><a/>",
        1,
        2,
        "XML declaration",
    );
}

#[test]
fn utf8_errors() {
    let e = Document::parse_bytes(b"<a>\n  x\xff</a>").unwrap_err();
    assert_eq!((e.line, e.column, e.offset), (2, 4, 7));
    assert!(e.message.contains("not valid UTF-8"), "{e}");
    assert!(e.message.contains("0xFF"), "{e}");
    let e = Document::parse_bytes(b"\xff\xfe<\x00a\x00/\x00>\x00").unwrap_err();
    assert!(e.message.contains("UTF-16"), "{e}");
    let e = Document::parse_bytes(b"<a>\xc3</a>").unwrap_err();
    assert_eq!((e.line, e.column), (1, 4));
    let doc = Document::parse_bytes("\u{feff}<a>é</a>".as_bytes()).unwrap();
    assert!(doc.has_bom());
    assert_eq!(doc.to_string(), "\u{feff}<a>é</a>");
}

#[test]
fn nesting_limit_of_btcpp() {
    let nested = |levels: usize, inner: &str| {
        let mut s = String::new();
        for _ in 0..levels {
            s.push_str("<e>");
        }
        s.push_str(inner);
        for _ in 0..levels {
            s.push_str("</e>");
        }
        s
    };
    // MAX_ELEMENT_DEPTH open elements are fine; an empty-element tag one
    // level deeper too.
    let ok = nested(MAX_ELEMENT_DEPTH, "<leaf/>");
    assert_eq!(Document::parse(&ok).unwrap().to_string(), ok);
    let (line, column, message) = error(&nested(MAX_ELEMENT_DEPTH + 1, ""));
    assert_eq!((line, column), (1, 3 * MAX_ELEMENT_DEPTH + 1));
    assert!(message.contains("nested too deeply"), "{message}");
}

#[test]
fn deep_input_does_not_overflow_the_stack() {
    // Far deeper than the limit: the parser stops with an error.
    let text = "<a>".repeat(200_000);
    assert!(Document::parse(&text).is_err());
}

#[test]
fn tinyxml2_leniencies_are_accepted() {
    let cases = [
        // A `&` that is not a reference stays literal.
        (
            "<a x=\"a && b\">x & y &unknown; &#xZZ;</a>",
            "x & y &unknown; &#xZZ;",
        ),
        // `<` and `>` in attribute values.
        ("<a x=\"1 < 2\">t</a>", "t"),
        // More than one top-level element, and text outside them.
        ("<a>1</a>text<b>2</b>", "1"),
        // `--` in a comment.
        ("<a><!-- a -- b -->c</a>", "c"),
        // `]]>` in text.
        ("<a>]]></a>", "]]>"),
        // No white space between attributes.
        ("<a x=\"1\"y='2'>z</a>", "z"),
    ];
    for (text, content) in cases {
        let doc = Document::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(doc.to_string(), text);
        let first = doc.document_element().unwrap();
        assert_eq!(doc.text_content(first), content, "{text}");
    }
    let doc = Document::parse("<a x=\"a && b\"/>").unwrap();
    let a = doc.document_element().unwrap();
    assert_eq!(doc.attribute(a, "x"), Some("a && b"));
}

#[test]
fn values_are_decoded_like_tinyxml2() {
    let text = "<a v=\"l1\r\nl2\rl3\n\rl4\t&#10;&#x9;&lt;&amp;lt;&#0;cut\">t\r\nu &#x41;&#65;&apos;<![CDATA[&amp;\r\n]]></a>";
    let doc = Document::parse(text).unwrap();
    let a = doc.document_element().unwrap();
    assert_eq!(doc.attribute(a, "v"), Some("l1\nl2\nl3\nl4\t\n\t<&lt;"));
    assert_eq!(
        doc.attributes(a)[0].raw_value(),
        "l1\r\nl2\rl3\n\rl4\t&#10;&#x9;&lt;&amp;lt;&#0;cut"
    );
    assert_eq!(doc.text_content(a), "t\nu AA'&amp;\n");
    match doc.node(doc.children(a)[1]) {
        Some(Node::CData(c)) => assert_eq!(c.content(), "&amp;\r\n"),
        other => panic!("{other:?}"),
    }
    assert_eq!(doc.to_string(), text);
}

#[test]
fn get_text_matches_tinyxml2() {
    let doc = Document::parse(
        "<r>\
         <a>\n  hello &amp; bye\n</a>\
         <b>\n  <!-- c -->\n  text<x/></b>\
         <c>  <![CDATA[<raw>]]></c>\
         <d>\n  <x/>after</d>\
         <e/>\
         <f>   </f>\
         </r>",
    )
    .unwrap();
    let r = doc.document_element().unwrap();
    let texts: Vec<Option<String>> = doc.element_children(r).map(|e| doc.get_text(e)).collect();
    assert_eq!(
        texts,
        [
            Some("\n  hello & bye\n".to_owned()),
            Some("\n  text".to_owned()),
            Some("<raw>".to_owned()),
            None,
            None,
            None,
        ]
    );
    assert_eq!(doc.get_text(doc.root()), None);
}

#[test]
fn doctype_with_internal_subset_and_misc_survive() {
    let text = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<!DOCTYPE root [\r\n  <!ELEMENT root ANY>\r\n  <!ENTITY e \"<x>\">\r\n]>\r\n<?pi data?>\r\n<!-- c -->\r\n<root/>\r\n<!-- after -->\r\n";
    let doc = Document::parse(text).unwrap();
    assert_eq!(doc.to_string(), text);
    let kinds: Vec<_> = doc
        .children(doc.root())
        .iter()
        .filter_map(|&n| doc.kind(n))
        .collect();
    use fasttick_core::xml::NodeKind as K;
    assert_eq!(
        kinds,
        [
            K::Declaration,
            K::Text,
            K::DocType,
            K::Text,
            K::ProcessingInstruction,
            K::Text,
            K::Comment,
            K::Text,
            K::Element,
            K::Text,
            K::Comment,
            K::Text
        ]
    );
    match doc.node(doc.children(doc.root())[2]) {
        Some(Node::DocType(d)) => assert!(d.content().starts_with("root [")),
        other => panic!("{other:?}"),
    }
}
