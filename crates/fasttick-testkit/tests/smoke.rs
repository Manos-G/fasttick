//! Smoke tests of the C++ test harness.
//!
//! Each test skips cleanly when BT.CPP 4.x is not found. Set
//! `FASTTICK_BTCPP_PREFIX` to the install prefix to run them.
//!
//! With `FASTTICK_UPDATE_FIXTURES=1`, the tests also write new samples to
//! `tests/fixtures/harness/` (models XML and `main.btlog`).

use std::fs;
use std::net::{Ipv4Addr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fasttick_testkit::{
    Executor, ExecutorOptions, Harness, Signal, TempDir, fixture_path, free_port_pair, harness,
    require_harness,
};

/// The IDs of the custom nodes that `registerFastTickTestNodes` registers.
const CUSTOM_NODE_IDS: [&str; 7] = [
    "SayMessage",
    "WaitTicks",
    "CheckFlag",
    "WriteValues",
    "IncrementCounter",
    "ReadPose",
    "AnnotatedAction",
];

/// Magic text at the start of a FileLogger2 file
/// (BT.CPP src/loggers/bt_file_logger_v2.cpp).
const BTLOG_MAGIC: &[u8] = b"BTCPP4-FileLogger2";

/// Magic text at the start of an SQLite database file.
const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

const RUN_TIMEOUT: Duration = Duration::from_secs(60);

fn update_fixtures() -> bool {
    std::env::var_os("FASTTICK_UPDATE_FIXTURES").is_some_and(|value| value == "1")
}

fn write_fixture(name: &str, bytes: &[u8]) {
    let path = fixture_path(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    eprintln!("updated fixture {}", path.display());
}

/// All XML files under `dir`, sorted.
fn xml_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "xml") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn assert_no_error_output(executor: &Executor) {
    let stderr = executor.stderr();
    assert!(
        !stderr.to_lowercase().contains("error"),
        "unexpected stderr:\n{stderr}"
    );
}

#[test]
fn harness_builds_once() {
    let harness = require_harness!();
    assert!(harness.executor_path.is_file());
    assert!(harness.dump_models_path.is_file());
    assert!(harness.tree("main.xml").is_file());
    assert!(harness.lib_dir.is_dir());
    assert!(
        harness
            .build_dir
            .starts_with(fasttick_testkit::workspace_root().join("target"))
    );
    // The second call gives the same instance; no second build.
    let again = fasttick_testkit::harness().unwrap();
    assert!(std::ptr::eq(harness, again));
}

#[test]
fn dump_models_lists_the_custom_nodes() {
    let harness = require_harness!();
    let custom = harness.dump_models(false).unwrap();
    assert!(custom.contains("<TreeNodesModel>"), "{custom}");
    for id in CUSTOM_NODE_IDS {
        assert!(
            custom.contains(&format!("ID=\"{id}\"")),
            "{id} missing:\n{custom}"
        );
    }
    assert!(custom.contains("<MetadataFields>"), "{custom}");
    assert!(custom.contains("<inout_port name=\"counter\""), "{custom}");
    assert!(custom.contains("default=\"hello\""), "{custom}");
    assert!(
        !custom.contains("ID=\"Sequence\""),
        "built-in node in:\n{custom}"
    );

    let all = harness.dump_models(true).unwrap();
    assert!(all.contains("<TreeNodesModel>"));
    for id in CUSTOM_NODE_IDS
        .iter()
        .chain(&["Sequence", "Fallback", "SubTree", "Script"])
    {
        assert!(all.contains(&format!("ID=\"{id}\"")), "{id} missing");
    }

    if update_fixtures() {
        write_fixture("custom_models.xml", custom.as_bytes());
        write_fixture("all_models.xml", all.as_bytes());
    }
    // The fixture must have the same nodes and ports as the real output.
    // The comparison ignores the order and the text details, because other
    // BT.CPP 4.x versions can format default values differently.
    let fixture = fs::read_to_string(fixture_path("custom_models.xml")).unwrap();
    assert_eq!(
        model_signature(&fixture),
        model_signature(&custom),
        "tests/fixtures/harness/custom_models.xml is out of date; \
         run the smoke tests with FASTTICK_UPDATE_FIXTURES=1"
    );
}

/// Gives a sorted list of `NodeId` and `NodeId/port_kind/port_name` entries
/// for model XML in the format of `writeTreeNodesModelXML` (one element on
/// each line).
fn model_signature(xml: &str) -> Vec<String> {
    fn attribute<'a>(line: &'a str, name: &str) -> Option<&'a str> {
        let start = line.find(&format!(" {name}=\""))? + name.len() + 3;
        let end = line[start..].find('"')? + start;
        Some(&line[start..end])
    }
    const NODE_TAGS: [&str; 5] = ["Action", "Condition", "Control", "Decorator", "SubTree"];
    const PORT_TAGS: [&str; 3] = ["input_port", "output_port", "inout_port"];

    let mut entries = Vec::new();
    let mut node = String::new();
    for line in xml.lines().map(str::trim) {
        let Some(tag) = line
            .strip_prefix('<')
            .and_then(|rest| rest.split([' ', '>', '/']).next())
        else {
            continue;
        };
        if NODE_TAGS.contains(&tag) {
            node = attribute(line, "ID").unwrap_or_default().to_owned();
            entries.push(format!("{tag}:{node}"));
        } else if PORT_TAGS.contains(&tag) {
            let name = attribute(line, "name").unwrap_or_default();
            entries.push(format!("{node}/{tag}/{name}"));
        }
    }
    entries.sort();
    entries
}

