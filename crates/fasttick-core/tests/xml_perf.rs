//! Speed of parse and write. The time limit applies only to optimized
//! builds (`cargo test --release`).

use std::time::{Duration, Instant};

use fasttick_core::xml::{Change, Document};

/// Builds a BT.CPP file with 10,001 elements (about 1 MB).
fn ten_thousand_elements() -> String {
    let mut text = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<root BTCPP_format=\"4\">\n");
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
    eprintln!("parse + write of 10,001 elements ({} bytes): {best:?}", text.len());
    #[cfg(not(debug_assertions))]
    assert!(best < Duration::from_millis(50), "parse + write took {best:?}");
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
        groups.push(vec![doc.set_attribute(a, "name", "changed").unwrap().inverse()]);
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
