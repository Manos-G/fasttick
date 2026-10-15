//! Live validation of BT.CPP documents and projects.
//!
//! The validator finds problems in a [`Project`] and reports each one as a
//! [`Problem`]: a [`Severity`], a [`ProblemCode`], the file, the tree, the
//! node, the attribute and a message in simple English. The message tells
//! what is wrong and how to correct it. See `docs/PLAN.md` section 4 and
//! `docs/DESIGN.md` section 2.6.
//!
//! The rules follow the BT.CPP 4.9.0 source. The main functions are in
//! `src/xml_parsing.cpp`:
//!
//! - `loadDocImpl`: the root element, `BTCPP_format` and `<include>`;
//! - `VerifyXML`: the structure rules (tree IDs, one root node, child
//!   counts, known node types, nesting depth);
//! - `createNodeFromXML`: the node form, the instance name, the ports and
//!   the conversion of literal port values;
//! - `recursivelyCreateSubtree`: SubTree remapping, `_autoremap`, the
//!   mandatory ports of a SubTree model, SubTree paths and cycles;
//! - `XMLParser::instantiateTree` and `detectMainTreeId`
//!   (`src/bt_factory.cpp`): the tree to execute.
//!
//! The documentation of each [`ProblemCode`] names its BT.CPP source.
//!
//! # Severity
//!
//! - [`Severity::Error`]: BT.CPP does not load the file or does not create
//!   the tree, a node fails each time that it runs, or BT.CPP ignores a
//!   part of the file without a message (for example a second tree with the
//!   same ID).
//! - [`Severity::Warning`]: BT.CPP loads the tree, but the tree probably
//!   does not do what the author wants.
//! - [`Severity::Info`]: a note. The tree can be correct.
//!
//! # Models
//!
//! The validator gets the node models from [`Project::registry`], in the
//! look-up order of `docs/DESIGN.md` section 2.4. A node type is known only
//! if a model declares it (a `<TreeNodesModel>` of the file or of an
//! included file, a project model file, or the built-in models). A SubTree
//! model that is made from a `<BehaviorTree>` does not make the compact form
//! `<TreeID/>` valid: BT.CPP accepts only `<SubTree ID="TreeID"/>`.
//!
//! # Order
//!
//! The result is deterministic. The files come in project order. In a file,
//! the problems of the file come first (root element, format, includes,
//! models, main tree), then the problems of each tree in document order
//! (the tree itself, then its nodes in depth-first order), then the problems
//! of the unconnected nodes. [`validate_project`] gives the same problems as
//! [`validate_file`] for each file, in project order.
//!
//! # Example
//!
//! ```
//! use fasttick_core::bt::BtFile;
//! use fasttick_core::project::Project;
//! use fasttick_core::validate::{ProblemCode, Severity, validate_project};
//!
//! let file = BtFile::parse(
//!     r#"<root BTCPP_format="4">
//!   <BehaviorTree ID="Main">
//!     <Inverter/>
//!   </BehaviorTree>
//! </root>"#,
//!     None,
//! )
//! .unwrap();
//! let (project, _) = Project::with_file("demo", file);
//! let problems = validate_project(&project);
//! assert_eq!(problems.len(), 1);
//! assert_eq!(problems[0].code, ProblemCode::ChildCount);
//! assert_eq!(problems[0].severity, Severity::Error);
//! assert_eq!(problems[0].tree.as_deref(), Some("Main"));
//! ```

mod check;
mod context;
mod value;

use std::fmt;

use crate::project::{FileId, Project};
use crate::xml::NodeId;

use check::FileChecker;
use context::Context;

/// How serious a problem is. `Error` sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    /// BT.CPP rejects the file or the tree, the node fails each time that it
    /// runs, or BT.CPP ignores a part of the file without a message.
    Error,
    /// BT.CPP accepts the tree, but it is probably not correct.
    Warning,
    /// A note. The tree can be correct.
    Info,
}

impl Severity {
    /// All severities, the most serious first.
    pub const ALL: [Severity; 3] = [Severity::Error, Severity::Warning, Severity::Info];