#[test]
fn model_signature_ignores_order() {
    let a = "<Action ID=\"A\">\n<input_port name=\"x\" default=\"1.000000\"/>\n\
             <output_port name=\"y\"/>\n</Action>";
    let b = "<Action ID=\"A\">\n<output_port name=\"y\"/>\n\
             <input_port name=\"x\" default=\"1\"/>\n</Action>";
    assert_eq!(model_signature(a), model_signature(b));
    assert_eq!(
        model_signature(a),
        ["A/input_port/x", "A/output_port/y", "Action:A"]
    );
    assert_ne!(model_signature(a), model_signature("<Action ID=\"A\"/>"));
}

#[test]
fn every_tree_runs_once_and_writes_a_btlog() {
    let harness = require_harness!();
    let temp = TempDir::new("smoke-trees").unwrap();
    let trees = xml_files(&harness.trees_dir);
    assert!(trees.len() >= 4, "too few trees: {trees:?}");

    for (index, xml) in trees.iter().enumerate() {
        let long_running = xml
            .file_name()
            .is_some_and(|name| name == "long_running.xml");
        let btlog = temp.join(format!("run{index}.btlog"));
        let mut options = ExecutorOptions::new(xml);
        options.loops = 1;
        options.port = 0;
        options.btlog = Some(btlog.clone());
        if long_running {
            // This tree never completes. The time limit ends the run.
            options.duration_ms = 1000;
        }
        let mut executor =
            Executor::spawn(options).unwrap_or_else(|error| panic!("{}: {error}", xml.display()));
        assert_eq!(executor.port(), 0);
        let status = executor
            .wait_done(RUN_TIMEOUT)
            .unwrap_or_else(|error| panic!("{}: {error}", xml.display()));
        assert_eq!(status.code(), Some(0));
        assert_no_error_output(&executor);
        if long_running {
            assert!(executor.results().is_empty(), "{:?}", executor.results());
        } else {
            assert_eq!(executor.results(), ["SUCCESS"], "{}", xml.display());
        }

        let bytes = fs::read(&btlog).unwrap();
        assert!(
            bytes.starts_with(BTLOG_MAGIC),
            "{} does not start with the FileLogger2 magic",
            btlog.display()
        );
        // Header: magic, version byte, XML length, XML, start time.
        assert!(bytes.len() > BTLOG_MAGIC.len() + 1 + 4 + 8);
        assert_eq!(bytes[BTLOG_MAGIC.len()], 1, "version byte");

        if xml.file_name().is_some_and(|name| name == "main.xml") {
            if update_fixtures() {
                write_fixture("main.btlog", &bytes);
            }
            // The fixture must come from the current main.xml: same nodes,
            // names and UIDs. Attribute text and the built-in models can
            // differ between BT.CPP 4.x versions, so they are not compared.
            let fixture = fs::read(fixture_path("main.btlog")).unwrap();
            assert_eq!(
                runtime_tree_signature(btlog_xml(&fixture)),
                runtime_tree_signature(btlog_xml(&bytes)),
                "tests/fixtures/harness/main.btlog is out of date; \
                 run the smoke tests with FASTTICK_UPDATE_FIXTURES=1"
            );
        }
    }
}

