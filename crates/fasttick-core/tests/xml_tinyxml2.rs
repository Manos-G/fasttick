//! Differential test against tinyxml2, the XML reader of BT.CPP.
//!
//! The test builds a small C++ program from `3rdparty/tinyxml2` of a
//! BehaviorTree.CPP source tree. The program parses each input as BT.CPP
//! does (`XMLDocument::Parse` with the default settings) and writes the
//! tree with the decoded names and values. The test compares that output
//! with the tree of [`Document::parse`]:
//!
//! - The parser must reject every input that tinyxml2 rejects.
//! - When both accept an input, the trees and all decoded values must be
//!   equal.
//! - When only tinyxml2 accepts an input, the error must be one of the
//!   differences that `Document::parse` documents.
//!
//! The test needs a C++ compiler and the BT.CPP source, so it does not run
//! by default:
//!
//! ```sh
//! FASTTICK_BTCPP_SRC=/path/to/BehaviorTree.CPP \
//!     cargo test -p fasttick-core --test xml_tinyxml2 -- --ignored
//! ```
//!
//! `CXX` selects the compiler (default `c++`), and `FASTTICK_DIFF_CASES` the
//! number of random inputs (default 20,000).

use std::path::{Path, PathBuf};
use std::process::Command;

use fasttick_core::xml::{Document, Node, NodeId};

/// The oracle program. It writes `<file>.tx` for each file argument.
const ORACLE_CPP: &str = r#"
#include "tinyxml2.h"
#include <cstdio>
#include <fstream>
#include <sstream>
#include <string>
using namespace tinyxml2;

static void esc(std::string& out, const char* s) {
  for (const unsigned char* p = (const unsigned char*)s; *p; ++p) {
    if (*p < 0x20 || *p == '\\' || *p == '|' || *p == '=' || *p == 0x7f) {
      char buf[8];
      snprintf(buf, sizeof buf, "\\x%02X", *p);
      out += buf;
    } else {
      out += (char)*p;
    }
  }
}

static void dump(const XMLNode* n, int depth, std::string& out) {
  for (const XMLNode* c = n->FirstChild(); c; c = c->NextSibling()) {
    out += std::string(depth * 2, ' ');
    if (const XMLElement* e = c->ToElement()) {
      out += "E|";
      esc(out, e->Name());
      for (const XMLAttribute* a = e->FirstAttribute(); a; a = a->Next()) {
        out += "|";
        esc(out, a->Name());
        out += "=";
        esc(out, a->Value());
      }
      out += "\n";
      dump(c, depth + 1, out);
    } else if (const XMLText* t = c->ToText()) {
      out += t->CData() ? "C|" : "T|";
      esc(out, t->Value());
      out += "\n";
    } else if (c->ToComment()) {
      out += "M|";
      esc(out, c->Value());
      out += "\n";
    } else if (c->ToDeclaration()) {
      out += "D|";
      esc(out, c->Value());
      out += "\n";
    } else if (c->ToUnknown()) {
      out += "U|";
      esc(out, c->Value());
      out += "\n";
    }
  }
}

int main(int argc, char** argv) {
  for (int i = 1; i < argc; ++i) {
    std::ifstream f(argv[i], std::ios::binary);
    std::stringstream ss;
    ss << f.rdbuf();
    std::string s = ss.str();
    XMLDocument doc;
    std::string out;
    if (doc.Parse(s.c_str(), s.size()) != XML_SUCCESS) {
      out = std::string("ERR ") + doc.ErrorName() + "\n";
    } else {
      dump(&doc, 0, out);
    }
    std::ofstream o(std::string(argv[i]) + ".tx", std::ios::binary);
    o << out;
  }
  return 0;
}
"#;

