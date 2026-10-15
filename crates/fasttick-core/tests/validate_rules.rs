//! The rules of the validator (`fasttick_core::validate`), one by one.
//!
//! [`CASES`] has, for each problem code, at least one document with the
//! problem (`bad`) and the same document without it (`good`). A `bad`
//! document must give a problem with the code and the severity of the
//! case. A `good` document must give no problem at all.
//!
//! Each project has the document and the models of the test executor
//! (`tests/fixtures/harness/all_models.xml`) as a project model file, so
//! the cases can use the custom nodes `SayMessage`, `CheckFlag`,
//! `IncrementCounter`, `WriteValues` and the others.
//!
//! The field [`Case::btcpp`] tells what BT.CPP 4.9.0 does with the `bad`
//! document. A scratch program that links the installed BT.CPP 4.9.0
//! checked all cases: it registers dummy nodes with the ports of the
//! harness models and of the models in the document, calls
//! `registerBehaviorTreeFromFile`, `createTree` for each tree and
//! `createTreeFromFile` (for the main tree). Run
//! `FASTTICK_DUMP_CASES=<dir> cargo test -p fasttick-core --test
//! validate_rules -- --ignored dump_cases` to write the documents.
//!
//! The rules that need more than one file, a document that is made in the
//! test, or a project state are tested below the table.

use std::path::{Path, PathBuf};

use fasttick_core::bt::BtFile;
use fasttick_core::project::{FileId, FileRole, Project};
use fasttick_core::validate::{
    Problem, ProblemCode, ProblemCounts, Severity, Validator, validate_file, validate_project,
};
use fasttick_core::xml::{Document, NodeId};

// --------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn harness_models() -> PathBuf {
    workspace().join("tests/fixtures/harness/all_models.xml")
}

/// A full document, or a tree body that goes into
/// `<root BTCPP_format="4"><BehaviorTree ID="Main">...</BehaviorTree></root>`.
fn document(text: &str) -> String {
    let t = text.trim_start();
    if t.starts_with("<root") || t.starts_with("<tree") || t.starts_with("<?xml") {
        text.to_owned()
    } else {
        format!(
            "<root BTCPP_format=\"4\">\n  <BehaviorTree ID=\"Main\">\n    {text}\n  \
             </BehaviorTree>\n</root>\n"
        )
    }
}

/// A project with the document (as an in-memory tree file) and the harness
/// models. Returns the project and the ID of the document.
fn setup(text: &str) -> (Project, FileId) {
    let file =
        BtFile::parse(&document(text), None).unwrap_or_else(|e| panic!("{e}\n{}", document(text)));
    let (mut project, id) = Project::with_file("rules", file);
    project.add_model_file(harness_models()).unwrap();
    (project, id)
}

/// The problems of the document of a case (not of the harness models).
fn problems(text: &str) -> Vec<Problem> {
    let (project, id) = setup(text);
    validate_file(&project, id)
}

fn show(problems: &[Problem]) -> String {
    problems
        .iter()
        .map(|p| format!("  {p} (tree {:?}, attribute {:?})", p.tree, p.attribute))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The problems with this code.
fn with_code(problems: &[Problem], code: ProblemCode) -> Vec<&Problem> {
    problems.iter().filter(|p| p.code == code).collect()
}

/// The only problem with this code.
fn single(problems: &[Problem], code: ProblemCode) -> &Problem {
    let found = with_code(problems, code);
    assert_eq!(found.len(), 1, "{code}:\n{}", show(problems));
    found[0]
}

/// The tag of the element of a problem.
fn tag_of(project: &Project, p: &Problem) -> String {
    let doc = project.file(p.file).unwrap().doc();
    doc.name(p.node.unwrap()).unwrap().to_owned()
}

// --------------------------------------------------------------------
// The table
// --------------------------------------------------------------------

/// What BT.CPP 4.9.0 does with the `bad` document of a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Btcpp {
    /// It throws when it loads the file or when it creates a tree.
    Rejects,
    /// It loads the file and creates each tree by its ID, but it cannot
    /// create the main tree with `createTreeFromFile`.
    MainTreeFails,
    /// It creates the tree; the node throws or fails when it ticks.
    TickFails,
    /// It accepts the file. The tree probably does not do what the author
    /// wants, or BT.CPP ignores a part of the file.
    Accepts,
}

struct Case {
    code: ProblemCode,
    severity: Severity,
    btcpp: Btcpp,
    bad: &'static str,
    good: &'static str,
}

const fn case(
    code: ProblemCode,
    severity: Severity,
    btcpp: Btcpp,
    bad: &'static str,
    good: &'static str,
) -> Case {
    Case {
        code,
        severity,
        btcpp,
        bad,
        good,
    }
}

use Btcpp::{Accepts, MainTreeFails, Rejects, TickFails};
use ProblemCode as C;
use Severity::{Error, Info, Warning};

/// A tree `Sub` that the documents with a `<SubTree>` use.
macro_rules! with_sub {
    ($models:expr, $main:expr) => {
        concat!(
            "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\">\n",
            $models,
            "\n<BehaviorTree ID=\"Main\">\n",
            $main,
            "\n</BehaviorTree>\n<BehaviorTree ID=\"Sub\">\n<AlwaysSuccess/>\n</BehaviorTree>\n</root>\n"
        )
    };
}

/// A model of the tree `Sub` with the port `goal` (no default value) and
/// the output port `result`.
macro_rules! sub_model {
    () => {
        "<TreeNodesModel>\n<SubTree ID=\"Sub\">\n<input_port name=\"goal\"/>\n\
         <output_port name=\"result\" default=\"{result}\"/>\n</SubTree>\n</TreeNodesModel>"
    };
}