/// Gives the XML text of a `.btlog` file (FileLogger2 layout: magic,
/// version byte, XML length as i32 LE, XML).
fn btlog_xml(bytes: &[u8]) -> &str {
    let length_at = BTLOG_MAGIC.len() + 1;
    let length = i32::from_le_bytes(bytes[length_at..length_at + 4].try_into().unwrap());
    let start = length_at + 4;
    std::str::from_utf8(&bytes[start..start + usize::try_from(length).unwrap()]).unwrap()
}

/// Gives `(tag, name, _uid)` for each node line of a runtime tree XML, in
/// document order. SubTree nodes have `_fullpath` instead of `name`.
fn runtime_tree_signature(xml: &str) -> Vec<(String, String, String)> {
    fn attribute(line: &str, name: &str) -> String {
        line.find(&format!(" {name}=\""))
            .map(|start| &line[start + name.len() + 3..])
            .and_then(|rest| rest.find('"').map(|end| rest[..end].to_owned()))
            .unwrap_or_default()
    }
    xml.lines()
        .map(str::trim)
        .filter(|line| line.contains(" _uid=\""))
        .map(|line| {
            let tag = line
                .trim_start_matches('<')
                .split([' ', '>', '/'])
                .next()
                .unwrap_or_default()
                .to_owned();
            let name = if tag == "SubTree" {
                attribute(line, "_fullpath")
            } else {
                attribute(line, "name")
            };
            (tag, name, attribute(line, "_uid"))
        })
        .collect()
}

#[test]
fn runtime_tree_signature_reads_nodes() {
    let xml = "<root>\n<BehaviorTree ID=\"M\" _fullpath=\"\">\n\
               <Sequence name=\"s\" _uid=\"1\">\n\
               <SubTree ID=\"N\" _fullpath=\"nav\" _uid=\"2\" goal=\"{g}\"/>\n\
               <Action name=\"a\" _uid=\"3\" x=\"1\"/>\n</Sequence>\n</BehaviorTree>\n</root>";
    let signature = runtime_tree_signature(xml);
    let expected = [
        ("Sequence", "s", "1"),
        ("SubTree", "nav", "2"),
        ("Action", "a", "3"),
    ];
    assert_eq!(signature.len(), expected.len());
    for (entry, (tag, name, uid)) in signature.iter().zip(expected) {
        assert_eq!(
            (entry.0.as_str(), entry.1.as_str(), entry.2.as_str()),
            (tag, name, uid)
        );
    }
}

#[test]
fn fixtures_are_valid_samples() {
    // This test does not need BT.CPP.
    let btlog = fs::read(fixture_path("main.btlog")).unwrap();
    assert!(btlog.starts_with(BTLOG_MAGIC));
    let length_at = BTLOG_MAGIC.len() + 1;
    let xml_length = i32::from_le_bytes(btlog[length_at..length_at + 4].try_into().unwrap());
    let xml_length = usize::try_from(xml_length).unwrap();
    let records_at = length_at + 4 + xml_length + 8;
    assert!(btlog.len() >= records_at);
    assert_eq!(
        (btlog.len() - records_at) % 9,
        0,
        "9-byte transition records"
    );
    let xml = btlog_xml(&btlog);
    assert!(xml.contains("_uid="), "runtime XML has UIDs");
    // main.xml has 43 nodes; UIDs are 1..=43 in document order.
    let signature = runtime_tree_signature(xml);
    assert_eq!(signature.len(), 43);
    let mut uids: Vec<u16> = signature
        .iter()
        .map(|entry| entry.2.parse().unwrap())
        .collect();
    uids.sort_unstable();
    assert_eq!(uids, (1..=43).collect::<Vec<u16>>());
    // Each record has a known UID and a status code of BT.CPP NodeStatus
    // (0 IDLE, 1 RUNNING, 2 SUCCESS, 3 FAILURE, 4 SKIPPED).
    for record in btlog[records_at..].chunks_exact(9) {
        let uid = u16::from_le_bytes([record[6], record[7]]);
        assert!((1..=43).contains(&uid), "uid {uid}");
        assert!(record[8] <= 4, "status {}", record[8]);
    }

    let custom = fs::read_to_string(fixture_path("custom_models.xml")).unwrap();
    let all = fs::read_to_string(fixture_path("all_models.xml")).unwrap();
    for id in CUSTOM_NODE_IDS {
        assert!(custom.contains(&format!("ID=\"{id}\"")));
        assert!(all.contains(&format!("ID=\"{id}\"")));
    }
    assert!(all.contains("ID=\"ReactiveSequence\""));
}

