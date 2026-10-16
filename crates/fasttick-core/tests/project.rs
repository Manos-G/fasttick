//! Projects: include resolution, model look-up, project file round trip.

use std::fs;
use std::path::{Path, PathBuf};

use fasttick_core::bt::{BtFile, StructureHash};
use fasttick_core::models::{ModelSource, NodeKind};
use fasttick_core::project::{
    FileId, FileRole, IncludeProblem, LayoutDirection, LayoutState, Project, ProjectError,
    ProjectFile, ProjectWarning, TreeRef,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .canonicalize()
        .unwrap()
}

fn btcpp_trees() -> PathBuf {
    fixtures().join("btcpp/tests/trees")
}

/// The file names of the project files, sorted, relative to `base`.
fn file_names(project: &Project, base: &Path) -> Vec<(String, FileRole)> {
    let mut out: Vec<(String, FileRole)> = project
        .files()
        .map(|(_, role, file)| {
            let path = file.path().unwrap();
            let rel = path.strip_prefix(base).unwrap_or(path);
            (rel.to_string_lossy().replace('\\', "/"), role)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn tree_file(project: &Project, tree: TreeRef) -> PathBuf {
    project
        .file(tree.file)
        .unwrap()
        .path()
        .unwrap()
        .to_path_buf()
}

#[test]
fn btcpp_include_fixtures() {
    let base = btcpp_trees();
    let cases: &[(&str, &[&str], &str, &str, &str)] = &[
        (
            "parent_include_child.xml",
            &["child/child_no_include.xml"],
            "ParentIncludeChild",
            "ChildNoInclude",
            "child/child_no_include.xml",
        ),
        (
            "parent_include_child_include_child.xml",
            &[
                "child/child_include_child.xml",
                "child/child/child_child_no_include.xml",
            ],
            "ParentIncludeChildIncludeChild",
            "ChildChildNoInclude",
            "child/child/child_child_no_include.xml",
        ),
        (
            "parent_include_child_include_parent.xml",
            &["child/child_include_parent.xml", "parent_no_include.xml"],
            "ParentIncludeChildIncludeParent",
            "ParentNoInclude",
            "parent_no_include.xml",
        ),
        (
            "parent_include_child_include_sibling.xml",
            &[
                "child/child_include_sibling.xml",
                "child/child_no_include.xml",
            ],
            "ParentIncludeChildIncludeSibling",
            "ChildNoInclude",
            "child/child_no_include.xml",
        ),
    ];
    for (main, included, main_tree_id, deepest_tree, deepest_file) in cases {
        let project = Project::open_xml(base.join(main)).unwrap();
        let mut expected: Vec<(String, FileRole)> = included
            .iter()
            .map(|f| ((*f).to_owned(), FileRole::Included))
            .collect();
        expected.push(((*main).to_owned(), FileRole::Tree));
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(file_names(&project, &base), expected, "{main}");
        assert_eq!(project.name(), main.trim_end_matches(".xml"));
        assert!(project.path().is_none());

        let main_id = project.file_id_by_path(base.join(main)).unwrap();
        assert_eq!(project.include_closure(main_id).len(), included.len() + 1);
        for id in project.file_ids() {
            for link in project.includes(id) {
                assert_eq!(link.problem, None, "{main}");
                assert!(link.target.is_some());
            }
        }
        // The main tree comes from main_tree_to_execute.
        let main_tree = project.main_tree(main_id).unwrap();
        assert_eq!(project.tree(main_tree).unwrap().id, *main_tree_id);
        // The SubTree target is found through the includes.
        let target = project.find_tree_from(main_id, deepest_tree).unwrap();
        assert_eq!(tree_file(&project, target), base.join(deepest_file));
        let (model, source) = project.lookup_model(main_id, deepest_tree).unwrap();
        assert_eq!(model.kind, NodeKind::SubTree);
        assert_eq!(source, ModelSource::InferredSubTree(target.file));
    }
}

#[test]
fn example_with_two_includes() {
    let base = fixtures().join("btcpp/examples/test_files");
    let project = Project::open_xml(base.join("subtree_test.xml")).unwrap();
    assert_eq!(
        file_names(&project, &base),
        vec![
            ("Check.xml".to_owned(), FileRole::Included),
            ("subtree_test.xml".to_owned(), FileRole::Tree),
            ("subtrees/Talk.xml".to_owned(), FileRole::Included),
        ]
    );
    let main = project
        .file_id_by_path(base.join("subtree_test.xml"))
        .unwrap();
    // The only tree of the main file is not the only tree of its closure.
    assert_eq!(
        project.file(main).unwrap().index().main_tree().unwrap().id,
        "BehaviorTree"
    );
    assert_eq!(project.main_tree(main), None);
    assert!(project.find_tree("SayStuff").is_some());
}

fn write(dir: &Path, rel: &str, text: &str) -> PathBuf {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

fn tree_xml(id: &str, includes: &[&str]) -> String {
    let mut s = String::from("<root BTCPP_format=\"4\">\n");
    for include in includes {
        s.push_str(&format!("  <include path=\"{include}\"/>\n"));
    }
    s.push_str(&format!(
        "  <BehaviorTree ID=\"{id}\">\n    <AlwaysSuccess/>\n  </BehaviorTree>\n</root>\n"
    ));
    s
}

fn problems(project: &Project, id: FileId) -> Vec<Option<IncludeProblem>> {
    project
        .includes(id)
        .iter()
        .map(|l| l.problem.clone())
        .collect()
}

#[test]
fn include_cycles_are_detected() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    // a -> b -> a
    let a = write(d, "a.xml", &tree_xml("A", &["b.xml"]));
    write(d, "b.xml", &tree_xml("B", &["a.xml"]));
    // self include
    let s = write(d, "self.xml", &tree_xml("S", &["./self.xml"]));
    // x -> y -> sub/z -> x (three files, relative paths)
    let x = write(d, "x.xml", &tree_xml("X", &["y.xml"]));
    write(d, "y.xml", &tree_xml("Y", &["sub/z.xml"]));
    write(d, "sub/z.xml", &tree_xml("Z", &["../x.xml"]));
    // diamond: p -> q, p -> r, q -> t, r -> t (no cycle)
    let p = write(d, "p.xml", &tree_xml("P", &["q.xml", "r.xml"]));
    write(d, "q.xml", &tree_xml("Q", &["t.xml"]));
    write(d, "r.xml", &tree_xml("R", &["t.xml"]));
    write(d, "t.xml", &tree_xml("T", &[]));

    let project = Project::open_xml(&a).unwrap();
    assert_eq!(project.files().count(), 2);
    let a_id = project.file_id_by_path(&a).unwrap();
    let b_id = project.file_id_by_path(d.join("b.xml")).unwrap();
    assert_eq!(problems(&project, a_id), vec![None]);
    assert!(matches!(
        problems(&project, b_id)[0],
        Some(IncludeProblem::Cycle { .. })
    ));
    assert_eq!(project.includes(b_id)[0].target, Some(a_id));
    assert_eq!(project.include_closure(a_id), vec![a_id, b_id]);
    assert_eq!(project.include_closure(b_id), vec![b_id, a_id]);
    // The models of both files are visible from both files.
    assert!(project.lookup_model(b_id, "A").is_some());

    let project = Project::open_xml(&s).unwrap();
    let s_id = project.file_id_by_path(&s).unwrap();
    assert_eq!(project.files().count(), 1);
    assert!(matches!(
        &problems(&project, s_id)[0],
        Some(IncludeProblem::Cycle { path }) if *path == s
    ));

    let project = Project::open_xml(&x).unwrap();
    assert_eq!(project.files().count(), 3);
    let z_id = project.file_id_by_path(d.join("sub/z.xml")).unwrap();
    assert!(matches!(
        problems(&project, z_id)[0],
        Some(IncludeProblem::Cycle { .. })
    ));
    let cycles = project
        .file_ids()
        .into_iter()
        .flat_map(|id| problems(&project, id))
        .filter(|p| p.is_some())
        .count();
    assert_eq!(cycles, 1);

    let project = Project::open_xml(&p).unwrap();
    assert_eq!(project.files().count(), 4);
    for id in project.file_ids() {
        assert!(problems(&project, id).iter().all(Option::is_none));
    }
    let p_id = project.file_id_by_path(&p).unwrap();
    let closure: Vec<String> = project
        .include_closure(p_id)
        .into_iter()
        .map(|id| project.file(id).unwrap().display_name())
        .collect();
    assert_eq!(closure, vec!["p.xml", "q.xml", "t.xml", "r.xml"]);
}

#[test]
fn include_problems_do_not_stop_loading() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let main = write(
        d,
        "main.xml",
        r#"<root BTCPP_format="4">
  <include path="missing.xml"/>
  <include/>
  <include path="bad.xml"/>
  <include path="pkg_tree.xml" ros_pkg="my_pkg"/>
  <include path="good.xml"/>
  <BehaviorTree ID="Main"><SubTree ID="Good"/></BehaviorTree>
</root>"#,
    );
    write(d, "bad.xml", "<root><BehaviorTree></root>");
    write(d, "good.xml", &tree_xml("Good", &[]));
    let mut project = Project::new("p");
    project.set_ament_prefix_path(None);
    let main_id = project.add_file(&main).unwrap();
    let p = problems(&project, main_id);
    assert!(
        matches!(&p[0], Some(IncludeProblem::Load { path, .. }) if path.ends_with("missing.xml"))
    );
    assert_eq!(p[1], Some(IncludeProblem::MissingPath));
    assert!(
        matches!(&p[2], Some(IncludeProblem::Load { message, .. }) if message.contains("line"))
    );
    assert_eq!(
        p[3],
        Some(IncludeProblem::RosPackageNotFound {
            package: "my_pkg".into(),
            no_ament_prefix_path: true
        })
    );
    assert_eq!(p[4], None);
    assert_eq!(project.files().count(), 2);
    assert!(project.find_tree_from(main_id, "Good").is_some());

    // With a ROS prefix, the ros_pkg include resolves to share/<pkg>.
    let prefix = d.join("install");
    write(
        &prefix,
        "share/ament_index/resource_index/packages/my_pkg",
        "",
    );
    let pkg_file = write(
        &prefix,
        "share/my_pkg/pkg_tree.xml",
        &tree_xml("FromPackage", &[]),
    );
    project.set_ament_prefix_path(Some(prefix.into_os_string()));
    let p = problems(&project, main_id);
    assert_eq!(p[3], None);
    let target = project.includes(main_id)[3].target.unwrap();
    assert_eq!(
        project.file(target).unwrap().path(),
        Some(pkg_file.as_path())
    );
    assert_eq!(project.role(target), Some(FileRole::Included));
    assert!(project.find_tree_from(main_id, "FromPackage").is_some());

    // When the include goes away, the included file goes too.
    project
        .edit_file(main_id, |file| {
            let element = file.index().includes[3].element;
            let undo = file.doc_mut().remove_element_pretty(element).unwrap();
            file.reindex();
            undo
        })
        .unwrap();
    assert!(project.file(target).is_none());
    assert!(project.find_tree("FromPackage").is_none());
}

#[test]
fn models_through_includes_and_model_files() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let main = write(
        d,
        "main.xml",
        r#"<root BTCPP_format="4">
  <include path="lib.xml"/>
  <BehaviorTree ID="Main"><Sequence><LibAction/><CheckFlag flag="true"/></Sequence></BehaviorTree>
</root>"#,
    );
    write(
        d,
        "lib.xml",
        r#"<root BTCPP_format="4">
  <BehaviorTree ID="Lib"><LibAction/></BehaviorTree>
  <TreeNodesModel><Action ID="LibAction"><input_port name="x"/></Action></TreeNodesModel>
</root>"#,
    );
    let models = fixtures().join("harness/custom_models.xml");
    let mut project = Project::new("m");
    let main_id = project.add_file(&main).unwrap();
    let models_id = project.add_model_file(&models).unwrap();
    let lib_id = project.file_id_by_path(d.join("lib.xml")).unwrap();
    assert_eq!(project.role(models_id), Some(FileRole::Models));
    assert_eq!(
        project.lookup_model(main_id, "LibAction").unwrap().1,
        ModelSource::File(lib_id)
    );
    assert_eq!(
        project.lookup_model(main_id, "CheckFlag").unwrap().1,
        ModelSource::ProjectModels(models_id)
    );
    assert_eq!(
        project.lookup_model(main_id, "Sequence").unwrap().1,
        ModelSource::Builtin
    );
    let palette = project.registry().all_models(main_id);
    assert!(
        palette[&NodeKind::Condition]
            .iter()
            .any(|m| m.id == "CheckFlag")
    );
    assert!(palette[&NodeKind::SubTree].iter().any(|m| m.id == "Lib"));

    // Adding the included file as a tree file changes its role.
    let again = project.add_file(d.join("lib.xml")).unwrap();
    assert_eq!(again, lib_id);
    assert_eq!(project.role(lib_id), Some(FileRole::Tree));
}

/// Copies the harness trees and models into a temporary project directory.
fn harness_project_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = fixtures().join("../../test_harness/trees");
    for (from, to) in [
        ("main.xml", "trees/main.xml"),
        ("include_main.xml", "trees/include_main.xml"),
        ("included/sub.xml", "trees/included/sub.xml"),
    ] {
        let text = fs::read_to_string(root.join(from)).unwrap();
        write(dir.path(), to, &text);
    }
    let models = fs::read_to_string(fixtures().join("harness/custom_models.xml")).unwrap();
    write(dir.path(), "models/custom_models.xml", &models);
    dir
}