/// Parts of our error messages for the inputs that tinyxml2 accepts and
/// `Document::parse` rejects on purpose (see its documentation).
const DOCUMENTED_DIFFERENCES: &[&str] = &[
    // A stray end tag at the top level: tinyxml2 ignores the rest.
    "has no matching start tag",
    // `<!X>`: tinyxml2 keeps an "unknown" node.
    "unknown markup after '<!'",
    // `< a>` and `</ a>`.
    "expected an element name after '<'",
    // `</a b="1">`.
    "unexpected text in the end tag",
    // `<??>`, `<?=?>`, `<?t"x"?>`.
    "processing instruction must start with a target name",
    "expected white space after the processing instruction target",
    // `<?>...?>`: quick-xml does not find the end; tinyxml2 accepts it
    // as a declaration (and the target rule above rejects it anyway).
    "processing instruction not closed",
    // `<!DOCTYPE>`.
    "does not contain a name of a document type",
    // A NUL character: tinyxml2 stops reading there.
    "NUL character",
    // A byte order mark and only white space: no root element.
    "the document is empty",
];

fn escape(out: &mut String, s: &str) {
    for c in s.chars() {
        if (c as u32) < 0x20 || matches!(c, '\\' | '|' | '=' | '\u{7f}') {
            out.push_str(&format!("\\x{:02X}", c as u32));
        } else {
            out.push(c);
        }
    }
}

/// Line-end normalization of tinyxml2 for comments, declarations and
/// unknown nodes (`NEEDS_NEWLINE_NORMALIZATION`, no entities).
fn normalize_newlines(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1)) {
            (b'\r', Some(b'\n')) | (b'\n', Some(b'\r')) => {
                out.push(b'\n');
                i += 2;
            }
            (b'\r', _) => {
                out.push(b'\n');
                i += 1;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap()
}

fn is_c_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}')
}

/// Decodes text with the rules of tinyxml2, through a small document.
fn decode_text(raw: &str) -> String {
    let doc = Document::parse(&format!("<x>{raw}</x>")).unwrap();
    let x = doc.document_element().unwrap();
    match doc.children(x).first().and_then(|&t| doc.node(t)) {
        Some(Node::Text(t)) => t.decoded().into_owned(),
        _ => String::new(),
    }
}

/// Writes the tree of `doc` in the format of the oracle.
fn dump(doc: &Document, parent: NodeId, depth: usize, out: &mut String) {
    for (index, &child) in doc.children(parent).iter().enumerate() {
        let Some(node) = doc.node(child) else {
            continue;
        };
        let mut line = String::new();
        match node {
            // tinyxml2 does not keep text with only white space.
            Node::Text(t) if t.is_whitespace() => continue,
            Node::Text(t) if parent == doc.root() && index == 0 && !doc.has_bom() => {
                // `XMLDocument::Parse` skips the white space at the start and
                // then a byte order mark. With a byte order mark at the very
                // start, nothing is skipped: `XMLDocument::Identify` keeps the
                // white space before text.
                let raw = t.raw().trim_start_matches(is_c_space);
                let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
                if raw.chars().all(is_c_space) {
                    continue;
                }
                line.push_str("T|");
                escape(&mut line, &decode_text(raw));
            }
            Node::Text(t) => {
                line.push_str("T|");
                escape(&mut line, &t.decoded());
            }
            Node::CData(c) => {
                line.push_str("C|");
                escape(&mut line, &c.decoded());
            }
            Node::Comment(c) => {
                line.push_str("M|");
                escape(&mut line, &normalize_newlines(c.content()));
            }
            Node::Declaration(d) => {
                line.push_str("D|");
                escape(
                    &mut line,
                    &normalize_newlines(&d.raw()[2..d.raw().len() - 2]),
                );
            }
            Node::ProcessingInstruction(p) => {
                line.push_str("D|");
                escape(&mut line, &normalize_newlines(p.content()));
            }
            Node::DocType(d) => {
                line.push_str("U|");
                escape(
                    &mut line,
                    &normalize_newlines(&d.raw()[2..d.raw().len() - 1]),
                );
            }
            Node::Element(e) => {
                line.push_str("E|");
                escape(&mut line, e.name());
                for a in e.attributes() {
                    line.push('|');
                    escape(&mut line, a.name());
                    line.push('=');
                    escape(&mut line, a.value());
                }
            }
            Node::Document => continue,
        }
        out.push_str(&" ".repeat(depth * 2));
        out.push_str(&line);
        out.push('\n');
        if matches!(node, Node::Element(_)) {
            dump(doc, child, depth + 1, out);
        }
    }
}