#[test]
fn sqlite_logger_writes_a_database() {
    let harness = require_harness!();
    let temp = TempDir::new("smoke-sqlite").unwrap();
    let db = temp.join("main.db3");
    let mut options = ExecutorOptions::new(harness.tree("main.xml"));
    options.loops = 1;
    options.tick_ms = 1;
    options.sqlite = Some(db.clone());
    let mut executor = match Executor::spawn(options) {
        Ok(executor) => executor,
        Err(error) if error.to_string().contains("has no SqliteLogger") => {
            eprintln!("SKIPPED: this BT.CPP build has no SqliteLogger");
            return;
        }
        Err(error) => panic!("{error}"),
    };
    executor.wait_done(RUN_TIMEOUT).unwrap();
    let bytes = fs::read(&db).unwrap();
    assert!(bytes.starts_with(SQLITE_MAGIC));
}

#[test]
fn long_running_tree_stops_cleanly_on_sigterm() {
    let harness = require_harness!();
    let port = free_port_pair();
    let mut options = ExecutorOptions::new(harness.tree("long_running.xml"));
    options.port = port;
    options.heartbeat_ms = Some(2000);
    let mut executor = Executor::spawn(options).unwrap();
    assert_eq!(executor.port(), port);
    assert!(
        executor
            .stdout_lines()
            .contains(&format!("READY port={port}"))
    );

    // The publisher listens on port (requests) and port + 1 (notifications).
    TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    TcpStream::connect((Ipv4Addr::LOCALHOST, port + 1)).unwrap();

    std::thread::sleep(Duration::from_millis(300));
    assert!(executor.is_running().unwrap());
    let status = executor.stop().unwrap();
    assert!(status.success(), "{status}; stderr:\n{}", executor.stderr());
    assert!(
        executor
            .stdout_lines()
            .iter()
            .any(|line| line == "STOPPED signal=15"),
        "{:?}",
        executor.stdout_lines()
    );
    assert!(!executor.saw_done());
    assert_no_error_output(&executor);
}

#[test]
fn sigint_also_stops_cleanly() {
    let harness = require_harness!();
    let temp = TempDir::new("smoke-sigint").unwrap();
    let btlog = temp.join("long.btlog");
    let mut options = ExecutorOptions::new(harness.tree("long_running.xml"));
    options.btlog = Some(btlog.clone());
    let mut executor = Executor::spawn(options).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let status = executor.stop_with(Signal::Int).unwrap();
    assert!(status.success(), "{status}");
    assert!(
        executor
            .stdout_lines()
            .iter()
            .any(|line| line == "STOPPED signal=2")
    );
    // The logger was destroyed before the exit, so the file is complete.
    assert!(fs::read(&btlog).unwrap().starts_with(BTLOG_MAGIC));
}

#[test]
fn duration_limit_ends_with_done() {
    let harness = require_harness!();
    let mut options = ExecutorOptions::new(harness.tree("long_running.xml"));
    options.duration_ms = 300;
    options.tick_ms = 5;
    let mut executor = Executor::spawn(options).unwrap();
    executor.wait_done(RUN_TIMEOUT).unwrap();
}

#[test]
fn loops_run_the_tree_several_times() {
    let harness = require_harness!();
    // A relative path is relative to the trees directory.
    let mut options = ExecutorOptions::new("include_main.xml");
    options.loops = 3;
    options.tick_ms = 1;
    let mut executor = Executor::spawn_with(harness, options).unwrap();
    executor.wait_done(RUN_TIMEOUT).unwrap();
    assert_eq!(executor.results(), ["SUCCESS", "SUCCESS", "SUCCESS"]);
}