#[test]
fn project_file_round_trip() {
    let dir = harness_project_dir();
    let d = dir.path();
    let mut project = Project::new("demo");
    let main = project.add_file(d.join("trees/main.xml")).unwrap();
    let inc_main = project.add_file(d.join("trees/include_main.xml")).unwrap();
    let models = project
        .add_model_file(d.join("models/custom_models.xml"))
        .unwrap();
    let sub = project
        .file_id_by_path(d.join("trees/included/sub.xml"))
        .unwrap();
    assert_eq!(project.role(sub), Some(FileRole::Included));
    assert!(project.is_project_dirty());

    // Layout with positions for two trees, one in an included file.
    let main_file = project.file(main).unwrap();
    let hash = main_file
        .structure_hash(main_file.index().tree_index("MainTree").unwrap())
        .unwrap();
    let mut state = LayoutState::new(LayoutDirection::LeftRight, hash);
    state.set_position(&[0], [0.0, 0.0]);
    state.set_position(&[0, 1], [120.5, 80.0]);
    state.set_position(&[0, 3, 0], [-10.0, 300.25]);
    project.set_layout(main, "MainTree", state.clone());
    let sub_file = project.file(sub).unwrap();
    let sub_tree = sub_file.trees()[0].id.clone();
    let sub_hash = sub_file.structure_hash(0).unwrap();
    project.set_layout(
        sub,
        &sub_tree,
        LayoutState::new(LayoutDirection::TopDown, sub_hash),
    );

    // Unconnected nodes.
    let nodes = project
        .file_mut(main)
        .unwrap()
        .doc_mut()
        .parse_fragment("<Sleep msec=\"10\"/><Sequence name=\"loose &amp; free\">\n  <AlwaysSuccess/>\n</Sequence>")
        .unwrap();
    project.add_unconnected(main, "MainTree", nodes[0], [300.0, 40.0]);
    project.add_unconnected(main, "Navigate", nodes[1], [10.0, -5.5]);
    assert_eq!(project.unconnected(main, "MainTree").len(), 1);

    let path = d.join("demo.ftproj");
    project.save_as(&path).unwrap();
    assert!(!project.is_project_dirty());
    assert_eq!(project.path(), Some(path.as_path()));
    let json = fs::read_to_string(&path).unwrap();
    let data = ProjectFile::from_json(&json).unwrap();
    assert_eq!(data.version, 1);
    assert_eq!(data.name, "demo");
    assert_eq!(data.files, vec!["trees/main.xml", "trees/include_main.xml"]);
    assert_eq!(data.model_files, vec!["models/custom_models.xml"]);
    let keys: Vec<&String> = data.layout.keys().collect();
    assert_eq!(
        keys,
        vec![
            "trees/included/sub.xml#IncludedTree",
            "trees/main.xml#MainTree"
        ]
    );
    let entry = &data.layout["trees/main.xml#MainTree"];
    assert_eq!(entry.structure_hash, hash.to_string());
    assert_eq!(entry.positions["0/3/0"], [-10.0, 300.25]);
    assert!(json.contains("\"direction\": \"left_right\""));
    assert_eq!(data.unconnected.len(), 2);
    assert_eq!(data.unconnected[0].xml, "<Sleep msec=\"10\"/>");
    assert_eq!(data.unconnected[1].tree, "Navigate");
    assert!(data.unconnected[1].xml.contains("loose &amp; free"));

    // Load it again.
    let loaded = Project::open(&path).unwrap();
    assert!(loaded.warnings().is_empty(), "{:?}", loaded.warnings());
    assert!(!loaded.is_dirty());
    assert_eq!(loaded.name(), "demo");
    assert_eq!(file_names(&loaded, d), file_names(&project, d));
    let l_main = loaded.file_id_by_path(d.join("trees/main.xml")).unwrap();
    let l_sub = loaded
        .file_id_by_path(d.join("trees/included/sub.xml"))
        .unwrap();
    assert_eq!(loaded.layout(l_main, "MainTree"), Some(&state));
    assert_eq!(
        loaded.valid_positions(l_main, "MainTree"),
        Some(&state.positions)
    );
    assert_eq!(
        loaded.layout(l_sub, &sub_tree).unwrap().direction,
        LayoutDirection::TopDown
    );
    let loose = loaded.unconnected(l_main, "Navigate");
    assert_eq!(loose.len(), 1);
    assert_eq!(loose[0].position, [10.0, -5.5]);
    let doc = loaded.file(l_main).unwrap().doc();
    assert_eq!(doc.parent(loose[0].node), None);
    assert_eq!(doc.attribute(loose[0].node, "name"), Some("loose & free"));
    assert_eq!(doc.element_children(loose[0].node).count(), 1);
    // The unconnected nodes are not in the XML file.
    assert_eq!(
        loaded.file(l_main).unwrap().to_xml_string(),
        fs::read_to_string(d.join("trees/main.xml")).unwrap()
    );
    let models_again = loaded
        .file_id_by_path(d.join("models/custom_models.xml"))
        .unwrap();
    assert_eq!(loaded.role(models_again), Some(FileRole::Models));
    let _ = (inc_main, models);

    // Saving the loaded project gives the same JSON.
    let mut loaded = loaded;
    let path2 = d.join("again.ftproj");
    loaded.save_as(&path2).unwrap();
    assert_eq!(fs::read_to_string(&path2).unwrap(), json);

    // An external change of the tree structure: the positions are not used.
    let main_path = d.join("trees/main.xml");
    let text = fs::read_to_string(&main_path).unwrap();
    fs::write(
        &main_path,
        text.replace(
            "<SayMessage name=\"say_default\"/>",
            "<SayMessage name=\"say_default\"/><AlwaysSuccess/>",
        ),
    )
    .unwrap();
    let changed = Project::open(&path).unwrap();
    let c_main = changed.file_id_by_path(&main_path).unwrap();
    assert!(changed.layout(c_main, "MainTree").is_some());
    assert_eq!(changed.valid_positions(c_main, "MainTree"), None);
    // Only attributes changed: the positions stay valid.
    fs::write(
        &main_path,
        text.replace("main_sequence", "renamed_sequence"),
    )
    .unwrap();
    let mut changed = Project::open(&path).unwrap();
    let c_main = changed.file_id_by_path(&main_path).unwrap();
    assert!(changed.valid_positions(c_main, "MainTree").is_some());

    // reload_file keeps the unconnected nodes.
    fs::write(&main_path, &text).unwrap();
    changed.reload_file(c_main).unwrap();
    let loose = changed.unconnected(c_main, "Navigate");
    assert_eq!(loose.len(), 1);
    let doc = changed.file(c_main).unwrap().doc();
    assert_eq!(doc.attribute(loose[0].node, "name"), Some("loose & free"));
}