    /// The name in lowercase: `error`, `warning` or `info`.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

macro_rules! problem_codes {
    ($( $(#[$doc:meta])* $name:ident => $summary:literal, )*) => {
        /// The kind of a problem. The documentation of each code names the
        /// BT.CPP rule that it copies.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum ProblemCode {
            $( $(#[$doc])* $name, )*
        }

        impl ProblemCode {
            /// All codes, in the order of the declaration.
            pub const ALL: &'static [ProblemCode] = &[$(ProblemCode::$name,)*];

            /// The name of the code, for example `UnknownNodeType`.
            pub fn name(self) -> &'static str {
                match self {
                    $(ProblemCode::$name => stringify!($name),)*
                }
            }

            /// A short description of the rule, for a help text.
            pub fn summary(self) -> &'static str {
                match self {
                    $(ProblemCode::$name => $summary,)*
                }
            }

            /// The code with this name (see [`ProblemCode::name`]).
            pub fn from_name(name: &str) -> Option<ProblemCode> {
                match name {
                    $(stringify!($name) => Some(ProblemCode::$name),)*
                    _ => None,
                }
            }
        }
    };
}

problem_codes! {
    /// The document has no root element, the root element is not `<root>`
    /// (Error; `VerifyXML`: "The XML must have a root node called
    /// \<root\>"), or `<root>` has a child element that BT.CPP ignores
    /// (Warning).
    RootElement => "The root element is not correct.",
    /// `BTCPP_format` is missing or is not `"4"` (Warning; `loadDocImpl`
    /// prints a warning when it is missing).
    FormatVersion => "The file does not declare BTCPP_format=\"4\".",
    /// An `<include>` gives no file: no `path`, a ROS package that is not
    /// found, a file that cannot be loaded, or an include cycle
    /// (`loadDocImpl`).
    UnresolvedInclude => "An included file cannot be loaded.",
    /// A problem in a `<TreeNodesModel>`: more than one section
    /// (`VerifyXML`), a SubTree model without `ID` or a port without `name`
    /// (`loadSubtreeModel`, `parseSubtreeModelPorts`), or a model element
    /// that FastTick cannot read.
    BadModel => "A node model is not correct.",
    /// An element has no `ID` where BT.CPP requires it: `<Action>`,
    /// `<Condition>`, `<Control>`, `<Decorator>`, `<SubTree>`, and
    /// `<BehaviorTree>` when `<root>` has more than one child element
    /// (`VerifyXML`).
    MissingId => "An element has no ID attribute.",
    /// A name breaks the BT.CPP name rules: `validateModelName`,
    /// `validatePortName`, `validateInstanceName` (`src/xml_parsing.cpp`)
    /// and `IsAllowedPortName` (`src/basic_types.cpp`).
    InvalidName => "A name breaks the BT.CPP name rules.",
    /// `Root` or `root` is used as a node type or tree ID
    /// (`validateModelName`: "'Root' is a reserved name").
    ReservedNodeName => "\"Root\" is a reserved name.",
    /// A `<BehaviorTree>` or `<SubTree>` ID is the ID of a registered node
    /// type (`VerifyXML`: "must not use the name of a registered Node").
    TreeIdConflict => "A tree ID is the name of a node type.",
    /// Two trees have the same ID. BT.CPP keeps only the last one
    /// (`loadDocImpl`: `tree_roots[tree_name] = bt_node`).
    DuplicateTreeId => "Two trees have the same ID.",
    /// A `<BehaviorTree>` has no root node (`VerifyXML`: "must have exactly
    /// 1 child").
    EmptyTree => "A tree has no root node.",
    /// A `<BehaviorTree>` has more than one root node (`VerifyXML`: "must
    /// have exactly 1 child").
    MultipleRoots => "A tree has more than one root node.",
    /// A node is more than 256 levels below its `<BehaviorTree>`
    /// (`VerifyXML`: `kMaxNestingDepth`).
    NestingTooDeep => "The tree is nested too deeply.",
    /// `main_tree_to_execute` names a tree that does not exist
    /// (`recursivelyCreateSubtree`: "Can't find a tree with name").
    MissingMainTree => "The main tree does not exist.",
    /// The file has more than one tree and no `main_tree_to_execute`
    /// (`XMLParser::instantiateTree`: "\[main_tree_to_execute\] was not
    /// specified correctly") (Warning).
    AmbiguousMainTree => "The tree to execute is not specified.",
    /// No model declares this node type (`VerifyXML`: "Node not
    /// recognized").
    UnknownNodeType => "The node type is not known.",
    /// A node in the compact form has an `ID` attribute
    /// (`createNodeFromXML`: "Attribute \[ID\] is not allowed").
    UnexpectedId => "The ID attribute is not allowed here.",
    /// The tag of the explicit form (for example `<Condition ID="X">`) does
    /// not match the kind of `X`. BT.CPP uses the registered kind and
    /// ignores the tag (Warning).
    KindMismatch => "The tag does not match the kind of the node type.",
    /// A node has a wrong number of children (`VerifyXML`; the `tick`
    /// functions of `IfThenElse`, `WhileDoElse` and `SwitchN`).
    ChildCount => "A node has a wrong number of children.",
    /// A child of a `ReactiveSequence` that `VerifyXML` rejects: the
    /// explicit form ("Unknown node type") or a second `AsyncSequence`
    /// ("cannot have more than one async child").
    ReactiveSequenceChild => "A ReactiveSequence has a child that BT.CPP rejects.",
    /// An attribute is not a port of the node type (Error;
    /// `createNodeFromXML`), or a SubTree remaps a port that the SubTree
    /// model does not declare (Warning).
    UnknownPort => "The attribute is not a port of the node type.",
    /// A port has no value and no default value. For a normal node, BT.CPP
    /// fails only when the node reads the port (Warning). For a port of a
    /// SubTree model, `recursivelyCreateSubtree` stops (Error).
    MissingRequiredPort => "A required port has no value.",
    /// A literal port value cannot be converted to the port type
    /// (`createNodeFromXML`: "can not be converted to").
    InvalidPortValue => "A port value cannot be converted to the port type.",
    /// An output port has a literal value instead of a blackboard
    /// reference (Warning). BT.CPP creates the tree, but
    /// `TreeNode::setOutput` (`include/behaviortree_cpp/tree_node.h`) fails
    /// with "setOutput requires a blackboard pointer". `"{=}"` and `"="`
    /// are correct (the port name is the key). For a SubTree, the value
    /// goes only into the blackboard of the SubTree.
    ///
    /// `createNodeFromXML` converts each literal value of a strongly typed
    /// port, also of an output port. A value that does not convert is an
    /// [`ProblemCode::InvalidPortValue`] error. This includes `"="` for a
    /// strongly typed port that is not a string: only `"{=}"` is a
    /// reference there.
    OutputPortLiteral => "An output port has a fixed value.",
    /// A value looks like a blackboard reference but is not a correct one:
    /// unbalanced braces, an empty key, braces or spaces in the key
    /// (`TreeNode::isBlackboardPointer`) (Warning).
    BadBlackboardRef => "A blackboard reference is not correct.",
    /// A pre-condition or post-condition script is empty
    /// (`BehaviorTreeFactory::instantiateTreeNode`: "Empty Script").
    EmptyCondition => "A condition script is empty.",
    /// A raw attribute value contains an `&` that does not start a valid
    /// reference. tinyxml2 can change the value (Warning).
    SuspiciousAmpersand => "An attribute value contains a bare &.",
    /// A `<SubTree>` refers to a tree that does not exist
    /// (`recursivelyCreateSubtree`: "Can't find a tree with name").
    MissingSubTree => "The SubTree refers to a tree that does not exist.",
    /// A tree is found only in a project file that this file does not
    /// include. BT.CPP finds it only if the executor loads that file too
    /// (Info).
    TreeNotIncluded => "The tree is in a file that this file does not include.",
    /// A tree uses itself as a SubTree (`recursivelyCreateSubtree`:
    /// "Recursive behavior tree cycle detected").
    SelfSubTree => "A tree uses itself as a SubTree.",
    /// Trees use each other in a cycle (`recursivelyCreateSubtree`:
    /// "Recursive behavior tree cycle detected").
    CircularSubTree => "SubTrees make a cycle.",
    /// A SubTree remapping is not correct: `_autoremap` is not a boolean
    /// (Error; `convertFromString<bool>`), `"="` instead of `"{=}"`, or
    /// `_autoremap` on a node that is not a SubTree, or the BT.CPP 3
    /// attribute `__shared_blackboard` (Warning). A remapping
    /// of a port that the SubTree model does not declare is an
    /// [`ProblemCode::UnknownPort`] warning.
    BadRemapping => "A SubTree remapping is not correct.",
    /// Two SubTree nodes in one tree have the same `name`, or an empty
    /// `name` gives the path of the root tree (`recursivelyCreateSubtree`:
    /// "Duplicate SubTree path detected").
    DuplicateSubTreeName => "Two SubTree nodes have the same path.",
    /// A node has no parent. FastTick keeps it in the project file but does
    /// not write it to the XML (decision D5) (Warning).
    UnconnectedNode => "A node has no parent.",
}

impl fmt::Display for ProblemCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One problem that the validator found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// How serious the problem is.
    pub severity: Severity,
    /// The rule.
    pub code: ProblemCode,
    /// The file with the problem.
    pub file: FileId,
    /// The ID of the tree with the problem. For an unconnected node, this
    /// is the tree of the node in the project file.
    pub tree: Option<String>,
    /// The element with the problem: a tree node, a `<BehaviorTree>`, an
    /// `<include>`, a model element or the root element.
    pub node: Option<NodeId>,
    /// The name of the attribute with the problem.
    pub attribute: Option<String>,
    /// What is wrong and how to correct it, in simple English.
    pub message: String,
}

impl Problem {
    /// True for [`Severity::Error`].
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// The line and the column (both from 1) of the element in the source
    /// text of its file. `None` if the problem has no element, or if the
    /// element was added after the file was loaded.
    pub fn line_col(&self, project: &Project) -> Option<(usize, usize)> {
        let node = self.node?;
        project.file(self.file)?.doc().node_line_col(node)
    }
}

impl fmt::Display for Problem {
    /// Writes `severity[Code]: message`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]: {}", self.severity, self.code, self.message)
    }
}

