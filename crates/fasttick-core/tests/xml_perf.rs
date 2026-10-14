//! Speed of parse and write. The time limit applies only to optimized
//! builds (`cargo test --release`).

use std::time::{Duration, Instant};

use fasttick_core::xml::{Change, Document};

/// Builds a BT.CPP file with 10,001 elements (about 1 MB).
fn ten_thousand_elements() -> String {
    let mut text =
        String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<root BTCPP_format=\"4\">\n");
    for t in 0..100 {
        text.push_str(&format!(
            "    <BehaviorTree ID=\"Tree{t}\">\n        <!-- tree {t} -->\n        <Sequence name=\"s{t}\">\n"
        ));
        for i in 0..98 {
            text.push_str(&format!(
                "            <Action ID=\"A{i}\" name='n &amp; {i}' value=\"{{bb_{i}}}\" _skipIf=\"x &lt; {i}\"/>\n"
            ));
        }
        text.push_str("        </Sequence>\n    </BehaviorTree>\n");
    }
    text.push_str("</root>\n");
    text
}

#[test]
fn parse_and_write_ten_thousand_elements() {
    let text = ten_thousand_elements();
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let start = Instant::now();
        let doc = Document::parse(&text).unwrap();
        let out = doc.to_string();
        best = best.min(start.elapsed());
        assert_eq!(out, text);
        let elements = doc
            .descendants(doc.root())
            .filter(|&n| doc.is_element(n))
            .count();
        assert_eq!(elements, 10_001);
    }
    eprintln!(
        "parse + write of 10,001 elements ({} bytes): {best:?}",
        text.len()
    );
    #[cfg(not(debug_assertions))]
    assert!(
        best < Duration::from_millis(50),
        "parse + write took {best:?}"
    );
}

#[test]
fn edits_on_a_large_document_stay_fast() {
    let text = ten_thousand_elements();
    let mut doc = Document::parse(&text).unwrap();
    let actions: Vec<_> = doc
        .descendants(doc.root())
        .filter(|&n| doc.name(n) == Some("Action"))
        .take(1000)
        .collect();
    let start = Instant::now();
    // Each group holds the changes that undo one edit, in their order.
    let mut groups: Vec<Vec<Change>> = Vec::new();
    for &a in &actions {
        groups.push(vec![
            doc.set_attribute(a, "name", "changed").unwrap().inverse(),
        ]);
        groups.push(doc.remove_element_pretty(a).unwrap());
    }
    for group in groups.into_iter().rev() {
        doc.apply_all(group).unwrap();
    }
    let elapsed = start.elapsed();
    assert_eq!(doc.to_string(), text);
    eprintln!("1,000 edits and their undo: {elapsed:?}");
    #[cfg(not(debug_assertions))]
    assert!(elapsed < Duration::from_millis(500), "{elapsed:?}");
}

/// Parses `text` and returns the time. In optimized builds the time must be
/// below `limit`.
fn timed_parse(text: &str, limit: Duration) -> Result<Document, fasttick_core::xml::XmlError> {
    let start = Instant::now();
    let result = Document::parse(text);
    let elapsed = start.elapsed();
    eprintln!("{} bytes: {elapsed:?}", text.len());
    #[cfg(not(debug_assertions))]
    assert!(elapsed < limit, "{elapsed:?}");
    #[cfg(debug_assertions)]
    let _ = limit;
    result
}

#[test]
fn wide_and_unusual_inputs_parse_in_linear_time() {
    let limit = Duration::from_millis(500);
    // 50,000 siblings, indented.
    let mut wide = String::from("<root>\n");
    for i in 0..50_000 {
        wide.push_str(&format!("  <a i=\"{i}\"/>\n"));
    }
    wide.push_str("</root>\n");
    let doc = timed_parse(&wide, limit).unwrap();
    assert_eq!(doc.indent_unit(), "  ");
    assert_eq!(doc.to_string(), wide);

    // One tag with 20,000 attributes.
    let mut attrs = String::from("<a");
    for i in 0..20_000 {
        attrs.push_str(&format!(" k{i}=\"{i}\""));
    }
    attrs.push_str("/>");
    assert_eq!(timed_parse(&attrs, limit).unwrap().to_string(), attrs);
    let duplicate = attrs.replace("/>", " k19999=\"x\"/>");
    let err = timed_parse(&duplicate, limit).unwrap_err();
    assert!(err.message.contains("duplicate"), "{err}");

    // 50,000 declarations (tinyxml2 accepts more than one).
    let decls = "<?xml version=\"1.0\"?>".repeat(50_000) + "<a/>";
    assert_eq!(timed_parse(&decls, limit).unwrap().to_string(), decls);

    // Long text with many references.
    let text = format!("<a>{}</a>", "x &amp; y &#65; & ".repeat(50_000));
    let doc = timed_parse(&text, limit).unwrap();
    assert_eq!(doc.to_string(), text);
    let a = doc.document_element().unwrap();
    assert!(doc.text_content(a).starts_with("x & y A & x"));
}