#[test]
fn unconnected_and_layout_edits() {
    let dir = harness_project_dir();
    let d = dir.path();
    let (mut project, main) =
        Project::with_file("x", BtFile::load(d.join("trees/main.xml")).unwrap());
    project.refresh();
    let nodes = project
        .file_mut(main)
        .unwrap()
        .doc_mut()
        .parse_fragment("<A/><B/>")
        .unwrap();
    project.add_unconnected(main, "MainTree", nodes[0], [1.0, 1.0]);
    project.add_unconnected(main, "MainTree", nodes[1], [2.0, 2.0]);
    assert!(project.set_unconnected_position(main, nodes[1], [5.0, 5.0]));
    let attached = project.file(main).unwrap().trees()[0].root.unwrap();
    assert!(!project.set_unconnected_position(main, attached, [0.0, 0.0]));
    assert_eq!(project.all_unconnected().count(), 2);
    let (tree, removed) = project.remove_unconnected(main, nodes[0]).unwrap();
    assert_eq!(tree, "MainTree");
    assert_eq!(removed.position, [1.0, 1.0]);
    assert!(project.remove_unconnected(main, nodes[0]).is_none());

    // Renaming a tree moves its state.
    project.set_layout(
        main,
        "MainTree",
        LayoutState::new(LayoutDirection::TopDown, StructureHash(1)),
    );
    project.rename_tree_state(main, "MainTree", "Renamed");
    assert!(project.layout(main, "MainTree").is_none());
    assert!(project.layout(main, "Renamed").is_some());
    assert_eq!(project.unconnected(main, "Renamed").len(), 1);
    assert_eq!(project.unconnected(main, "Renamed")[0].position, [5.0, 5.0]);
    assert!(project.remove_layout(main, "Renamed").is_some());

    // A node that was connected again is not saved as unconnected.
    let root = project.file(main).unwrap().trees()[0].root.unwrap();
    project
        .file_mut(main)
        .unwrap()
        .doc_mut()
        .append_child(root, nodes[1])
        .unwrap();
    let data = project.to_project_file(d).unwrap();
    assert!(data.unconnected.is_empty());
}