const CASES: &[Case] = &[
    // ---- File level ----
    case(
        C::RootElement,
        Error,
        Rejects,
        "<tree BTCPP_format=\"4\"><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></tree>",
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
    ),
    case(
        C::RootElement,
        Warning,
        Accepts,
        "<root BTCPP_format=\"4\"><Notes text=\"x\"/><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
    ),
    case(
        C::FormatVersion,
        Warning,
        Accepts,
        "<root><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
    ),
    case(
        C::FormatVersion,
        Warning,
        Accepts,
        "<root BTCPP_format=\"3\"><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
    ),
    case(
        C::UnresolvedInclude,
        Error,
        Rejects,
        "<root BTCPP_format=\"4\"><include/><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
    ),
    case(
        C::UnresolvedInclude,
        Error,
        Rejects,
        "<root BTCPP_format=\"4\"><include path=\"/nonexistent/fasttick/missing.xml\"/>\
         <BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
    ),
    case(
        C::BadModel,
        Error,
        Rejects,
        "<root BTCPP_format=\"4\">\n<TreeNodesModel><Action ID=\"First\"/></TreeNodesModel>\n\
         <TreeNodesModel><Action ID=\"Second\"/></TreeNodesModel>\n\
         <BehaviorTree ID=\"Main\"><First/></BehaviorTree>\n</root>",
        "<root BTCPP_format=\"4\">\n<TreeNodesModel><Action ID=\"First\"/><Action ID=\"Second\"/>\
         </TreeNodesModel>\n<BehaviorTree ID=\"Main\"><First/></BehaviorTree>\n</root>",
    ),
    case(
        C::BadModel,
        Error,
        Rejects,
        with_sub!(
            "<TreeNodesModel><SubTree><input_port name=\"goal\" default=\"1\"/></SubTree></TreeNodesModel>",
            "<SubTree ID=\"Sub\"/>"
        ),
        with_sub!(
            "<TreeNodesModel><SubTree ID=\"Sub\"><input_port name=\"goal\" default=\"1\"/></SubTree></TreeNodesModel>",
            "<SubTree ID=\"Sub\"/>"
        ),
    ),
    case(
        C::BadModel,
        Error,
        Rejects,
        with_sub!(
            "<TreeNodesModel><SubTree ID=\"Sub\"><input_port default=\"1\"/></SubTree></TreeNodesModel>",
            "<SubTree ID=\"Sub\"/>"
        ),
        with_sub!(
            "<TreeNodesModel><SubTree ID=\"Sub\"><input_port name=\"goal\" default=\"1\"/></SubTree></TreeNodesModel>",
            "<SubTree ID=\"Sub\"/>"
        ),
    ),
    case(
        C::BadModel,
        Warning,
        Accepts,
        "<root BTCPP_format=\"4\">\n<TreeNodesModel><Widget ID=\"W\"/><Action ID=\"First\"/></TreeNodesModel>\n\
         <BehaviorTree ID=\"Main\"><First/></BehaviorTree>\n</root>",
        "<root BTCPP_format=\"4\">\n<TreeNodesModel><Action ID=\"First\"/></TreeNodesModel>\n\
         <BehaviorTree ID=\"Main\"><First/></BehaviorTree>\n</root>",
    ),
    // ---- IDs and names ----
    case(
        C::MissingId,
        Error,
        Rejects,
        "<Action/>",
        "<Action ID=\"AlwaysSuccess\"/>",
    ),
    case(
        C::MissingId,
        Error,
        Rejects,
        with_sub!("", "<SubTree/>"),
        with_sub!("", "<SubTree ID=\"Sub\"/>"),
    ),
    case(
        C::MissingId,
        Error,
        Rejects,
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Other\">\n\
         <BehaviorTree><AlwaysSuccess/></BehaviorTree>\n\
         <BehaviorTree ID=\"Other\"><AlwaysSuccess/></BehaviorTree>\n</root>",
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Other\">\n\
         <BehaviorTree ID=\"First\"><AlwaysSuccess/></BehaviorTree>\n\
         <BehaviorTree ID=\"Other\"><AlwaysSuccess/></BehaviorTree>\n</root>",
    ),
    case(
        C::MissingId,
        Error,
        Rejects,
        "<root BTCPP_format=\"4\">\n<TreeNodesModel/>\n<Action/>\n\
         <BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree>\n</root>",
        "<root BTCPP_format=\"4\">\n<TreeNodesModel/>\n\
         <BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree>\n</root>",
    ),
    case(
        C::InvalidName,
        Error,
        Rejects,
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"My Tree\"><AlwaysSuccess/></BehaviorTree></root>",
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"MyTree\"><AlwaysSuccess/></BehaviorTree></root>",
    ),
    case(
        C::InvalidName,
        Error,
        Rejects,
        "<AlwaysSuccess name=\"first&#1;second\"/>",
        "<AlwaysSuccess name=\"first second.third/fourth\"/>",
    ),
    case(
        C::InvalidName,
        Error,
        Rejects,
        with_sub!(
            "<TreeNodesModel><SubTree ID=\"Sub\"><input_port name=\"1goal\" default=\"1\"/></SubTree></TreeNodesModel>",
            "<SubTree ID=\"Sub\"/>"
        ),
        with_sub!(
            "<TreeNodesModel><SubTree ID=\"Sub\"><input_port name=\"goal1\" default=\"1\"/></SubTree></TreeNodesModel>",
            "<SubTree ID=\"Sub\"/>"
        ),
    ),
    case(
        C::InvalidName,
        Error,
        Rejects,
        "<My.Node/>",
        "<SayMessage/>",
    ),
    case(
        C::InvalidName,
        Warning,
        Accepts,
        "<AlwaysSuccess goal.x=\"1\"/>",
        "<AlwaysSuccess _description=\"1\"/>",
    ),
    case(
        C::InvalidName,
        Warning,
        Accepts,
        "<root BTCPP_format=\"4\">\n<TreeNodesModel><Action ID=\"Bad:Name\"/><Action ID=\"Good\"/></TreeNodesModel>\n\
         <BehaviorTree ID=\"Main\"><Good/></BehaviorTree>\n</root>",
        "<root BTCPP_format=\"4\">\n<TreeNodesModel><Action ID=\"Good\"/></TreeNodesModel>\n\
         <BehaviorTree ID=\"Main\"><Good/></BehaviorTree>\n</root>",
    ),
    case(
        C::ReservedNodeName,
        Error,
        Rejects,
        "<Root/>",
        "<AlwaysSuccess name=\"Root\"/>",
    ),
    case(
        C::ReservedNodeName,
        Error,
        Rejects,
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"root\"><AlwaysSuccess/></BehaviorTree></root>",
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"RootTree\"><AlwaysSuccess/></BehaviorTree></root>",
    ),
    case(
        C::TreeIdConflict,
        Error,
        Rejects,
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Sequence\"><AlwaysSuccess/></BehaviorTree></root>",
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"MySequence\"><AlwaysSuccess/></BehaviorTree></root>",
    ),
    case(
        C::TreeIdConflict,
        Error,
        Rejects,
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"SayMessage\"><AlwaysSuccess/></BehaviorTree></root>",
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"SayMessages\"><AlwaysSuccess/></BehaviorTree></root>",
    ),
    case(
        C::TreeIdConflict,
        Error,
        Rejects,
        with_sub!("", "<SubTree ID=\"Sleep\"/>"),
        with_sub!("", "<SubTree ID=\"Sub\"/>"),
    ),
    case(
        C::DuplicateTreeId,
        Error,
        Accepts,
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\">\n\
         <BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree>\n\
         <BehaviorTree ID=\"Main\"><AlwaysFailure/></BehaviorTree>\n</root>",
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\">\n\
         <BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree>\n\
         <BehaviorTree ID=\"Other\"><AlwaysFailure/></BehaviorTree>\n</root>",
    ),
    // ---- Tree structure ----
    case(
        C::EmptyTree,
        Error,
        Rejects,
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Main\"/></root>",
        "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
    ),
    case(
        C::MultipleRoots,
        Error,
        Rejects,
        "<AlwaysSuccess/><AlwaysFailure/>",
        "<Sequence><AlwaysSuccess/><AlwaysFailure/></Sequence>",
    ),
    case(
        C::MissingMainTree,
        Error,
        MainTreeFails,
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Mian\"><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\"><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>",
    ),
    case(
        C::AmbiguousMainTree,
        Warning,
        MainTreeFails,
        "<root BTCPP_format=\"4\">\n<BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree>\n\
         <BehaviorTree ID=\"Other\"><AlwaysSuccess/></BehaviorTree>\n</root>",
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\">\n\
         <BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree>\n\
         <BehaviorTree ID=\"Other\"><AlwaysSuccess/></BehaviorTree>\n</root>",
    ),
    // ---- Node types ----
    case(
        C::UnknownNodeType,
        Error,
        Rejects,
        "<SayMesage/>",
        "<SayMessage/>",
    ),
    case(
        C::UnknownNodeType,
        Error,
        Rejects,
        "<Action ID=\"SayMesage\"/>",
        "<Action ID=\"SayMessage\"/>",
    ),
    case(
        C::UnknownNodeType,
        Error,
        Rejects,
        with_sub!("", "<Sub/>"),
        with_sub!("", "<SubTree ID=\"Sub\"/>"),
    ),
    case(
        C::UnexpectedId,
        Error,
        Rejects,
        "<Sequence ID=\"Sequence\"><AlwaysSuccess/></Sequence>",
        "<Sequence><AlwaysSuccess/></Sequence>",
    ),
    case(
        C::KindMismatch,
        Warning,
        Accepts,
        "<Condition ID=\"AlwaysSuccess\"/>",
        "<Action ID=\"AlwaysSuccess\"/>",
    ),
    // ---- Children ----
    case(
        C::ChildCount,
        Error,
        Rejects,
        "<Inverter/>",
        "<Inverter><AlwaysSuccess/></Inverter>",
    ),
    case(
        C::ChildCount,
        Error,
        Rejects,
        "<Inverter><AlwaysSuccess/><AlwaysFailure/></Inverter>",
        "<Inverter><Sequence><AlwaysSuccess/><AlwaysFailure/></Sequence></Inverter>",
    ),
    case(
        C::ChildCount,
        Error,
        Rejects,
        "<Sequence/>",
        "<Sequence><AlwaysSuccess/></Sequence>",
    ),
    case(
        C::ChildCount,
        Error,
        Rejects,
        "<SayMessage><AlwaysSuccess/></SayMessage>",
        "<Sequence><SayMessage/><AlwaysSuccess/></Sequence>",
    ),
    case(
        C::ChildCount,
        Error,
        Rejects,
        "<TryCatch><AlwaysSuccess/></TryCatch>",
        "<TryCatch><AlwaysSuccess/><AlwaysFailure/></TryCatch>",
    ),
    case(
        C::ChildCount,
        Error,
        TickFails,
        "<IfThenElse><AlwaysSuccess/></IfThenElse>",
        "<IfThenElse><AlwaysSuccess/><AlwaysFailure/></IfThenElse>",
    ),
    case(
        C::ChildCount,
        Error,
        TickFails,
        "<WhileDoElse><AlwaysSuccess/><AlwaysSuccess/><AlwaysSuccess/><AlwaysSuccess/></WhileDoElse>",
        "<WhileDoElse><AlwaysSuccess/><AlwaysSuccess/><AlwaysSuccess/></WhileDoElse>",
    ),
    case(
        C::ChildCount,
        Error,
        TickFails,
        "<Switch2 variable=\"{v}\" case_1=\"1\" case_2=\"2\"><AlwaysSuccess/><AlwaysSuccess/></Switch2>",
        "<Switch2 variable=\"{v}\" case_1=\"1\" case_2=\"2\"><AlwaysSuccess/><AlwaysSuccess/><AlwaysFailure/></Switch2>",
    ),
    case(
        C::ChildCount,
        Error,
        Rejects,
        with_sub!("", "<SubTree ID=\"Sub\"><AlwaysSuccess/></SubTree>"),
        with_sub!("", "<SubTree ID=\"Sub\"/>"),
    ),
    case(
        C::ReactiveSequenceChild,
        Error,
        Rejects,
        "<ReactiveSequence><Condition ID=\"CheckFlag\" flag=\"true\"/><AlwaysSuccess/></ReactiveSequence>",
        "<ReactiveSequence><CheckFlag flag=\"true\"/><AlwaysSuccess/></ReactiveSequence>",
    ),
    case(
        C::ReactiveSequenceChild,
        Error,
        Rejects,
        "<ReactiveSequence><AsyncSequence><AlwaysSuccess/></AsyncSequence>\
         <AsyncSequence><AlwaysSuccess/></AsyncSequence></ReactiveSequence>",
        "<ReactiveSequence><AsyncSequence><AlwaysSuccess/></AsyncSequence>\
         <Sequence><AlwaysSuccess/></Sequence></ReactiveSequence>",
    ),
    // ---- Ports ----
    case(
        C::UnknownPort,
        Error,
        Rejects,
        "<SayMessage mesage=\"hello\"/>",
        "<SayMessage message=\"hello\"/>",
    ),
    case(
        C::UnknownPort,
        Warning,
        Accepts,
        with_sub!(
            sub_model!(),
            "<SubTree ID=\"Sub\" goal=\"{goal}\" gaol=\"{goal}\"/>"
        ),
        with_sub!(sub_model!(), "<SubTree ID=\"Sub\" goal=\"{goal}\"/>"),
    ),
    case(
        C::MissingRequiredPort,
        Warning,
        TickFails,
        "<Sleep/>",
        "<Sleep msec=\"10\"/>",
    ),
    case(
        C::MissingRequiredPort,
        Warning,
        TickFails,
        "<IncrementCounter/>",
        "<IncrementCounter counter=\"{counter}\"/>",
    ),
    case(
        C::MissingRequiredPort,
        Error,
        Rejects,
        with_sub!(sub_model!(), "<SubTree ID=\"Sub\"/>"),
        with_sub!(sub_model!(), "<SubTree ID=\"Sub\" _autoremap=\"true\"/>"),
    ),
    case(
        C::MissingRequiredPort,
        Error,
        Rejects,
        with_sub!(
            sub_model!(),
            "<SubTree ID=\"Sub\" _autoremap=\"false\" result=\"{r}\"/>"
        ),
        with_sub!(
            sub_model!(),
            "<SubTree ID=\"Sub\" _autoremap=\"false\" goal=\"5\"/>"
        ),
    ),
    case(
        C::InvalidPortValue,
        Error,
        Rejects,
        "<Sleep msec=\"ten\"/>",
        "<Sleep msec=\"{delay}\"/>",
    ),
    case(
        C::InvalidPortValue,
        Error,
        Rejects,
        "<Sleep msec=\"-1\"/>",
        "<Sleep msec=\"12abc\"/>",
    ),
    case(
        C::InvalidPortValue,
        Error,
        Rejects,
        "<Repeat num_cycles=\"99999999999\"><AlwaysSuccess/></Repeat>",
        "<Repeat num_cycles=\"-1\"><AlwaysSuccess/></Repeat>",
    ),
    case(
        C::InvalidPortValue,
        Error,
        Rejects,
        "<CheckFlag flag=\"yes\"/>",
        "<CheckFlag flag=\"TRUE\"/>",
    ),
    case(
        C::OutputPortLiteral,
        Warning,
        Accepts,
        "<WriteValues int_value=\"5\"/>",
        "<WriteValues int_value=\"{int_value}\" double_value=\"{=}\" string_value=\"=\"/>",
    ),
    case(
        C::OutputPortLiteral,
        Warning,
        Accepts,
        with_sub!(
            sub_model!(),
            "<SubTree ID=\"Sub\" goal=\"1\" result=\"done\"/>"
        ),
        with_sub!(
            sub_model!(),
            "<SubTree ID=\"Sub\" goal=\"1\" result=\"{done}\"/>"
        ),
    ),
    case(
        C::BadBlackboardRef,
        Warning,
        Accepts,
        "<SayMessage message=\"{text\"/>",
        "<SayMessage message=\"{text}\"/>",
    ),
    case(
        C::BadBlackboardRef,
        Warning,
        Accepts,
        "<UnsetBlackboard key=\"{}\"/>",
        "<UnsetBlackboard key=\"{key}\"/>",
    ),
    case(
        C::BadBlackboardRef,
        Warning,
        Accepts,
        "<SayMessage message=\"{ text }\"/>",
        "<SayMessage message=\" {text} \"/>",
    ),
    case(
        C::BadBlackboardRef,
        Warning,
        Accepts,
        with_sub!("", "<SubTree ID=\"Sub\" goal=\"{goal\"/>"),
        with_sub!("", "<SubTree ID=\"Sub\" goal=\"{goal}\"/>"),
    ),
    // ---- Conditions and attribute values ----
    case(
        C::EmptyCondition,
        Error,
        Rejects,
        "<AlwaysSuccess _skipIf=\"\"/>",
        "<AlwaysSuccess _skipIf=\"false\"/>",
    ),
    case(
        C::EmptyCondition,
        Error,
        Rejects,
        "<AlwaysSuccess _onSuccess=\"  \"/>",
        "<AlwaysSuccess _onSuccess=\"done := true\"/>",
    ),
    case(
        C::ScriptSyntax,
        Error,
        Rejects,
        "<AlwaysSuccess _skipIf=\"x == (1\"/>",
        "<AlwaysSuccess _skipIf=\"x == (1)\"/>",
    ),
    case(
        C::ScriptSyntax,
        Error,
        Rejects,
        "<AlwaysSuccess _post=\"a := 3foo\"/>",
        "<AlwaysSuccess _post=\"a := 3\"/>",
    ),
    case(
        C::ScriptSyntax,
        Error,
        Rejects,
        "<AlwaysSuccess _while=\"a ? b\"/>",
        "<AlwaysSuccess _while=\"a ? b : c\"/>",
    ),
    case(
        C::ScriptSyntax,
        Error,
        Rejects,
        "<Script code=\"x := \"/>",
        "<Script code=\"x := 1\"/>",
    ),
    case(
        C::ScriptSyntax,
        Error,
        Rejects,
        "<ScriptCondition code=\"1 +\"/>",
        "<ScriptCondition code=\"1 + 1 == 2\"/>",
    ),
    case(
        C::ScriptSyntax,
        Error,
        Rejects,
        "<Precondition if=\"a ==\" else=\"FAILURE\"><AlwaysSuccess/></Precondition>",
        "<Precondition if=\"a == 1\" else=\"FAILURE\"><AlwaysSuccess/></Precondition>",
    ),
    case(
        C::SuspiciousAmpersand,
        Warning,
        Accepts,
        "<AlwaysSuccess name=\"Tom & Jerry\"/>",
        "<AlwaysSuccess name=\"Tom &amp; Jerry\"/>",
    ),
    // ---- SubTrees ----
    case(
        C::MissingSubTree,
        Error,
        Rejects,
        with_sub!("", "<SubTree ID=\"Sbu\"/>"),
        with_sub!("", "<SubTree ID=\"Sub\"/>"),
    ),
    case(
        C::SelfSubTree,
        Error,
        Rejects,
        "<Sequence><AlwaysSuccess/><SubTree ID=\"Main\"/></Sequence>",
        "<Sequence><AlwaysSuccess/></Sequence>",
    ),
    case(
        C::CircularSubTree,
        Error,
        Rejects,
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\">\n\
         <BehaviorTree ID=\"Main\"><SubTree ID=\"Sub\"/></BehaviorTree>\n\
         <BehaviorTree ID=\"Sub\"><Sequence><SubTree ID=\"Main\"/></Sequence></BehaviorTree>\n</root>",
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\">\n\
         <BehaviorTree ID=\"Main\"><SubTree ID=\"Sub\"/></BehaviorTree>\n\
         <BehaviorTree ID=\"Sub\"><Sequence><AlwaysSuccess/></Sequence></BehaviorTree>\n</root>",
    ),
    case(
        C::BadRemapping,
        Error,
        Rejects,
        with_sub!("", "<SubTree ID=\"Sub\" _autoremap=\"yes\"/>"),
        with_sub!("", "<SubTree ID=\"Sub\" _autoremap=\"True\"/>"),
    ),
    case(
        C::BadRemapping,
        Error,
        Rejects,
        with_sub!("", "<SubTree ID=\"Sub\" _autoremap=\"{=}\"/>"),
        with_sub!("", "<SubTree ID=\"Sub\" _autoremap=\"0\"/>"),
    ),
    case(
        C::BadRemapping,
        Warning,
        Accepts,
        with_sub!("", "<SubTree ID=\"Sub\" goal=\"=\"/>"),
        with_sub!("", "<SubTree ID=\"Sub\" goal=\"{=}\"/>"),
    ),
    case(
        C::BadRemapping,
        Warning,
        Accepts,
        with_sub!("", "<SubTree ID=\"Sub\" __shared_blackboard=\"true\"/>"),
        with_sub!("", "<SubTree ID=\"Sub\" _autoremap=\"true\"/>"),
    ),
    case(
        C::BadRemapping,
        Warning,
        Accepts,
        "<Sequence _autoremap=\"true\"><AlwaysSuccess/></Sequence>",
        "<Sequence><AlwaysSuccess/></Sequence>",
    ),
    case(
        C::DuplicateSubTreeName,
        Error,
        Rejects,
        with_sub!(
            "",
            "<Sequence><SubTree ID=\"Sub\" name=\"step\"/><Sequence><SubTree ID=\"Sub\" name=\"step\"/></Sequence></Sequence>"
        ),
        with_sub!(
            "",
            "<Sequence><SubTree ID=\"Sub\" name=\"step\"/><Sequence><SubTree ID=\"Sub\" name=\"step2\"/></Sequence></Sequence>"
        ),
    ),
    case(
        C::DuplicateSubTreeName,
        Error,
        Rejects,
        with_sub!("", "<Sequence><SubTree ID=\"Sub\" name=\"\"/></Sequence>"),
        with_sub!("", "<Sequence><SubTree ID=\"Sub\"/></Sequence>"),
    ),
];