/// A small deterministic random number generator (xorshift64).
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

/// Pieces of markup and text, for random token sequences.
const TOKENS: &[&str] = &[
    "<a",
    "<b",
    "<c:d",
    ">",
    "/>",
    "</a>",
    "</b>",
    "</c:d>",
    "</a >",
    " x=\"1\"",
    " y='&amp;'",
    " z = \"<'>\"",
    " x",
    "=",
    "\"",
    "'",
    "<!--",
    "-->",
    "--",
    "<!-->",
    "<![CDATA[",
    "]]>",
    "]",
    "<?p ",
    "<?xml version='1.0'?>",
    "<?xml",
    "?>",
    "<!DOCTYPE a>",
    "<!X>",
    "<!",
    "&",
    "&amp;",
    "&lt;",
    "&#65;",
    "&#x41;",
    "&#X41;",
    "&#;",
    "&#",
    "&#x",
    "&#0;",
    "&foo;",
    "&&",
    ";",
    "#",
    "x",
    "text",
    " ",
    "\n",
    "\r",
    "\r\n",
    "\t",
    "\u{b}",
    "\u{c}",
    "\u{feff}",
    "\u{fc}",
    "<",
    "/",
    "</",
    "< a",
    "a=\"\"",
    "1",
    "\0",
];

/// Pieces of attribute values and text.
const VALUES: &[&str] = &[
    "v",
    "&amp;",
    "&lt;",
    "&gt;",
    "&quot;",
    "&apos;",
    "&",
    "&&",
    "&foo;",
    "&#65;",
    "&#x4e2d;",
    "&#X41;",
    "&#;",
    "&#",
    "\n",
    "\r\n",
    "\r",
    "\n\r",
    "\t",
    "<",
    ">",
    " ",
    "{x}",
    "&#x;",
    "&#0;",
    "&#1#65;",
    "&#x1x41;",
    "\u{fc}",
    "\u{e4}\u{e4}",
    "&#13;",
    "&#10;",
    "&#x10FFFF;",
    "&#x110000;",
];

fn token_soup(rng: &mut Rng) -> String {
    let count = rng.below(25);
    (0..count).map(|_| rng.pick(TOKENS)).collect()
}

fn element(rng: &mut Rng, depth: usize, out: &mut String) {
    let name = rng.pick(&["a", "b", "Seq", "x.y", "_z", "\u{e4}"]);
    out.push('<');
    out.push_str(name);
    for i in 0..rng.below(4) {
        let quote = if rng.below(2) == 0 { '"' } else { '\'' };
        out.push_str(rng.pick(&[" ", "\n  ", "\t", "\u{b}"]));
        out.push_str(&format!("k{i}"));
        out.push_str(rng.pick(&["=", " = "]));
        out.push(quote);
        for _ in 0..rng.below(6) {
            let v = rng.pick(VALUES);
            if !v.contains(quote) {
                out.push_str(v);
            }
        }
        out.push(quote);
    }
    if depth > 3 || rng.below(3) == 0 {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for _ in 0..rng.below(5) {
        match rng.below(6) {
            0 | 1 => element(rng, depth + 1, out),
            2 => {
                for _ in 0..rng.below(5) {
                    let v = rng.pick(VALUES);
                    if v != "<" {
                        out.push_str(v);
                    }
                }
            }
            3 => out.push_str("<!-- c\r\n -->"),
            4 => out.push_str("<![CDATA[x\r\n<&>]]>"),
            _ => out.push_str("\n  "),
        }
    }
    out.push_str("</");
    out.push_str(name);
    out.push('>');
}

fn structured(rng: &mut Rng) -> String {
    let mut s = String::new();
    if rng.below(4) == 0 {
        s.push('\u{feff}');
    }
    if rng.below(3) == 0 {
        s.push_str("<?xml version=\"1.0\"?>\r\n");
    }
    element(rng, 0, &mut s);
    if rng.below(3) == 0 {
        s.push_str(rng.pick(TOKENS));
    }
    s
}

fn fixture_files() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/btcpp");
    let mut stack = vec![root];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "xml") {
                files.push(String::from_utf8(std::fs::read(&path).unwrap()).unwrap());
            }
        }
    }
    files
}