#[test]
fn missing_files_and_bad_entries_are_warnings() {
    let dir = harness_project_dir();
    let d = dir.path();
    let path = write(
        d,
        "p.ftproj",
        r##"{
  "version": 1,
  "files": ["trees/main.xml", "trees/gone.xml"],
  "model_files": ["models/missing_models.xml"],
  "layout": {
    "trees/main.xml#MainTree": { "direction": "top_down", "structure_hash": "…", "positions": { "0": [1.0, 2.0] } },
    "trees/main.xml#Navigate": { "direction": "top_down", "structure_hash": "0000000000000001", "positions": { "x/y": [1.0, 2.0], "0": [3.0, 4.0] } },
    "trees/unknown.xml#T": { "direction": "top_down", "structure_hash": "0000000000000001" }
  },
  "unconnected": [
    { "file": "trees/main.xml", "tree": "MainTree", "xml": "<Broken", "position": [0.0, 0.0] },
    { "file": "trees/nope.xml", "tree": "MainTree", "xml": "<A/>", "position": [0.0, 0.0] }
  ],
  "future_field": [1, 2, 3]
}"##,
    );
    let mut project = Project::open(&path).unwrap();
    assert_eq!(project.name(), "p");
    let warnings = project.warnings().to_vec();
    let not_loaded = warnings
        .iter()
        .filter(|w| matches!(w, ProjectWarning::FileNotLoaded { .. }))
        .count();
    assert_eq!(not_loaded, 2);
    let layouts = warnings
        .iter()
        .filter(|w| matches!(w, ProjectWarning::LayoutIgnored { .. }))
        .count();
    assert_eq!(layouts, 3, "{warnings:?}");
    let unconnected = warnings
        .iter()
        .filter(|w| matches!(w, ProjectWarning::UnconnectedIgnored { .. }))
        .count();
    assert_eq!(unconnected, 2);
    for w in &warnings {
        assert!(!w.to_string().is_empty());
    }
    assert_eq!(project.missing_files().len(), 2);
    let main = project.file_id_by_path(d.join("trees/main.xml")).unwrap();
    // A bad hash keeps the direction only.
    let state = project.layout(main, "MainTree").unwrap();
    assert!(state.positions.is_empty());
    // A bad position key is dropped, the others stay.
    assert_eq!(project.layout(main, "Navigate").unwrap().positions.len(), 1);

    // The missing files stay in the project file.
    project.save().unwrap();
    let data = ProjectFile::from_json(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(data.files, vec!["trees/main.xml", "trees/gone.xml"]);
    assert_eq!(data.model_files, vec!["models/missing_models.xml"]);

    // Errors.
    assert!(matches!(
        Project::open(d.join("none.ftproj")),
        Err(ProjectError::Read { .. })
    ));
    let bad = write(d, "bad.ftproj", r#"{"version": 9}"#);
    let err = Project::open(&bad).unwrap_err();
    assert!(matches!(err, ProjectError::Format { .. }));
    assert!(err.to_string().contains("version 9"), "{err}");
    assert!(matches!(
        Project::open_xml(d.join("none.xml")),
        Err(ProjectError::File(_))
    ));
}

#[test]
fn dirty_state_and_saving_files() {
    let dir = harness_project_dir();
    let d = dir.path();
    let mut project = Project::open_xml(d.join("trees/include_main.xml")).unwrap();
    assert!(!project.is_dirty());
    let main = project
        .file_id_by_path(d.join("trees/include_main.xml"))
        .unwrap();
    assert!(project.dirty_files().is_empty());
    let root = project.file(main).unwrap().index().root.unwrap();
    project
        .edit_file(main, |file| {
            file.doc_mut()
                .set_attribute(root, "main_tree_to_execute", "IncludedTree")
                .unwrap();
            file.reindex();
            file.bump_generation();
        })
        .unwrap();
    assert_eq!(project.dirty_files(), vec![main]);
    assert!(project.is_dirty());
    let tree = project.main_tree(main).unwrap();
    assert_eq!(project.tree(tree).unwrap().id, "IncludedTree");
    project.save_dirty_files().unwrap();
    assert!(project.dirty_files().is_empty());
    assert!(
        fs::read_to_string(d.join("trees/include_main.xml"))
            .unwrap()
            .contains("main_tree_to_execute=\"IncludedTree\"")
    );

    // Save as in another directory: the relative include breaks there.
    fs::create_dir_all(d.join("elsewhere")).unwrap();
    project
        .save_file_as(main, d.join("elsewhere/copy.xml"))
        .unwrap();
    assert!(matches!(
        project.includes(main)[0].problem,
        Some(IncludeProblem::Load { .. })
    ));
    // The old included file is no longer needed and goes away.
    assert!(
        project
            .file_id_by_path(d.join("trees/included/sub.xml"))
            .is_none()
    );
    assert!(matches!(
        project.save_file(FileId(99)),
        Err(ProjectError::UnknownFile(_))
    ));
}

#[test]
fn layout_keys_with_hash_signs() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let file = write(d, "a#b.xml", &tree_xml("T#1", &[]));
    let mut project = Project::new("h");
    let id = project.add_file(&file).unwrap();
    let hash = project.file(id).unwrap().structure_hash(0).unwrap();
    let mut state = LayoutState::new(LayoutDirection::TopDown, hash);
    state.set_position(&[0], [7.0, 8.0]);
    project.set_layout(id, "T#1", state.clone());
    let path = d.join("h.ftproj");
    project.save_as(&path).unwrap();
    let loaded = Project::open(&path).unwrap();
    let id = loaded.file_id_by_path(&file).unwrap();
    assert_eq!(loaded.layout(id, "T#1"), Some(&state));
    assert!(loaded.warnings().is_empty());
}