#[test]
fn tree_option_selects_another_tree() {
    let harness = require_harness!();
    let mut options = ExecutorOptions::new(harness.tree("main.xml"));
    options.tree = Some("SecondTree".to_owned());
    options.loops = 1;
    let mut executor = Executor::spawn(options).unwrap();
    executor.wait_done(RUN_TIMEOUT).unwrap();
    assert_eq!(executor.results(), ["SUCCESS"]);
}

#[test]
fn executor_reports_errors() {
    let harness: &Harness = require_harness!();
    let temp = TempDir::new("smoke-errors").unwrap();

    let missing = Executor::spawn(ExecutorOptions::new(temp.join("missing.xml"))).unwrap_err();
    assert!(
        missing.to_string().contains("XML file not found"),
        "{missing}"
    );

    let mut options = ExecutorOptions::new(harness.tree("main.xml"));
    options.tree = Some("NoSuchTree".to_owned());
    let unknown = Executor::spawn(options).unwrap_err();
    assert!(unknown.to_string().contains("NoSuchTree"), "{unknown}");

    let mut options = ExecutorOptions::new(harness.tree("main.xml"));
    options.btlog = Some(temp.join("wrong_extension.log"));
    let extension = Executor::spawn(options).unwrap_err();
    assert!(extension.to_string().contains(".btlog"), "{extension}");

    let broken = temp.join("broken.xml");
    fs::write(
        &broken,
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"A\"><Nope/>",
    )
    .unwrap();
    let parse = Executor::spawn(ExecutorOptions::new(&broken)).unwrap_err();
    assert!(parse.to_string().contains("error"), "{parse}");

    // Usage errors give exit code 2.
    let output = harness.command(&harness.executor_path).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--xml"));
    let output = harness
        .command(&harness.executor_path)
        .args(["--xml", "x.xml", "--port", "65535"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let output = harness
        .command(&harness.executor_path)
        .arg("--help")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("--heartbeat-ms"));
}

#[test]
fn harness_errors_are_consistent() {
    // harness() gives the same answer on each call, also when it skips.
    match (harness(), harness()) {
        (Ok(first), Ok(second)) => assert!(std::ptr::eq(first, second)),
        (Err(first), Err(second)) => assert_eq!(first, second),
        _ => panic!("harness() changed its answer"),
    }
}

/// Runs the executor on an XML text in a temporary directory.
fn run_xml_text(harness: &Harness, temp: &TempDir, name: &str, xml: &str, loops: u64) -> Executor {
    let path = temp.join(name);
    fs::write(&path, xml).unwrap();
    let mut options = ExecutorOptions::new(&path);
    options.loops = loops;
    options.tick_ms = 1;
    let mut executor = Executor::spawn_with(harness, options).unwrap();
    executor.wait_done(RUN_TIMEOUT).unwrap();
    executor
}

#[test]
fn loops_do_not_run_on_halted_scripts_of_idle_nodes() {
    // Tree::haltTree() calls haltNode() on each node, and haltNode() always
    // runs _onHalted. The executor must not do that between two runs: the
    // node "never_halted" completes and is never halted.
    let harness = require_harness!();
    let temp = TempDir::new("smoke-halted").unwrap();
    let xml = r#"<root BTCPP_format="4" main_tree_to_execute="T">
  <BehaviorTree ID="T">
    <Sequence>
      <IncrementCounter counter="{run}"/>
      <Script code="halted:='no'" _skipIf="run != 1"/>
      <AlwaysSuccess name="never_halted" _onHalted="halted:='yes'"/>
      <ScriptCondition code="halted == 'no'"/>
    </Sequence>
  </BehaviorTree>
</root>"#;
    let executor = run_xml_text(harness, &temp, "halted.xml", xml, 3);
    assert_eq!(executor.results(), ["SUCCESS", "SUCCESS", "SUCCESS"]);
}

#[test]
fn halted_node_runs_its_on_halted_script_in_each_run() {
    // In main.xml the Parallel node halts "long_wait" in each run.
    let harness = require_harness!();
    let temp = TempDir::new("smoke-halted-real").unwrap();
    let xml = r#"<root BTCPP_format="4">
  <BehaviorTree ID="T">
    <Sequence>
      <Script code="halted:=0"/>
      <Parallel success_count="1" failure_count="1">
        <WaitTicks ticks="1"/>
        <WaitTicks ticks="100" _onHalted="halted:=halted + 1"/>
      </Parallel>
      <ScriptCondition code="halted == 1"/>
    </Sequence>
  </BehaviorTree>
</root>"#;
    let executor = run_xml_text(harness, &temp, "halted_real.xml", xml, 2);
    assert_eq!(executor.results(), ["SUCCESS", "SUCCESS"]);
}

#[test]
fn default_tree_is_main_tree_to_execute_else_the_first_in_the_file() {
    let harness = require_harness!();
    let temp = TempDir::new("smoke-default-tree").unwrap();
    // BT.CPP keeps its trees in a sorted map; "Alpha" comes first there.
    // The executor must use the document order: "Zed".
    let trees = r#"<BehaviorTree ID="Zed"><AlwaysSuccess/></BehaviorTree>
  <BehaviorTree ID="Alpha"><AlwaysFailure/></BehaviorTree>"#;
    let first = format!("<root BTCPP_format=\"4\">\n  {trees}\n</root>");
    let executor = run_xml_text(harness, &temp, "first.xml", &first, 1);
    assert_eq!(executor.results(), ["SUCCESS"]);

    let main =
        format!("<root BTCPP_format=\"4\" main_tree_to_execute=\"Alpha\">\n  {trees}\n</root>");
    let executor = run_xml_text(harness, &temp, "main.xml", &main, 1);
    assert_eq!(executor.results(), ["FAILURE"]);

    // Comments, a DOCTYPE, CDATA, quotes and references do not confuse the
    // scan. The XML parser of BT.CPP decodes "Z&#101;d" and "&#x5A;ed" to
    // "Zed", so the scan must do the same.
    for reference in ["Z&#101;d", "&#x5A;ed", "Ze&#x64;"] {
        let tricky = format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE root>\n\
             <!-- <BehaviorTree ID=\"Alpha\"> -->\n\
             <root x='a&gt;b \"q\"' BTCPP_format=\"4\" main_tree_to_execute=\"{reference}\">\n\
             <![CDATA[ <BehaviorTree ID=\"Alpha\"> ]]>\n  {trees}\n</root>"
        );
        let executor = run_xml_text(harness, &temp, "tricky.xml", &tricky, 1);
        assert_eq!(executor.results(), ["SUCCESS"], "{reference}");
    }
    // Without main_tree_to_execute, a comment before the trees is skipped.
    let commented = format!(
        "<root BTCPP_format=\"4\">\n<!-- <BehaviorTree ID=\"Alpha\"/> -->\n  {trees}\n</root>"
    );
    let executor = run_xml_text(harness, &temp, "commented.xml", &commented, 1);
    assert_eq!(executor.results(), ["SUCCESS"]);
}