/// The number of problems of each severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProblemCounts {
    /// The number of errors.
    pub errors: usize,
    /// The number of warnings.
    pub warnings: usize,
    /// The number of notes.
    pub infos: usize,
}

impl ProblemCounts {
    /// Counts the problems of a list.
    pub fn of(problems: &[Problem]) -> ProblemCounts {
        let mut counts = ProblemCounts::default();
        for p in problems {
            match p.severity {
                Severity::Error => counts.errors += 1,
                Severity::Warning => counts.warnings += 1,
                Severity::Info => counts.infos += 1,
            }
        }
        counts
    }

    /// The total number of problems.
    pub fn total(&self) -> usize {
        self.errors + self.warnings + self.infos
    }
}

/// Validates a project.
///
/// The validator reads the project once when it is made (tree IDs,
/// include closures, SubTree cycles). Make a new validator after the
/// project changes.
#[derive(Debug)]
pub struct Validator<'p> {
    ctx: Context<'p>,
}

impl<'p> Validator<'p> {
    /// Prepares the validation of a project.
    pub fn new(project: &'p Project) -> Validator<'p> {
        Validator {
            ctx: Context::new(project),
        }
    }

    /// Validates all files, in project order.
    pub fn run(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        for &file in self.ctx.files() {
            if let Some(checker) = FileChecker::new(&self.ctx, file, &mut out) {
                checker.run();
            }
        }
        out
    }

    /// Validates one file. The result is empty if the file is not in the
    /// project.
    pub fn run_file(&self, file: FileId) -> Vec<Problem> {
        let mut out = Vec::new();
        if let Some(checker) = FileChecker::new(&self.ctx, file, &mut out) {
            checker.run();
        }
        out
    }
}

/// Validates all files of a project. See the [module documentation](self).
pub fn validate_project(project: &Project) -> Vec<Problem> {
    Validator::new(project).run()
}

/// Validates one file of a project. The rules that look at more than one
/// file (tree IDs, SubTrees, includes) also read the other files. The
/// result is empty if the file is not in the project.
pub fn validate_file(project: &Project, file: FileId) -> Vec<Problem> {
    Validator::new(project).run_file(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_names_round_trip() {
        for &code in ProblemCode::ALL {
            assert_eq!(ProblemCode::from_name(code.name()), Some(code));
            assert_eq!(code.to_string(), code.name());
            assert!(code.summary().ends_with('.'), "{code}");
        }
        assert_eq!(ProblemCode::from_name("Nope"), None);
        assert_eq!(ProblemCode::ALL.len(), 33);
    }

    #[test]
    fn severity_order_and_names() {
        assert!(Severity::Error < Severity::Warning);
        assert!(Severity::Warning < Severity::Info);
        let names: Vec<&str> = Severity::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(names, ["error", "warning", "info"]);
    }

    #[test]
    fn counts_and_display() {
        let p = Problem {
            severity: Severity::Warning,
            code: ProblemCode::FormatVersion,
            file: FileId(0),
            tree: None,
            node: None,
            attribute: None,
            message: "Text.".to_owned(),
        };
        assert_eq!(p.to_string(), "warning[FormatVersion]: Text.");
        assert!(!p.is_error());
        let mut e = p.clone();
        e.severity = Severity::Error;
        let counts = ProblemCounts::of(&[p.clone(), e, p]);
        assert_eq!(
            counts,
            ProblemCounts {
                errors: 1,
                warnings: 2,
                infos: 0
            }
        );
        assert_eq!(counts.total(), 3);
    }
}