#[test]
fn remove_file_drops_unneeded_includes() {
    let dir = harness_project_dir();
    let d = dir.path();
    let mut project = Project::new("r");
    let a = project.add_file(d.join("trees/include_main.xml")).unwrap();
    let main = project.add_file(d.join("trees/main.xml")).unwrap();
    let sub = project
        .file_id_by_path(d.join("trees/included/sub.xml"))
        .unwrap();
    project.set_layout(
        a,
        "IncludeMain",
        LayoutState::new(LayoutDirection::TopDown, StructureHash(0)),
    );
    let removed = project.remove_file(a).unwrap();
    assert_eq!(removed.display_name(), "include_main.xml");
    assert!(project.file(sub).is_none());
    assert!(project.layout(a, "IncludeMain").is_none());
    assert_eq!(project.file_ids(), vec![main]);
    assert!(project.find_tree("IncludedTree").is_none());
    assert!(project.find_tree("MainTree").is_some());
    assert_eq!(project.find_trees("MainTree").len(), 1);
}

/// BT.CPP resolves an include relative to the directory of the including
/// file as it was given (it does not follow a link at the file), and the
/// operating system resolves `..` after it follows a directory link. This is
/// the layout of a ROS workspace that is built with `--symlink-install`.
#[cfg(unix)]
#[test]
fn includes_through_symbolic_links_resolve_like_btcpp() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().canonicalize().unwrap();
    let main_text = r#"<root BTCPP_format="4">
  <include path="../other/x.xml"/>
  <BehaviorTree ID="Main"><AlwaysSuccess/></BehaviorTree>
