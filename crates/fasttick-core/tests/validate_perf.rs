//! Speed of the validator on a project with more than 5,000 nodes. The time
//! limit applies only to optimized builds (`cargo test --release`).

use std::path::Path;
use std::time::{Duration, Instant};

use fasttick_core::project::Project;
use fasttick_core::validate::{ProblemCode, Validator, validate_project};

/// The number of trees in each file and of files.
const TREES_PER_FILE: usize = 10;
const LIBRARIES: usize = 4;

/// The time limit for one validation of the project (release builds).
const LIMIT: Duration = Duration::from_millis(10);

/// One tree of exactly 100 nodes: a `Sequence` with 9 `Fallback` nodes,
/// each with 10 leaves (custom nodes of the harness, built-in nodes and
/// SubTrees with remapping). With `problems`, every 10th leaf has a
/// problem.
fn tree(file: &str, index: usize, problems: bool) -> String {
    let id = format!("{file}_{index}");
    let mut text = format!("  <BehaviorTree ID=\"{id}\">\n    <Sequence name=\"{id}\">\n");
    for group in 0..9 {
        text.push_str(&format!(
            "      <Fallback name=\"g{group}\" _skipIf=\"skip_{group} == true\">\n"
        ));
        for leaf in 0..10 {
            let n = group * 10 + leaf;
            let node = match leaf {
                0 if problems => format!("<SayMessage mesage=\"{{msg_{n}}}\"/>"),
                0 => format!("<SayMessage message=\"{{msg_{n}}}\"/>"),
                1 => format!("<CheckFlag name=\"check {n}\" flag=\"{{flag_{n}}}\"/>"),
                2 => format!("<IncrementCounter step=\"2\" counter=\"{{counter_{n}}}\"/>"),
                3 => "<Sleep msec=\"10\" _onSuccess=\"done := true\"/>".to_owned(),
                4 if index + 1 < TREES_PER_FILE => format!(
                    "<SubTree ID=\"{file}_{}\" name=\"sub{n}\" goal=\"{{goal_{n}}}\"/>",
                    index + 1
                ),
                5 => format!("<WriteValues int_value=\"{{int_{n}}}\" string_value=\"=\"/>"),
                6 => "<Script code=\"x := x + 1\"/>".to_owned(),
                7 => format!("<Action ID=\"WaitTicks\" ticks=\"{n}\"/>"),
                8 => "<AlwaysFailure/>".to_owned(),
                _ => "<AlwaysSuccess/>".to_owned(),
            };
            text.push_str(&format!("        {node}\n"));
        }
        text.push_str("      </Fallback>\n");
    }
    text.push_str("    </Sequence>\n  </BehaviorTree>\n");
    text
}

/// A file with [`TREES_PER_FILE`] trees and a SubTree model for each tree.
fn file(name: &str, includes: &[String], problems: bool) -> String {
    let mut text = format!("<root BTCPP_format=\"4\" main_tree_to_execute=\"{name}_0\">\n");
    for include in includes {
        text.push_str(&format!("  <include path=\"{include}\"/>\n"));
    }
    text.push_str("  <TreeNodesModel>\n");
    for t in 0..TREES_PER_FILE {
        text.push_str(&format!(
            "    <SubTree ID=\"{name}_{t}\"><input_port name=\"goal\"/></SubTree>\n"
        ));
    }
    text.push_str("  </TreeNodesModel>\n");
    for t in 0..TREES_PER_FILE {
        text.push_str(&tree(name, t, problems));
    }
    text.push_str("</root>\n");
    text
}

/// Writes the project files and opens the main file with the harness
/// models.
fn large_project(dir: &Path, problems: bool) -> Project {
    let libraries: Vec<String> = (0..LIBRARIES).map(|i| format!("lib{i}.xml")).collect();
    for (i, name) in libraries.iter().enumerate() {
        std::fs::write(dir.join(name), file(&format!("Lib{i}"), &[], problems)).unwrap();
    }
    std::fs::write(dir.join("main.xml"), file("Main", &libraries, problems)).unwrap();
    let mut project = Project::open_xml(dir.join("main.xml")).unwrap();
    project
        .add_model_file(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/harness/all_models.xml"),
        )
        .unwrap();
    project
}

fn node_count(project: &Project) -> usize {
    project
        .files()
        .map(|(_, _, f)| {
            let doc = f.doc();
            f.trees()
                .iter()
                .map(|t| {
                    doc.descendants(t.element)
                        .skip(1)
                        .filter(|&n| doc.is_element(n))
                        .count()
                })
                .sum::<usize>()
        })
        .sum()
}

/// The shortest time of several runs.
fn best_time(project: &Project) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..7 {
        let start = Instant::now();
        let problems = validate_project(project);
        best = best.min(start.elapsed());
        std::hint::black_box(problems);
    }
    best
}

#[test]
fn validation_of_5000_nodes_is_fast() {
    let dir = tempfile::tempdir().unwrap();
    let project = large_project(dir.path(), false);
    let nodes = node_count(&project);
    assert!(nodes >= 5_000, "{nodes} nodes");
    let problems = validate_project(&project);
    assert!(
        problems.is_empty(),
        "{:#?}",
        &problems[..problems.len().min(5)]
    );

    let clean = best_time(&project);
    // Preparation and file validation separately (the GUI validates the
    // file that changed).
    let start = Instant::now();
    let validator = Validator::new(&project);
    let prepare = start.elapsed();
    let main = project
        .file_id_by_path(dir.path().join("main.xml"))
        .unwrap();
    let start = Instant::now();
    std::hint::black_box(validator.run_file(main));
    let one_file = start.elapsed();

    // The same project with one problem in every 10th leaf.
    let dir = tempfile::tempdir().unwrap();
    let project = large_project(dir.path(), true);
    let problems = validate_project(&project);
    let unknown = problems
        .iter()
        .filter(|p| p.code == ProblemCode::UnknownPort)
        .count();
    assert_eq!(unknown, (LIBRARIES + 1) * TREES_PER_FILE * 9);
    let with_problems = best_time(&project);

    println!(
        "{nodes} nodes: validate_project {clean:?} (prepare {prepare:?}, main file {one_file:?}); \
         with {} problems {with_problems:?}",
        problems.len()
    );
    if !cfg!(debug_assertions) {
        assert!(clean < LIMIT, "{clean:?}");
        assert!(with_problems < LIMIT, "{with_problems:?}");
    }
}