/// The codes that need a project with more than one file, a document that
/// the test makes, or a project state. Each one has its own tests below.
const OTHER_TESTS: &[ProblemCode] = &[
    ProblemCode::NestingTooDeep,
    ProblemCode::TreeNotIncluded,
    ProblemCode::UnconnectedNode,
];

#[test]
fn each_code_has_a_case() {
    for &code in ProblemCode::ALL {
        let in_table = CASES.iter().any(|c| c.code == code);
        assert!(
            in_table || OTHER_TESTS.contains(&code),
            "{code} has no test case"
        );
    }
}

#[test]
fn bad_cases_give_the_problem() {
    let mut failures = Vec::new();
    for (i, case) in CASES.iter().enumerate() {
        let found = problems(case.bad);
        let matching = found
            .iter()
            .any(|p| p.code == case.code && p.severity == case.severity);
        if !matching {
            failures.push(format!(
                "case {i} ({} {}): expected the problem, got:\n{}\n{}",
                case.severity,
                case.code,
                show(&found),
                document(case.bad)
            ));
        }
        // An error that BT.CPP does not report is possible (BT.CPP ignores
        // a part of the file), but a BT.CPP failure must be an error, except
        // the failures that depend on the executor (the main tree, a port
        // that the node reads).
        if case.btcpp == Rejects && !found.iter().any(Problem::is_error) {
            failures.push(format!("case {i} ({}): BT.CPP rejects it", case.code));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn good_cases_give_no_problem() {
    let mut failures = Vec::new();
    for (i, case) in CASES.iter().enumerate() {
        let found = problems(case.good);
        if !found.is_empty() {
            failures.push(format!(
                "case {i} ({}): expected no problem, got:\n{}\n{}",
                case.code,
                show(&found),
                document(case.good)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Each message is one or more sentences in simple English: it starts
/// with a capital letter, ends with a full stop, has no double spaces, and
/// has more than one sentence (what is wrong, and how to correct it).
#[test]
fn messages_are_sentences() {
    for case in CASES {
        for p in problems(case.bad) {
            let m = &p.message;
            assert!(
                m.chars()
                    .next()
                    .is_some_and(|c| c.is_uppercase() || c == '"' || c == '<'),
                "{m}"
            );
            assert!(m.ends_with('.'), "{m}");
            assert!(!m.contains("  "), "{m}");
            assert!(!m.contains('\n'), "{m}");
            assert!(m.matches(". ").count() >= 1, "only one sentence: {m}");
        }
    }
}

/// Writes the documents of the cases into the directory
/// `$FASTTICK_DUMP_CASES`, for the BT.CPP check (see the module
/// documentation).
#[test]
#[ignore = "writes files for a manual check with BT.CPP"]
fn dump_cases() {
    let Ok(dir) = std::env::var("FASTTICK_DUMP_CASES") else {
        return;
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut list = String::new();
    for (i, case) in CASES.iter().enumerate() {
        for (kind, text) in [("bad", case.bad), ("good", case.good)] {
            let name = format!("{i:03}_{}_{kind}.xml", case.code);
            std::fs::write(dir.join(&name), document(text)).unwrap();
            let expected = match (kind, case.btcpp) {
                ("good", _) | (_, Accepts) | (_, TickFails) => "OK",
                (_, Rejects) => "REJECT",
                (_, MainTreeFails) => "MAIN",
            };
            list.push_str(&format!("{name}\t{expected}\n"));
        }
    }
    std::fs::write(dir.join("expected.txt"), list).unwrap();
}

// --------------------------------------------------------------------
// Details of the rules
// --------------------------------------------------------------------

#[test]
fn problem_fields_point_to_the_node_and_the_attribute() {
    let (project, id) = setup("<Sequence><SayMessage mesage=\"hello\"/></Sequence>");
    let found = validate_file(&project, id);
    let p = single(&found, C::UnknownPort);
    assert_eq!(p.severity, Error);
    assert_eq!(p.file, id);
    assert_eq!(p.tree.as_deref(), Some("Main"));
    assert_eq!(p.attribute.as_deref(), Some("mesage"));
    assert_eq!(tag_of(&project, p), "SayMessage");
    assert_eq!(p.line_col(&project), Some((3, 15)));
    assert!(p.message.contains("\"mesage\""), "{}", p.message);
    assert!(p.message.contains("message"), "the message lists the ports");
}

#[test]
fn file_problems_point_to_the_root_element() {
    let (project, id) =
        setup("<root><BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>");
    let found = validate_file(&project, id);
    let p = single(&found, C::FormatVersion);
    assert_eq!(p.tree, None);
    assert_eq!(p.attribute.as_deref(), Some("BTCPP_format"));
    assert_eq!(tag_of(&project, p), "root");
}

#[test]
fn a_document_without_a_root_element() {
    // A new file has an empty <root BTCPP_format="4">: no problem.
    let (project, id) = Project::with_file("new", BtFile::new_empty());
    assert!(validate_file(&project, id).is_empty());
    let file = BtFile::from_document(Document::default(), None);
    let (project, id) = Project::with_file("empty", file);
    let found = validate_file(&project, id);
    let p = single(&found, C::RootElement);
    assert_eq!(p.severity, Error);
    assert_eq!(p.node, None);
}

#[test]
fn a_wrong_root_element_gives_one_problem() {
    // For example a ROS package manifest: the children are not checked.
    let found = problems(
        "<?xml version=\"1.0\"?>\n<package format=\"3\"><name>x</name><depend>y</depend></package>",
    );
    assert_eq!(found.len(), 1, "{}", show(&found));
    assert_eq!(found[0].code, C::RootElement);
    assert_eq!(found[0].severity, Error);
    assert!(
        found[0].message.contains("<package>"),
        "{}",
        found[0].message
    );
}

#[test]
fn unknown_type_messages() {
    let found = problems("<Sequence><SayMesage/><Action ID=\"Nope\"/></Sequence>");
    let unknown = with_code(&found, C::UnknownNodeType);
    assert_eq!(unknown.len(), 2, "{}", show(&found));
    assert!(unknown[0].message.contains("\"SayMesage\""));
    assert!(unknown[1].message.contains("\"Nope\""));
    // A tree in the compact form: the message tells to use <SubTree>.
    let found = problems(with_sub!("", "<Sub/>"));
    let p = single(&found, C::UnknownNodeType);
    assert!(p.message.contains("<SubTree ID=\"Sub\"/>"), "{}", p.message);
}

#[test]
fn child_count_messages() {
    let cases = [
        (
            "<Inverter/>",
            "\"Inverter\" must have exactly 1 child, but it has no children. Add a child node.",
        ),
        (
            "<Inverter><AlwaysSuccess/><AlwaysSuccess/></Inverter>",
            "\"Inverter\" must have exactly 1 child, but it has 2 children. Put the child nodes under one control node, for example a Sequence.",
        ),
        (
            "<AlwaysSuccess><AlwaysSuccess/></AlwaysSuccess>",
            "\"AlwaysSuccess\" must have no children, but it has 1 child. Remove the child nodes: this node cannot have children.",
        ),
        (
            "<WhileDoElse><AlwaysSuccess/><AlwaysSuccess/><AlwaysSuccess/><AlwaysSuccess/><AlwaysSuccess/></WhileDoElse>",
            "\"WhileDoElse\" must have 2 to 3 children, but it has 5 children. Remove 2 child nodes.",
        ),
        (
            "<Switch3 variable=\"{v}\" case_1=\"1\" case_2=\"2\" case_3=\"3\"><AlwaysSuccess/></Switch3>",
            "\"Switch3\" must have exactly 4 children, but it has 1 child. Add 3 child nodes.",
        ),
    ];
    for (body, expected) in cases {
        let found = problems(body);
        let p = single(&found, C::ChildCount);
        assert_eq!(p.message, expected);
    }
}

#[test]
fn child_rules_of_project_models() {
    // A control node of a project model needs 1 or more children, a
    // decorator exactly 1, an action and a condition none.
    let doc = "<root BTCPP_format=\"4\">\n<TreeNodesModel><Control ID=\"MyControl\"/>\
               <Decorator ID=\"MyDecorator\"/><Condition ID=\"MyCondition\"/></TreeNodesModel>\n\
               <BehaviorTree ID=\"Main\"><Sequence>\n\
               <MyControl/>\n<MyDecorator><MyCondition/><MyCondition/></MyDecorator>\n\
               <MyCondition><AlwaysSuccess/></MyCondition>\n\
               <MyControl><MyCondition/><MyCondition/><MyCondition/></MyControl>\n\
               </Sequence></BehaviorTree>\n</root>";
    let (project, id) = setup(doc);
    let found = validate_file(&project, id);
    let tags: Vec<String> = with_code(&found, C::ChildCount)
        .into_iter()
        .map(|p| tag_of(&project, p))
        .collect();
    assert_eq!(
        tags,
        ["MyControl", "MyDecorator", "MyCondition"],
        "{}",
        show(&found)
    );
}

#[test]
fn subtree_cycle_path() {
    let doc = "<root BTCPP_format=\"4\" main_tree_to_execute=\"A\">\n\
               <BehaviorTree ID=\"A\"><SubTree ID=\"B\"/></BehaviorTree>\n\
               <BehaviorTree ID=\"B\"><Sequence><SubTree ID=\"C\"/><AlwaysSuccess/></Sequence></BehaviorTree>\n\
               <BehaviorTree ID=\"C\"><SubTree ID=\"A\"/></BehaviorTree>\n\
               <BehaviorTree ID=\"D\"><SubTree ID=\"A\"/></BehaviorTree>\n\
               </root>";
    let found = problems(doc);
    let cycles = with_code(&found, C::CircularSubTree);
    // One problem for each <SubTree> of the cycle; D is not in the cycle.
    let trees: Vec<&str> = cycles.iter().map(|p| p.tree.as_deref().unwrap()).collect();
    assert_eq!(trees, ["A", "B", "C"], "{}", show(&found));
    assert!(
        cycles[0].message.contains("A -> B -> C -> A"),
        "{}",
        cycles[0].message
    );
    assert!(
        cycles[1].message.contains("B -> C -> A -> B"),
        "{}",
        cycles[1].message
    );
    assert!(
        cycles[2].message.contains("C -> A -> B -> C"),
        "{}",
        cycles[2].message
    );
    assert!(cycles.iter().all(|p| p.attribute.as_deref() == Some("ID")));
    assert!(with_code(&found, C::SelfSubTree).is_empty());
}

#[test]
fn self_subtree_is_not_a_cycle_problem() {
    let found = problems("<Sequence><SubTree ID=\"Main\"/></Sequence>");
    let p = single(&found, C::SelfSubTree);
    assert_eq!(p.severity, Error);
    assert!(with_code(&found, C::CircularSubTree).is_empty());
}

#[test]
fn nesting_depth_limit() {
    // VerifyXML: the root node of the tree has depth 1. A node at depth
    // 257 is too deep.
    let nested = |levels: usize| {
        let mut body = String::new();
        for _ in 1..levels {
            body.push_str("<Inverter>");
        }
        body.push_str("<AlwaysSuccess/>");
        for _ in 1..levels {
            body.push_str("</Inverter>");
        }
        body
    };
    let found = problems(&nested(256));
    assert!(found.is_empty(), "{}", show(&found));
    let (project, id) = setup(&nested(257));
    let found = validate_file(&project, id);
    let p = single(&found, C::NestingTooDeep);
    assert_eq!(p.severity, Error);
    assert_eq!(tag_of(&project, p), "AlwaysSuccess");
    // One problem for each tree, at the first node that is too deep.
    let (project, id) = setup(&nested(300));
    let found = validate_file(&project, id);
    let p = single(&found, C::NestingTooDeep);
    assert_eq!(tag_of(&project, p), "Inverter");
    assert!(p.message.contains("257 levels"), "{}", p.message);
}

#[test]
fn required_ports_and_defaults() {
    // Delay has a required port; Parallel and RunOnce have defaults.
    let found = problems(
        "<Sequence><Delay><AlwaysSuccess/></Delay><Parallel><AlwaysSuccess/></Parallel>\
         <RunOnce><AlwaysSuccess/></RunOnce></Sequence>",
    );
    let p = single(&found, C::MissingRequiredPort);
    assert_eq!(p.attribute.as_deref(), Some("delay_msec"));
    assert_eq!(p.severity, Warning);
    // Output ports are never required.
    assert!(problems("<WriteValues/>").is_empty());
}

#[test]
fn subtree_model_ports() {
    // A port of a SubTree model with an empty default is mandatory
    // (recursivelyCreateSubtree checks `defaultValueString().empty()`).
    let models = "<TreeNodesModel><SubTree ID=\"Sub\">\
                  <input_port name=\"a\" default=\"\"/><output_port name=\"b\"/>\
                  <inout_port name=\"c\" default=\"x\"/></SubTree></TreeNodesModel>";
    let doc = format!(
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\">{models}\
         <BehaviorTree ID=\"Main\"><SubTree ID=\"Sub\"/></BehaviorTree>\
         <BehaviorTree ID=\"Sub\"><AlwaysSuccess/></BehaviorTree></root>"
    );
    let found = problems(&doc);
    let names: Vec<&str> = with_code(&found, C::MissingRequiredPort)
        .iter()
        .map(|p| p.attribute.as_deref().unwrap())
        .collect();
    assert_eq!(names, ["a", "b"], "{}", show(&found));
    assert!(found.iter().all(|p| p.severity == Error));
    // _autoremap="true" remaps all ports.
    let doc = doc.replace(
        "<SubTree ID=\"Sub\"/>",
        "<SubTree ID=\"Sub\" _autoremap=\"1\"/>",
    );
    assert!(problems(&doc).is_empty());
    // A remapping to a literal also counts.
    let doc = doc.replace("_autoremap=\"1\"", "a=\"1\" b=\"{b}\"");
    assert!(problems(&doc).is_empty());
}

#[test]
fn subtree_without_model_accepts_any_remapping() {
    let found = problems(with_sub!(
        "",
        "<SubTree ID=\"Sub\" anything=\"{x}\" other=\"5\"/>"
    ));
    assert!(found.is_empty(), "{}", show(&found));
}

#[test]
fn output_port_values() {
    // "=" and "{=}" give the same name; a reference with spaces around it
    // is a reference too (isBlackboardPointer).
    for value in ["=", "{=}", "{x}", " {x} ", "{@x}"] {
        let body = format!("<WriteValues string_value=\"{value}\"/>");
        assert!(problems(&body).is_empty(), "{value}");
    }
    // createNodeFromXML converts "=" like a literal, so "=" is not correct
    // for a strongly typed port that is not a string. ("{=}" is correct.)
    let found = problems("<WriteValues int_value=\"=\"/>");
    let p = single(&found, C::InvalidPortValue);
    assert_eq!(p.attribute.as_deref(), Some("int_value"));
    assert!(with_code(&found, C::OutputPortLiteral).is_empty());
    for value in ["", "5", "x"] {
        let body = format!("<WriteValues string_value=\"{value}\"/>");
        let found = problems(&body);
        let p = single(&found, C::OutputPortLiteral);
        assert_eq!(p.attribute.as_deref(), Some("string_value"));
    }
    // An in/out port can have a literal value: the node reads it.
    assert!(problems("<IncrementCounter counter=\"5\"/>").is_empty());
}

#[test]
fn output_port_literal_is_converted_too() {
    // createNodeFromXML converts a literal value of each strongly typed
    // port, also of an output port.
    let found = problems("<WriteValues int_value=\"five\"/>");
    assert_eq!(
        with_code(&found, C::InvalidPortValue).len(),
        1,
        "{}",
        show(&found)
    );
    assert_eq!(with_code(&found, C::OutputPortLiteral).len(), 1);
}

#[test]
fn values_that_are_not_converted() {
    // No conversion for a blackboard reference, for generic ports and for
    // types without a known conversion.
    for body in [
        "<Sleep msec=\"{x}\"/>",
        "<ScriptCondition code=\"anything at all\"/>",
        "<Precondition if=\"x\" else=\"NOT_A_STATUS\"><AlwaysSuccess/></Precondition>",
        "<ReadPose pose=\"not a pose\" distance=\"{d}\"/>",
    ] {
        let found = problems(body);
        assert!(with_code(&found, C::InvalidPortValue).is_empty(), "{body}");
    }
}

#[test]
fn blackboard_reference_messages() {
    let cases = [
        ("{key", "does not end with }"),
        ("key}", "does not start with {"),
        ("{}", "has no key"),
        ("{@}", "has no key"),
        ("{a}{b}", "the key \"a}{b\""),
        ("{ key}", "starts or ends with white space"),
    ];
    for (value, text) in cases {
        let body = format!("<SayMessage message=\"{value}\"/>");
        let found = problems(&body);
        let p = single(&found, C::BadBlackboardRef);
        assert!(p.message.contains(text), "{value}: {}", p.message);
        assert_eq!(p.attribute.as_deref(), Some("message"));
    }
    // JSON values and text with braces inside are not references.
    for value in ["json:{&quot;x&quot;:1}", "a {b} c", "{=}"] {
        let body = format!("<SayMessage message=\"{value}\"/>");
        assert!(problems(&body).is_empty(), "{value}");
    }
}

#[test]
fn ampersand_that_changes_the_value() {
    // tinyxml2 reads `a &lt; b && c` as `a < b  b c` (see the xml module).
    let found = problems("<SayMessage message=\"a &lt; b &amp;&amp; c &amp d\"/>");
    let p = single(&found, C::SuspiciousAmpersand);
    assert_eq!(p.attribute.as_deref(), Some("message"));
    let found = problems("<SayMessage message=\"a &lt; b && c\"/>");
    let p = single(&found, C::SuspiciousAmpersand);
    assert!(
        p.message
            .contains("BT.CPP reads this value as \"a < b  b c\""),
        "{}",
        p.message
    );
    // On the tree element and on <root> too.
    let found = problems(
        "<root BTCPP_format=\"4\" note=\"R&D\"><BehaviorTree ID=\"Main\" note=\"&\">\
         <AlwaysSuccess/></BehaviorTree></root>",
    );
    assert_eq!(
        with_code(&found, C::SuspiciousAmpersand).len(),
        2,
        "{}",
        show(&found)
    );
}

#[test]
fn condition_attributes() {
    // Each pre- and post-condition is checked; other attributes that
    // start with "_" are ignored, as in BT.CPP.
    for name in [
        "_failureIf",
        "_successIf",
        "_skipIf",
        "_while",
        "_onHalted",
        "_onFailure",
        "_onSuccess",
        "_post",
    ] {
        let body = format!("<AlwaysSuccess {name}=\"\"/>");
        let found = problems(&body);
        assert_eq!(
            single(&found, C::EmptyCondition).attribute.as_deref(),
            Some(name)
        );
        let body = format!("<AlwaysSuccess {name}=\"x == 1\"/>");
        assert!(problems(&body).is_empty(), "{name}");
    }
    assert!(problems("<AlwaysSuccess _uid=\"3\" _fullpath=\"x\" _other=\"\"/>").is_empty());
}

#[test]
fn duplicate_subtree_names() {
    // The same name in different trees, or under different SubTree
    // instances, is correct.
    let doc = "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\">\n\
               <BehaviorTree ID=\"Main\"><Sequence><SubTree ID=\"A\" name=\"x\"/>\
               <SubTree ID=\"B\" name=\"y\"/></Sequence></BehaviorTree>\n\
               <BehaviorTree ID=\"A\"><SubTree ID=\"B\" name=\"x\"/></BehaviorTree>\n\
               <BehaviorTree ID=\"B\"><AlwaysSuccess/></BehaviorTree>\n</root>";
    assert!(problems(doc).is_empty());
    // An empty name in a tree that is not the main tree: a warning.
    let doc = doc.replace(
        "<SubTree ID=\"B\" name=\"x\"/>",
        "<SubTree ID=\"B\" name=\"\"/>",
    );
    let found = problems(&doc);
    let p = single(&found, C::DuplicateSubTreeName);
    assert_eq!(p.severity, Warning);
    assert_eq!(p.tree.as_deref(), Some("A"));
    // The second node with the name gets the problem, with the line of the
    // first one.
    let found = problems(with_sub!(
        "",
        "<Sequence>\n<SubTree ID=\"Sub\" name=\"s\"/>\n<SubTree ID=\"Sub\" name=\"s\"/>\n</Sequence>"
    ));
    let p = single(&found, C::DuplicateSubTreeName);
    assert!(p.message.contains("(line 5)"), "{}", p.message);
}

#[test]
fn main_tree_detection() {
    // One tree with an ID: it is the main tree.
    assert!(problems("<AlwaysSuccess/>").is_empty());
    // One tree without an ID: BT.CPP names it BehaviorTree_0 and uses it.
    let found =
        problems("<root BTCPP_format=\"4\"><BehaviorTree><AlwaysSuccess/></BehaviorTree></root>");
    assert!(found.is_empty(), "{}", show(&found));
    // The empty name of a SubTree in the main tree is an error.
    let doc = "<root BTCPP_format=\"4\" main_tree_to_execute=\"Other\">\n\
               <BehaviorTree ID=\"Main\"><SubTree ID=\"Other\" name=\"\"/></BehaviorTree>\n\
               <BehaviorTree ID=\"Other\"><SubTree ID=\"Leaf\" name=\"\"/></BehaviorTree>\n\
               <BehaviorTree ID=\"Leaf\"><AlwaysSuccess/></BehaviorTree>\n</root>";
    let found = problems(doc);
    let dups = with_code(&found, C::DuplicateSubTreeName);
    let severities: Vec<(Option<&str>, Severity)> = dups
        .iter()
        .map(|p| (p.tree.as_deref(), p.severity))
        .collect();
    assert_eq!(
        severities,
        [(Some("Main"), Warning), (Some("Other"), Error)]
    );
}

#[test]
fn reactive_sequence_children() {
    // A SubTree is a registered node type ("SubTree"), so it is a correct
    // child of a ReactiveSequence.
    let found = problems(with_sub!(
        "",
        "<ReactiveSequence><SubTree ID=\"Sub\"/><AlwaysSuccess/></ReactiveSequence>"
    ));
    assert!(found.is_empty(), "{}", show(&found));
    // Only the second async child gets the problem.
    let found = problems(
        "<ReactiveSequence><AsyncSequence><AlwaysSuccess/></AsyncSequence>\
         <AsyncSequence><AlwaysSuccess/></AsyncSequence>\
         <AsyncSequence><AlwaysSuccess/></AsyncSequence></ReactiveSequence>",
    );
    assert_eq!(with_code(&found, C::ReactiveSequenceChild).len(), 1);
}

#[test]
fn models_in_the_file_win() {
    // A model in the file is used before a project model with the same ID.
    let doc = "<root BTCPP_format=\"4\">\n<TreeNodesModel><Action ID=\"SayMessage\">\
               <input_port name=\"text\"/></Action></TreeNodesModel>\n\
               <BehaviorTree ID=\"Main\"><SayMessage text=\"x\"/></BehaviorTree>\n</root>";
    assert!(problems(doc).is_empty());
    let found = problems(&doc.replace("text=\"x\"", "message=\"x\""));
    assert_eq!(
        with_code(&found, C::UnknownPort).len(),
        1,
        "{}",
        show(&found)
    );
}

#[test]
fn model_file_problems() {
    // The harness models have no problems.
    let mut project = Project::new("models");
    let id = project.add_model_file(harness_models()).unwrap();
    let found = validate_file(&project, id);
    assert!(found.is_empty(), "{}", show(&found));
    // A model file needs a <TreeNodesModel>.
    let file = BtFile::parse("<root BTCPP_format=\"4\"/>", None).unwrap();
    let id = project.add_bt_file(file, FileRole::Models);
    project.refresh();
    let found = validate_file(&project, id);
    assert_eq!(single(&found, C::RootElement).severity, Warning);
    // A bad port name of a SubTree model in a model file is a warning:
    // BT.CPP reads such a file only if the executor loads it.
    let file = BtFile::parse(
        "<TreeNodesModel><SubTree ID=\"Sub\"><input_port name=\"bad name\"/></SubTree></TreeNodesModel>",
        None,
    )
    .unwrap();
    let id = project.add_bt_file(file, FileRole::Models);
    project.refresh();
    let found = validate_file(&project, id);
    assert_eq!(single(&found, C::InvalidName).severity, Warning);
}

#[test]
fn counts() {
    let found = problems(
        "<root><BehaviorTree ID=\"Main\"><Sequence><Sleep/><Inverter/></Sequence></BehaviorTree></root>",
    );
    let counts = ProblemCounts::of(&found);
    assert_eq!(
        (counts.errors, counts.warnings, counts.infos),
        (1, 2, 0),
        "{}",
        show(&found)
    );
}

// --------------------------------------------------------------------
// Projects with more than one file
// --------------------------------------------------------------------

/// Writes files into a new temporary directory.
fn write_files(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn file_id(project: &Project, dir: &Path, name: &str) -> FileId {
    project.file_id_by_path(dir.join(name)).unwrap()
}

#[test]
fn tree_in_a_file_that_is_not_included() {
    let dir = write_files(&[
        (
            "main.xml",
            "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Main\"><SubTree ID=\"Sub\"/></BehaviorTree></root>",
        ),
        (
            "sub.xml",
            "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Sub\"><AlwaysSuccess/></BehaviorTree></root>",
        ),
    ]);
    let mut project = Project::new("p");
    project.add_file(dir.path().join("main.xml")).unwrap();
    project.add_file(dir.path().join("sub.xml")).unwrap();
    let main = file_id(&project, dir.path(), "main.xml");
    let found = validate_file(&project, main);
    let p = single(&found, C::TreeNotIncluded);
    assert_eq!(p.severity, Info);
    assert!(p.message.contains("sub.xml"), "{}", p.message);
    assert_eq!(found.len(), 1, "{}", show(&found));

    // With the include: no problem.
    std::fs::write(
        dir.path().join("main.xml"),
        "<root BTCPP_format=\"4\"><include path=\"sub.xml\"/>\
         <BehaviorTree ID=\"Main\"><SubTree ID=\"Sub\"/></BehaviorTree></root>",
    )
    .unwrap();
    project.reload_file(main).unwrap();
    project.refresh();
    let found = validate_project(&project);
    assert!(found.is_empty(), "{}", show(&found));

    // main_tree_to_execute in another file.
    let file = BtFile::parse(
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Sub\"><BehaviorTree ID=\"Other\">\
         <AlwaysSuccess/></BehaviorTree></root>",
        None,
    )
    .unwrap();
    let other = project.add_bt_file(file, FileRole::Tree);
    project.refresh();
    let found = validate_file(&project, other);
    let p = single(&found, C::TreeNotIncluded);
    assert_eq!(p.attribute.as_deref(), Some("main_tree_to_execute"));
}

#[test]
fn includes() {
    let dir = write_files(&[
        (
            "main.xml",
            "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\">\
             <include path=\"lib/sub.xml\"/><include path=\"lib/none.xml\"/>\
             <BehaviorTree ID=\"Main\"><Sequence><SubTree ID=\"Sub\"/><Leaf/></Sequence></BehaviorTree></root>",
        ),
        (
            "lib/sub.xml",
            "<root BTCPP_format=\"4\"><TreeNodesModel><Action ID=\"Leaf\"/></TreeNodesModel>\
             <BehaviorTree ID=\"Sub\"><Leaf/></BehaviorTree></root>",
        ),
    ]);
    let project = Project::open_xml(dir.path().join("main.xml")).unwrap();
    let main = file_id(&project, dir.path(), "main.xml");
    let found = validate_project(&project);
    // Only the missing include; the models of the included file are known
    // in the file that includes it.
    let p = single(&found, C::UnresolvedInclude);
    assert_eq!(found.len(), 1, "{}", show(&found));
    assert_eq!(p.file, main);
    assert_eq!(p.severity, Error);
    assert_eq!(p.attribute.as_deref(), Some("path"));
    assert!(p.message.contains("none.xml"), "{}", p.message);
}

#[test]
fn include_cycle() {
    let dir = write_files(&[
        (
            "a.xml",
            "<root BTCPP_format=\"4\" main_tree_to_execute=\"A\"><include path=\"b.xml\"/>\
             <BehaviorTree ID=\"A\"><AlwaysSuccess/></BehaviorTree></root>",
        ),
        (
            "b.xml",
            "<root BTCPP_format=\"4\"><include path=\"a.xml\"/>\
             <BehaviorTree ID=\"B\"><AlwaysSuccess/></BehaviorTree></root>",
        ),
    ]);
    let project = Project::open_xml(dir.path().join("a.xml")).unwrap();
    let found = validate_project(&project);
    let cycles = with_code(&found, C::UnresolvedInclude);
    assert!(!cycles.is_empty(), "{}", show(&found));
    assert!(
        cycles
            .iter()
            .all(|p| p.severity == Error && p.message.contains("cycle"))
    );
}

#[test]
fn ros_package_include() {
    let doc = "<root BTCPP_format=\"4\"><include ros_pkg=\"no_such_package\" path=\"x.xml\"/>\
               <BehaviorTree ID=\"Main\"><AlwaysSuccess/></BehaviorTree></root>";
    let file = BtFile::parse(doc, None).unwrap();
    let mut project = Project::new("ros");
    project.set_ament_prefix_path(None);
    let id = project.add_bt_file(file, FileRole::Tree);
    project.refresh();
    let found = validate_file(&project, id);
    assert_eq!(single(&found, C::UnresolvedInclude).severity, Warning);
    let dir = tempfile::tempdir().unwrap();
    project.set_ament_prefix_path(Some(dir.path().as_os_str().to_owned()));
    project.refresh();
    let found = validate_file(&project, id);
    assert_eq!(single(&found, C::UnresolvedInclude).severity, Error);
}

#[test]
fn duplicate_tree_ids_across_files() {
    let dir = write_files(&[
        (
            "a.xml",
            "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Same\"><AlwaysSuccess/></BehaviorTree></root>",
        ),
        (
            "b.xml",
            "<root BTCPP_format=\"4\"><BehaviorTree ID=\"Same\"><AlwaysSuccess/></BehaviorTree></root>",
        ),
        (
            "c.xml",
            "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\"><include path=\"b.xml\"/>\
             <BehaviorTree ID=\"Main\"><SubTree ID=\"Same\"/></BehaviorTree></root>",
        ),
    ]);
    let mut project = Project::new("dups");
    let a = project.add_file(dir.path().join("a.xml")).unwrap();
    let found = validate_project(&project);
    assert!(found.is_empty(), "{}", show(&found));
    // Two files that do not include each other: a warning in each.
    let b = project.add_file(dir.path().join("b.xml")).unwrap();
    let found = validate_project(&project);
    let dups = with_code(&found, C::DuplicateTreeId);
    let files: Vec<(FileId, Severity)> = dups.iter().map(|p| (p.file, p.severity)).collect();
    assert_eq!(files, [(a, Warning), (b, Warning)], "{}", show(&found));
    assert!(dups[0].message.contains("b.xml"), "{}", dups[0].message);
    assert!(dups[1].message.contains("a.xml"), "{}", dups[1].message);
    // c.xml includes b.xml and has no tree "Same": b.xml still has only the
    // warning about a.xml.
    let c = project.add_file(dir.path().join("c.xml")).unwrap();
    let found = validate_file(&project, c);
    assert!(found.is_empty(), "{}", show(&found));
    // A tree in c.xml with the same ID as a tree in b.xml: an error.
    std::fs::write(
        dir.path().join("c.xml"),
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"Main\"><include path=\"b.xml\"/>\
         <BehaviorTree ID=\"Main\"><SubTree ID=\"Same\"/></BehaviorTree>\
         <BehaviorTree ID=\"Same\"><AlwaysFailure/></BehaviorTree></root>",
    )
    .unwrap();
    project.reload_file(c).unwrap();
    project.refresh();
    let found = validate_file(&project, c);
    let p = single(&found, C::DuplicateTreeId);
    assert_eq!(p.severity, Error);
    assert!(
        p.message.contains("which this file includes"),
        "{}",
        p.message
    );
    let found = validate_file(&project, b);
    let errors: Vec<&Problem> = with_code(&found, C::DuplicateTreeId)
        .into_iter()
        .filter(|p| p.is_error())
        .collect();
    assert_eq!(errors.len(), 1, "{}", show(&found));
    assert!(errors[0].message.contains("which includes this file"));
}

#[test]
fn subtree_cycle_across_files() {
    let dir = write_files(&[
        (
            "a.xml",
            "<root BTCPP_format=\"4\" main_tree_to_execute=\"A\"><include path=\"b.xml\"/>\
             <BehaviorTree ID=\"A\"><SubTree ID=\"B\"/></BehaviorTree></root>",
        ),
        (
            "b.xml",
            "<root BTCPP_format=\"4\"><BehaviorTree ID=\"B\"><SubTree ID=\"A\"/></BehaviorTree></root>",
        ),
    ]);
    let mut project = Project::new("cycle");
    project.add_file(dir.path().join("a.xml")).unwrap();
    project.add_file(dir.path().join("b.xml")).unwrap();
    let found = validate_project(&project);
    let cycles = with_code(&found, C::CircularSubTree);
    assert_eq!(cycles.len(), 2, "{}", show(&found));
    assert!(cycles[0].message.contains("A -> B -> A"));
    assert!(cycles[1].message.contains("B -> A -> B"));
}

// --------------------------------------------------------------------
// Unconnected nodes
// --------------------------------------------------------------------

/// Detaches the element at the index path of tree 0 and adds it as an
/// unconnected node of the tree.
fn detach(project: &mut Project, id: FileId, path: &[usize]) -> NodeId {
    let node = project
        .file(id)
        .unwrap()
        .node_at_index_path(0, path)
        .unwrap();
    project
        .edit_file(id, |file| {
            file.edit(|doc| {
                doc.remove_element_pretty(node).unwrap();
            })
        })
        .unwrap();
    project.add_unconnected(id, "Main", node, [10.0, 20.0]);
    node
}

#[test]
fn unconnected_nodes() {
    let (mut project, id) =
        setup("<Sequence><AlwaysSuccess/><Inverter><SayMessage bad=\"1\"/></Inverter></Sequence>");
    let found = validate_file(&project, id);
    assert_eq!(single(&found, C::UnknownPort).tree.as_deref(), Some("Main"));
    let node = detach(&mut project, id, &[0, 1]);
    let found = validate_file(&project, id);
    // The unconnected node, then the problems inside it (they are
    // checked like the nodes of the tree).
    let codes: Vec<(Severity, ProblemCode)> = found.iter().map(|p| (p.severity, p.code)).collect();
    assert_eq!(
        codes,
        [(Warning, C::UnconnectedNode), (Error, C::UnknownPort)],
        "{}",
        show(&found)
    );
    assert_eq!(found[0].node, Some(node));
    assert_eq!(found[0].tree.as_deref(), Some("Main"));
    // A node that has a parent again is part of the tree.
    project
        .edit_file(id, |file| {
            file.edit(|doc| {
                let root = file_root_sequence(doc);
                doc.insert_element_pretty(root, 1, node).unwrap();
            })
        })
        .unwrap();
    let found = validate_file(&project, id);
    let codes: Vec<ProblemCode> = found.iter().map(|p| p.code).collect();
    assert_eq!(codes, [C::UnknownPort], "{}", show(&found));
}

/// The `<Sequence>` of the tree `Main`.
fn file_root_sequence(doc: &fasttick_core::xml::Document) -> NodeId {
    let root = doc.document_element().unwrap();
    let tree = doc.element_children(root).next().unwrap();
    doc.element_children(tree).next().unwrap()
}

#[test]
fn unconnected_subtree_nodes_skip_the_cycle_check() {
    let (mut project, id) = setup("<Sequence><AlwaysSuccess/><SubTree ID=\"Main\"/></Sequence>");
    let found = validate_file(&project, id);
    assert_eq!(single(&found, C::SelfSubTree).severity, Error);
    detach(&mut project, id, &[0, 1]);
    let found = validate_file(&project, id);
    let codes: Vec<ProblemCode> = found.iter().map(|p| p.code).collect();
    assert_eq!(codes, [C::UnconnectedNode], "{}", show(&found));
}

// --------------------------------------------------------------------
// Order and API
// --------------------------------------------------------------------

#[test]
fn order_is_deterministic() {
    let doc = "<root>\n<Extra/>\n<TreeNodesModel><Widget/></TreeNodesModel>\n\
               <BehaviorTree ID=\"B\"><Sequence><Inverter/><SayMessage x=\"1\"/>\
               <SubTree ID=\"Nope\"/></Sequence></BehaviorTree>\n\
               <BehaviorTree ID=\"A\"><Sleep/></BehaviorTree>\n</root>";
    let (project, id) = setup(doc);
    let first = validate_file(&project, id);
    for _ in 0..5 {
        assert_eq!(validate_file(&project, id), first);
    }
    let codes: Vec<ProblemCode> = first.iter().map(|p| p.code).collect();
    assert_eq!(
        codes,
        [
            // The file: the format, the models, the other elements, the
            // main tree.
            C::FormatVersion,
            C::BadModel,
            C::RootElement,
            C::AmbiguousMainTree,
            // Tree B in depth-first order, then tree A.
            C::ChildCount,
            C::UnknownPort,
            C::MissingSubTree,
            C::MissingRequiredPort,
        ],
        "{}",
        show(&first)
    );
    let trees: Vec<Option<&str>> = first.iter().map(|p| p.tree.as_deref()).collect();
    assert_eq!(
        trees,
        [
            None,
            None,
            None,
            None,
            Some("B"),
            Some("B"),
            Some("B"),
            Some("A")
        ]
    );
}

#[test]
fn project_and_file_results_agree() {
    let dir = write_files(&[
        (
            "a.xml",
            "<root><BehaviorTree ID=\"A\"><Inverter/></BehaviorTree></root>",
        ),
        (
            "b.xml",
            "<root BTCPP_format=\"4\"><BehaviorTree ID=\"B\"><SubTree ID=\"A\"/></BehaviorTree></root>",
        ),
    ]);
    let mut project = Project::new("two");
    let a = project.add_file(dir.path().join("a.xml")).unwrap();
    let b = project.add_file(dir.path().join("b.xml")).unwrap();
    let all = validate_project(&project);
    let validator = Validator::new(&project);
    assert_eq!(validator.run(), all);
    let mut joined = validate_file(&project, a);
    joined.extend(validate_file(&project, b));
    assert_eq!(joined, all);
    assert!(all.iter().any(|p| p.file == a) && all.iter().any(|p| p.file == b));
    // A file that is not in the project gives no problems.
    assert!(validator.run_file(FileId(99)).is_empty());
}

#[test]
fn messages_have_one_line() {
    // Control characters of quoted values are written as escapes.
    let found = problems("<SayMessage message=\"{a&#10;b\"/>");
    let p = single(&found, C::BadBlackboardRef);
    assert!(p.message.contains("\"{a\\nb\""), "{}", p.message);
    let found = problems("<AlwaysSuccess name=\"a&#1;b\"/>");
    assert!(
        found
            .iter()
            .all(|p| !p.message.chars().any(char::is_control))
    );
}