</root>"#;
    write(&d, "src/pkg/trees/main.xml", main_text);
    write(&d, "src/pkg/other/x.xml", &tree_xml("SrcX", &[]));
    write(
        &d,
        "install/share/pkg/other/x.xml",
        &tree_xml("InstallX", &[]),
    );

    // 1. A link at the file: the include is relative to the install tree.
    fs::create_dir_all(d.join("install/share/pkg/trees")).unwrap();
    symlink(
        d.join("src/pkg/trees/main.xml"),
        d.join("install/share/pkg/trees/main.xml"),
    )
    .unwrap();
    let project = Project::open_xml(d.join("install/share/pkg/trees/main.xml")).unwrap();
    assert!(project.find_tree("InstallX").is_some());
    assert!(project.find_tree("SrcX").is_none());

    // 2. A link at the directory: ".." goes to the parent of the target.
    symlink(
        d.join("src/pkg/trees"),
        d.join("install/share/pkg/trees_link"),
    )
    .unwrap();
    let project = Project::open_xml(d.join("install/share/pkg/trees_link/main.xml")).unwrap();
    assert!(project.find_tree("SrcX").is_some());
    assert!(project.find_tree("InstallX").is_none());
}

#[test]
fn values_that_json_cannot_hold_are_not_saved() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let file = write(d, "t.xml", &tree_xml("T", &[]));
    let mut project = Project::new("n");
    let id = project.add_file(&file).unwrap();
    let mut state = LayoutState::new(LayoutDirection::TopDown, StructureHash(3));
    state.positions.insert("0".into(), [f32::NAN, 1.0]);
    state.positions.insert("0/0".into(), [1.0, 2.0]);
    project.set_layout(id, "T", state);
    let nodes = project
        .file_mut(id)
        .unwrap()
        .doc_mut()
        .parse_fragment("<A/>")
        .unwrap();
    project.add_unconnected(id, "T", nodes[0], [f32::INFINITY, 0.0]);
    let path = d.join("n.ftproj");
    project.save_as(&path).unwrap();
    let loaded = Project::open(&path).unwrap();
    let id = loaded.file_id_by_path(&file).unwrap();
    assert_eq!(loaded.layout(id, "T").unwrap().positions.len(), 1);
    assert_eq!(loaded.unconnected(id, "T")[0].position, [0.0, 0.0]);
}

