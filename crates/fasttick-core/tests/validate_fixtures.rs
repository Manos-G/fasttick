//! The validator on all BT.CPP 4.9.0 XML fixtures, compared with BT.CPP
//! itself.
//!
//! The expected results are in `tests/validate_fixtures/`:
//!
//! - `btcpp_rejected.txt`: the fixtures that BT.CPP rejects when it loads
//!   the file alone, with the problem code that the validator must report.
//! - `unknown_types.txt`: the fixtures that BT.CPP accepts but that use node
//!   types that only exist in the C++ code of the BT.CPP tests. The
//!   validator must report `UnknownNodeType` for exactly these types and no
//!   other error.
//!
//! All other fixtures must give no error at all.
//!
//! A scratch oracle made both lists: a C++ program that links the installed
//! BT.CPP 4.9.0. For each fixture, it registers dummy nodes with the ports
//! of the harness models (`tests/fixtures/harness/all_models.xml`), of the
//! models in the fixture, and (for the types that only exist in C++) with
//! the attributes that the fixture uses. Then it calls
//! `registerBehaviorTreeFromFile` and `createTree` for each tree.
//!
//! Each project of this test has the fixture (with its includes) and the
//! harness models as a project model file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use fasttick_core::bt::NodeView;
use fasttick_core::project::Project;
use fasttick_core::validate::{Problem, ProblemCode, Severity, validate_project};

/// The fixtures that are not well-formed XML (see `xml_roundtrip.rs`).
const MALFORMED_FIXTURES: &[&str] = &[
    "inline/gtest_factory_16.xml",
    "inline/gtest_factory_17.xml",
    "inline/gtest_switch_1.xml",
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture_root() -> PathBuf {
    workspace().join("tests/fixtures/btcpp")
}

fn harness_models() -> PathBuf {
    workspace().join("tests/fixtures/harness/all_models.xml")
}

fn data(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/validate_fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The lines of a data file without comments, split at tabs.
fn records(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect()
}

fn fixtures() -> Vec<(String, PathBuf)> {
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
                files.push((rel, path));
            }
        }
    }
    files.sort();
    files
}

fn validate_fixture(path: &Path) -> (Project, Vec<Problem>) {
    let mut project = Project::open_xml(path).unwrap_or_else(|e| panic!("{e}"));
    project.add_model_file(harness_models()).unwrap();
    let problems = validate_project(&project);
    (project, problems)
}

fn describe(project: &Project, p: &Problem) -> String {
    let line = p
        .line_col(project)
        .map(|(l, _)| format!("line {l}: "))
        .unwrap_or_default();
    let file = project
        .file(p.file)
        .map(|f| f.display_name())
        .unwrap_or_default();
    format!("{file}: {line}{p}")
}

/// The type ID of the node of an `UnknownNodeType` problem.
fn unknown_type(project: &Project, p: &Problem) -> String {
    let doc = project.file(p.file).unwrap().doc();
    NodeView::new(doc, p.node.unwrap())
        .unwrap()
        .type_id()
        .unwrap()
        .to_owned()
}

#[test]
fn fixtures_agree_with_btcpp() {
    let rejected: BTreeMap<String, ProblemCode> = records(&data("btcpp_rejected.txt"))
        .into_iter()
        .map(|r| {
            let code = ProblemCode::from_name(&r[1]).unwrap_or_else(|| panic!("{}", r[1]));
            (r[0].clone(), code)
        })
        .collect();
    let unknown: BTreeMap<String, BTreeSet<String>> = records(&data("unknown_types.txt"))
        .into_iter()
        .map(|r| {
            let types = r[1].split(' ').map(str::to_owned).collect();
            (r[0].clone(), types)
        })
        .collect();
    assert_eq!(rejected.len(), 41);
    assert_eq!(unknown.len(), 196);

    let mut failures = Vec::new();
    let mut checked = 0;
    let mut accepted = 0;
    let mut seen = BTreeSet::new();
    for (rel, path) in fixtures() {
        if MALFORMED_FIXTURES.contains(&rel.as_str()) {
            continue;
        }
        checked += 1;
        seen.insert(rel.clone());
        let (project, problems) = validate_fixture(&path);
        let errors: Vec<&Problem> = problems.iter().filter(|p| p.is_error()).collect();
        if let Some(code) = rejected.get(&rel) {
            assert!(!unknown.contains_key(&rel), "{rel} is in both lists");
            if !errors.iter().any(|p| p.code == *code) {
                failures.push(format!(
                    "{rel}: BT.CPP rejects it; expected an error {code}, got: {:#?}",
                    errors
                        .iter()
                        .map(|p| describe(&project, p))
                        .collect::<Vec<_>>()
                ));
            }
            continue;
        }
        accepted += 1;
        let expected = unknown.get(&rel).cloned().unwrap_or_default();
        let mut reported = BTreeSet::new();
        for p in &errors {
            if p.code == ProblemCode::UnknownNodeType {
                reported.insert(unknown_type(&project, p));
            } else {
                failures.push(format!(
                    "{rel}: BT.CPP accepts it, but: {}",
                    describe(&project, p)
                ));
            }
        }
        if reported != expected {
            failures.push(format!(
                "{rel}: unknown types {reported:?}, expected {expected:?}"
            ));
        }
    }
    assert_eq!(checked, 314);
    assert_eq!(accepted, 273);
    for rel in rejected.keys().chain(unknown.keys()) {
        assert!(seen.contains(rel), "{rel} is not a fixture");
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The fixtures give warnings and notes too; this test prints a summary
/// and checks that each problem has a message that ends with a full stop.
#[test]
fn fixture_messages_are_sentences() {
    let mut by_code: BTreeMap<(Severity, ProblemCode), usize> = BTreeMap::new();
    for (rel, path) in fixtures() {
        if MALFORMED_FIXTURES.contains(&rel.as_str()) {
            continue;
        }
        let (_, problems) = validate_fixture(&path);
        for p in problems {
            assert!(
                p.message.ends_with('.') || p.message.ends_with(".\""),
                "{rel}: {}",
                p.message
            );
            assert!(!p.message.contains("  "), "{rel}: {}", p.message);
            *by_code.entry((p.severity, p.code)).or_default() += 1;
        }
    }
    for ((severity, code), n) in &by_code {
        println!("{severity:7} {code:22} {n}");
    }
}