#[test]
fn relative_xml_path_without_directory_works() {
    // XMLParser::loadFromFile calls absolute(parent_path()), which throws
    // for an empty parent path. The executor makes the path absolute.
    let harness = require_harness!();
    let output = harness
        .command(&harness.executor_path)
        .current_dir(&harness.trees_dir)
        .args(["--xml", "include_main.xml", "--port", "0", "--loops", "1"])
        .args(["--tick-ms", "1"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("RESULT loop=1 status=SUCCESS"), "{stdout}");
}

#[test]
fn busy_port_gives_a_clear_error() {
    let harness = require_harness!();
    let port = free_port_pair();
    let _busy = std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, port + 1)).unwrap();
    let mut options = ExecutorOptions::new(harness.tree("main.xml"));
    options.port = port;
    let error = Executor::spawn_with(harness, options).unwrap_err();
    let text = error.to_string();
    assert!(
        text.contains(&format!(
            "cannot start the publisher on ports {port} and {}",
            port + 1
        )),
        "{text}"
    );
}

#[test]
fn wait_for_stdout_follows_the_runs() {
    let harness = require_harness!();
    let mut options = ExecutorOptions::new("main.xml");
    options.loops = 2;
    options.tick_ms = 1;
    let mut executor = Executor::spawn_with(harness, options).unwrap();
    let line = executor
        .wait_for_stdout(RUN_TIMEOUT, |line| line.starts_with("RESULT loop=2 "))
        .unwrap();
    assert_eq!(line, "RESULT loop=2 status=SUCCESS");
    executor.wait_done(RUN_TIMEOUT).unwrap();
    let error = executor
        .wait_for_stdout(RUN_TIMEOUT, |line| line == "never")
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn executor_stops_when_its_parent_is_killed() {
    let harness = require_harness!();
    let temp = TempDir::new("smoke-orphan").unwrap();
    let out = temp.join("executor.out");
    let pid_file = temp.join("executor.pid");
    let btlog = temp.join("orphan.btlog");
    // A shell starts the executor and is then killed with SIGKILL. The
    // executor gets a new parent and must stop cleanly.
    let script = "\"$1\" --xml \"$2\" --port 0 --tick-ms 5 --exit-with-parent --btlog \"$5\" \
                  > \"$3\" 2>&1 & echo $! > \"$4\"; wait";
    let mut shell = harness
        .command("sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .arg(&harness.executor_path)
        .arg(harness.tree("long_running.xml"))
        .arg(&out)
        .arg(&pid_file)
        .arg(&btlog)
        .spawn()
        .unwrap();

    let read_out = || fs::read_to_string(&out).unwrap_or_default();
    let wait_for = |text: &str| {
        let deadline = std::time::Instant::now() + RUN_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if read_out().contains(text) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    };
    let ready = wait_for("READY port=0");
    shell.kill().unwrap();
    shell.wait().unwrap();
    let stopped = ready && wait_for("STOPPED parent=exited");
    if !stopped {
        // Do not leave the executor behind.
        if let Ok(pid) = fs::read_to_string(&pid_file) {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", pid.trim()])
                .status();
        }
    }
    assert!(ready, "no READY: {}", read_out());
    assert!(stopped, "the executor did not stop: {}", read_out());
    assert!(fs::read(&btlog).unwrap().starts_with(BTLOG_MAGIC));
}

#[test]
fn executor_command_line_errors() {
    let harness = require_harness!();
    let run = |args: &[&str]| {
        let output = harness
            .command(&harness.executor_path)
            .args(args)
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let main = harness.tree("main.xml");
    let main = main.to_str().unwrap();

    for args in [
        &["--xml"][..],
        &["--xml="],
        &["--xml", main, "--port", "-1"],
        &["--xml", main, "--port", "1x"],
        &["--xml", main, "--loops", "99999999999999999999999"],
        &["--xml", main, "--tick-ms", ""],
        &["--xml", main, "--tree="],
        &["--xml", main, "--exit-with-parent=yes"],
        &["--xml", main, "--unknown"],
        &["--xml", main, "extra"],
    ] {
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, Some(2), "{args:?}: {stdout} {stderr}");
        assert!(stdout.is_empty(), "{args:?}: {stdout}");
        assert!(stderr.contains("fasttick_executor:"), "{args:?}: {stderr}");
    }

    // "--flag=value" works like "--flag value".
    let (code, stdout, stderr) = run(&[
        &format!("--xml={main}"),
        "--port=0",
        "--loops=1",
        "--tick-ms=1",
        "--tree=SecondTree",
    ]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        ["READY port=0", "RESULT loop=1 status=SUCCESS", "DONE"]
    );

    // An exception in a node gives exit code 1 and the BT.CPP message.
    let temp = TempDir::new("smoke-throw").unwrap();
    let bad = temp.join("bad_port.xml");
    fs::write(
        &bad,
        r#"<root BTCPP_format="4"><BehaviorTree ID="T"><WaitTicks ticks="abc"/></BehaviorTree></root>"#,
    )
    .unwrap();
    let (code, stdout, stderr) = run(&["--xml", bad.to_str().unwrap(), "--port", "0"]);
    assert_eq!(code, Some(1), "{stdout} {stderr}");
    assert!(stderr.contains("ticks"), "{stderr}");
    assert!(!stdout.contains("DONE"), "{stdout}");

    // dump_models rejects unknown options.
    let output = harness
        .command(&harness.dump_models_path)
        .arg("--bad")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