#[test]
fn a_missing_file_can_come_back() {
    let dir = harness_project_dir();
    let d = dir.path();
    let path = write(
        d,
        "p.ftproj",
        r#"{"version": 1, "files": ["trees/main.xml", "trees/later.xml"]}"#,
    );
    let mut project = Project::open(&path).unwrap();
    assert_eq!(project.missing_files().len(), 1);
    write(d, "trees/later.xml", &tree_xml("Later", &[]));
    let id = project.add_file(d.join("trees/later.xml")).unwrap();
    assert!(project.missing_files().is_empty());
    assert_eq!(project.role(id), Some(FileRole::Tree));
    project.save().unwrap();
    let data = ProjectFile::from_json(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(data.files, vec!["trees/main.xml", "trees/later.xml"]);

    // A path change through file_mut is seen by refresh.
    let main = project.file_id_by_path(d.join("trees/main.xml")).unwrap();
    project
        .file_mut(main)
        .unwrap()
        .save(d.join("trees/moved.xml"))
        .unwrap();
    project.refresh();
    assert_eq!(
        project.file_id_by_path(d.join("trees/moved.xml")),
        Some(main)
    );
}

#[test]
fn absolute_includes_resolve_below_a_mirror_root() {
    use fasttick_core::project::MirrorRoot;
    let dir = tempfile::tempdir().unwrap();
    // The copy of a remote computer: /home/pi/trees/main.xml includes the
    // absolute path /opt/lib/my lib.xml (encoded as "my%20lib.xml").
    let root = dir.path().join("pi@robot");
    let main = write(
        &root,
        "home/pi/trees/main.xml",
        &tree_xml("Main", &["/opt/lib/my lib.xml", "sub/b.xml"]),
    );
    write(&root, "opt/lib/my%20lib.xml", &tree_xml("Lib", &[]));
    write(&root, "home/pi/trees/sub/b.xml", &tree_xml("B", &[]));
    fn encode(part: &str) -> String {
        part.replace(' ', "%20")
    }
    let mut project = Project::open_xml(&main).unwrap();
    let main_id = project.file_id_by_path(&main).unwrap();
    // Without the mirror root, the absolute path is a path of this computer.
    assert!(matches!(
        problems(&project, main_id)[0],
        Some(IncludeProblem::Load { .. })
    ));
    assert_eq!(problems(&project, main_id)[1], None);
    project.set_mirror_root(Some(MirrorRoot {
        root: root.clone(),
        encode,
    }));
    assert_eq!(problems(&project, main_id), [None, None]);
    let target = project.includes(main_id)[0].path.clone().unwrap();
    assert_eq!(target, root.join("opt/lib/my%20lib.xml"));
    assert!(project.find_tree("Lib").is_some());
    // A file outside the mirror root keeps the normal rule.
    let outside = write(
        dir.path(),
        "local/main.xml",
        &tree_xml("M", &["/opt/lib/x.xml"]),
    );
    let mut local = Project::open_xml(&outside).unwrap();
    local.set_mirror_root(Some(MirrorRoot { root, encode }));
    let id = local.file_id_by_path(&outside).unwrap();
    assert_eq!(
        local.includes(id)[0].path.as_deref(),
        Some(Path::new("/opt/lib/x.xml"))
    );
}

#[test]
fn restore_file_text_makes_the_file_dirty_and_loads_new_includes() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main.xml");
    fs::write(
        &main,
        r#"<root BTCPP_format="4"><BehaviorTree ID="Main"><AlwaysSuccess/></BehaviorTree></root>"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("sub.xml"),
        r#"<root BTCPP_format="4"><BehaviorTree ID="Sub"><AlwaysFailure/></BehaviorTree></root>"#,
    )
    .unwrap();
    let mut project = Project::open_xml(&main).unwrap();
    let id = project.file_id_by_path(&main).unwrap();
    assert!(!project.is_dirty());
    assert!(project.find_tree("Sub").is_none());

    let text = r#"<root BTCPP_format="4"><include path="sub.xml"/><BehaviorTree ID="Main"><SubTree ID="Sub"/></BehaviorTree></root>"#;
    project.restore_file_text(id, text).unwrap();
    assert_eq!(project.dirty_files(), vec![id]);
    assert_eq!(project.file(id).unwrap().path(), Some(main.as_path()));
    assert!(project.find_tree("Sub").is_some(), "the include is loaded");
    // The file on disk does not change.
    assert!(!fs::read_to_string(&main).unwrap().contains("SubTree"));

    // Text that is not XML does not change the project.
    assert!(project.restore_file_text(id, "<root").is_err());
    assert!(project.find_tree("Sub").is_some());
}