/// Builds the oracle. Returns `None` (and prints why) if the BT.CPP source
/// or the compiler is missing.
fn build_oracle(work: &Path) -> Option<PathBuf> {
    let btcpp = std::env::var_os("FASTTICK_BTCPP_SRC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user/ref/BehaviorTree.CPP"));
    let tinyxml2 = btcpp.join("3rdparty/tinyxml2");
    if !tinyxml2.join("tinyxml2.cpp").is_file() {
        eprintln!("skipped: no tinyxml2 source in {}", btcpp.display());
        return None;
    }
    let source = work.join("oracle.cpp");
    std::fs::write(&source, ORACLE_CPP).unwrap();
    let binary = work.join("oracle");
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".to_owned());
    let status = Command::new(&cxx)
        .arg("-O1")
        .arg("-std=c++17")
        .arg("-I")
        .arg(&tinyxml2)
        .arg(&source)
        .arg(tinyxml2.join("tinyxml2.cpp"))
        .arg("-o")
        .arg(&binary)
        .status();
    match status {
        Ok(s) if s.success() => Some(binary),
        other => {
            eprintln!("skipped: {cxx} failed: {other:?}");
            None
        }
    }
}

#[test]
#[ignore = "needs a C++ compiler and the BT.CPP source (FASTTICK_BTCPP_SRC)"]
fn parser_matches_tinyxml2() {
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join("xml_tinyxml2");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let Some(oracle) = build_oracle(&work) else {
        return;
    };
    let cases: usize = std::env::var("FASTTICK_DIFF_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    let mut inputs = fixture_files();
    let fixtures = inputs.len();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for i in 0..cases {
        inputs.push(if i % 2 == 0 {
            token_soup(&mut rng)
        } else {
            structured(&mut rng)
        });
    }
    let paths: Vec<PathBuf> = inputs
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let path = work.join(format!("{i}.xml"));
            std::fs::write(&path, text).unwrap();
            path
        })
        .collect();
    for chunk in paths.chunks(1000) {
        assert!(
            Command::new(&oracle)
                .args(chunk)
                .status()
                .unwrap()
                .success()
        );
    }

    let mut failures = Vec::new();
    let mut both_accept = 0;
    for (i, (text, path)) in inputs.iter().zip(&paths).enumerate() {
        let mut tx_path = path.clone().into_os_string();
        tx_path.push(".tx");
        let expected = String::from_utf8_lossy(&std::fs::read(tx_path).unwrap()).into_owned();
        let tinyxml2_error = expected.starts_with("ERR ");
        match Document::parse(text) {
            Ok(doc) => {
                let mut ours = String::new();
                dump(&doc, doc.root(), 0, &mut ours);
                if tinyxml2_error {
                    failures.push(format!("accepted, tinyxml2: {expected}  input {text:?}"));
                } else if ours != expected {
                    failures.push(format!(
                        "trees differ for {text:?}\ntinyxml2:\n{expected}ours:\n{ours}"
                    ));
                } else {
                    both_accept += 1;
                }
            }
            Err(e) => {
                let documented = DOCUMENTED_DIFFERENCES.iter().any(|d| e.message.contains(d));
                if !tinyxml2_error && !documented {
                    failures.push(format!("rejected ({e}), tinyxml2 accepts {text:?}"));
                }
                assert!(i >= fixtures || tinyxml2_error, "fixture {i}: {e}");
            }
        }
    }
    eprintln!(
        "{} inputs ({fixtures} fixtures), {both_accept} accepted by both, {} failures",
        inputs.len(),
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{}",
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
