//! The checks of one file: the root element, includes, models, the main
//! tree, the trees, the nodes and the unconnected nodes.

use std::collections::HashMap;

use crate::bt::names::{
    NameError, is_allowed_port_name, validate_instance_name, validate_model_name,
    validate_port_name,
};
use crate::bt::{
    AttributeClass, BtFile, MAX_NESTING_DEPTH, NodeView, PortValue, TreeEntry, blackboard_pointer,
};
use crate::builtin::{self, ChildRule};
use crate::models::{ModelIssueKind, NodeKind, NodeModel, PortDirection, PortModel};
use crate::project::{FileId, FileRole, IncludeProblem};
use crate::xml::{Attribute, Document, NodeId};

use super::context::{Context, Cycle, Resolved};
use super::value::{
    RefProblem, bad_ampersand, check_blackboard_ref, decode_strict, is_blackboard_for_conversion,
    literal_converts,
};
use super::{Problem, ProblemCode, Severity};

/// The registered node types that `VerifyXML` counts as asynchronous
/// children of a `ReactiveSequence` (only `AsyncSequence` is built in).
const ASYNC_CHILD_TYPES: [&str; 4] = [
    "ThreadedAction",
    "StatefulActionNode",
    "CoroActionNode",
    "AsyncSequence",
];

/// The tree that BT.CPP selects when it loads the file and no tree ID is
/// given (`detectMainTreeId` and `loadXmlAndResolveTreeId` in
/// `src/bt_factory.cpp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainTree<'a> {
    /// `main_tree_to_execute` names this tree.
    Named(&'a str),
    /// The file has one tree with an ID, or the file and its includes
    /// register only one tree.
    Single(&'a str),
    /// More than one tree, and no `main_tree_to_execute`.
    Ambiguous,
    /// No trees.
    None,
}

/// The result of the check of the root element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootState {
    /// The document has no root element.
    Missing,
    /// The root element is not the one that BT.CPP expects.
    Wrong,
    /// The root element is correct.
    Correct,
}

/// The state of the walk through one tree (or one unconnected node).
struct TreeState<'a> {
    /// The tree ID.
    name: &'a str,
    /// True for a tree of the file (not an unconnected node): the nesting
    /// depth and the SubTree cycles are checked.
    attached: bool,
    /// True if the tree is the main tree of the file.
    is_main: bool,
    /// The first SubTree node with each `name`.
    subtree_names: HashMap<&'a str, NodeId>,
    /// True after the first `NestingTooDeep` problem of the tree.
    too_deep_reported: bool,
}

/// The kind of a node and the model that BT.CPP uses for it.
enum NodeType<'a> {
    /// A `<SubTree>` node. The model is the declared SubTree model of the
    /// target tree, if there is one.
    SubTree(Option<&'a NodeModel>),
    /// A node of a declared type.
    Known(&'a NodeModel),
    /// The type is not known (or the node has no type ID).
    Unknown,
}

/// Checks one file and adds the problems to a list.
pub(super) struct FileChecker<'c, 'p> {
    ctx: &'c Context<'p>,
    file: FileId,
    bt: &'p BtFile,
    doc: &'p Document,
    out: &'c mut Vec<Problem>,
}

impl<'c, 'p> FileChecker<'c, 'p> {
    /// Makes a checker for a file of the project. Returns `None` if the
    /// file is not in the project.
    pub(super) fn new(
        ctx: &'c Context<'p>,
        file: FileId,
        out: &'c mut Vec<Problem>,
    ) -> Option<Self> {
        let bt = ctx.project().file(file)?;
        Some(FileChecker {
            ctx,
            file,
            bt,
            doc: bt.doc(),
            out,
        })
    }

    fn push(
        &mut self,
        severity: Severity,
        code: ProblemCode,
        tree: Option<&str>,
        node: Option<NodeId>,
        attribute: Option<&str>,
        message: String,
    ) {
        self.out.push(Problem {
            severity,
            code,
            file: self.file,
            tree: tree.map(str::to_owned),
            node,
            attribute: attribute.map(str::to_owned),
            message: one_line(message),
        });
    }

    fn is_models_file(&self) -> bool {
        self.ctx.project().role(self.file) == Some(FileRole::Models)
    }

    pub(super) fn run(mut self) {
        let is_models_file = self.is_models_file();
        let root = self.check_root(is_models_file);
        if root == RootState::Missing {
            self.check_unconnected();
            return;
        }
        self.check_includes();
        self.check_models();
        // The children of a wrong root element (for example of a ROS
        // package.xml) are not BT.CPP elements: one problem is sufficient.
        if root == RootState::Correct {
            self.check_other_elements();
        }
        let main = self.main_tree();
        if !is_models_file {
            self.check_main_tree(main);
        }
        let main_name = match main {
            MainTree::Named(id) | MainTree::Single(id) => Some(id),
            MainTree::Ambiguous | MainTree::None => None,
        };
        for tree in self.bt.trees() {
            self.check_tree(tree, main_name == Some(tree.id.as_str()));
        }
        self.check_unconnected();
    }

    // ----------------------------------------------------------------
    // File level
    // ----------------------------------------------------------------

    /// Checks the root element and `BTCPP_format`.
    fn check_root(&mut self, is_models_file: bool) -> RootState {
        let Some(root) = self.bt.index().root else {
            self.push(
                Severity::Error,
                ProblemCode::RootElement,
                None,
                None,
                None,
                "The file has no root element. BT.CPP cannot load it. \
                 Add a <root BTCPP_format=\"4\"> element."
                    .to_owned(),
            );
            return RootState::Missing;
        };
        self.check_amp_attributes(root, None);
        let name = self.doc.name(root).unwrap_or_default();
        if is_models_file {
            let state = if name == "root" || name == "TreeNodesModel" {
                RootState::Correct
            } else {
                RootState::Wrong
            };
            if state == RootState::Wrong {
                self.push(
                    Severity::Error,
                    ProblemCode::RootElement,
                    None,
                    Some(root),
                    None,
                    format!(
                        "The root element of the model file is <{name}>. A model file must \
                         have <root> or <TreeNodesModel> as its root element."
                    ),
                );
            } else if self.bt.index().model_sections.is_empty() {
                self.push(
                    Severity::Warning,
                    ProblemCode::RootElement,
                    None,
                    Some(root),
                    None,
                    "The model file has no <TreeNodesModel> element, so it gives no node \
                     models. Add the models in a <TreeNodesModel> element."
                        .to_owned(),
                );
            }
            return state;
        }
        if name != "root" {
            // VerifyXML: "The XML must have a root node called <root>".
            self.push(
                Severity::Error,
                ProblemCode::RootElement,
                None,
                Some(root),
                None,
                format!(
                    "The root element is <{name}>. BT.CPP accepts only <root> as the root \
                     element. Rename the element to <root>."
                ),
            );
            return RootState::Wrong;
        }
        // loadDocImpl prints a warning when BTCPP_format is missing.
        match self.bt.index().format.as_deref() {
            None => self.push(
                Severity::Warning,
                ProblemCode::FormatVersion,
                None,
                Some(root),
                Some("BTCPP_format"),
                "The <root> element has no BTCPP_format attribute. BT.CPP 4 prints a \
                 warning. Add BTCPP_format=\"4\" to <root>."
                    .to_owned(),
            ),
            Some("4") => {}
            Some(other) => self.push(
                Severity::Warning,
                ProblemCode::FormatVersion,
                None,
                Some(root),
                Some("BTCPP_format"),
                format!(
                    "BTCPP_format is \"{other}\". FastTick and BT.CPP 4 use the format \"4\". \
                     If the file uses an older format, convert it, then set \
                     BTCPP_format=\"4\"."
                ),
            ),
        }
        RootState::Correct
    }

    fn check_includes(&mut self) {
        let links = self.ctx.project().includes(self.file);
        for link in links {
            self.check_amp_tree(link.element, None);
            let Some(problem) = &link.problem else {
                continue;
            };
            let (severity, message) = match problem {
                IncludeProblem::MissingPath => (
                    Severity::Error,
                    "The <include> element has no path attribute. BT.CPP stops with \
                     \"missing 'path' attribute\". Add the path of the file to include."
                        .to_owned(),
                ),
                IncludeProblem::RosPackageNotFound {
                    package,
                    no_ament_prefix_path: true,
                } => (
                    Severity::Warning,
                    format!(
                        "FastTick cannot find the ROS package \"{package}\" because \
                         AMENT_PREFIX_PATH is not set. Start FastTick in a shell where the \
                         ROS workspace is sourced."
                    ),
                ),
                IncludeProblem::RosPackageNotFound { package, .. } => (
                    Severity::Error,
                    format!(
                        "The ROS package \"{package}\" is not in AMENT_PREFIX_PATH. BT.CPP \
                         cannot find the included file. Correct the package name or build \
                         and source the package."
                    ),
                ),
                IncludeProblem::Load { path, message } => (
                    Severity::Error,
                    format!(
                        "The included file {} cannot be loaded: {message}. BT.CPP stops \
                         when it loads this file. Correct the path or the included file.",
                        path.display()
                    ),
                ),
                IncludeProblem::Cycle { path } => (
                    Severity::Error,
                    format!(
                        "The include of {} makes a cycle: that file includes this file again. \
                         BT.CPP loads the files again and again without end. Remove one of \
                         the <include> elements.",
                        path.display()
                    ),
                ),
            };
            self.push(
                severity,
                ProblemCode::UnresolvedInclude,
                None,
                Some(link.element),
                Some("path"),
                message,
            );
        }
    }

    fn check_models(&mut self) {
        let index = self.bt.index();
        let is_root_section = index
            .root
            .is_some_and(|r| index.model_sections.first() == Some(&r));
        for &section in &index.model_sections {
            self.check_amp_tree(section, None);
        }
        // VerifyXML: "Only a single node <TreeNodesModel> is supported".
        if !is_root_section {
            for &section in index.model_sections.iter().skip(1) {
                self.push(
                    Severity::Error,
                    ProblemCode::BadModel,
                    None,
                    Some(section),
                    None,
                    "BT.CPP accepts only one <TreeNodesModel> element in a file. Move these \
                     models into the first <TreeNodesModel> element."
                        .to_owned(),
                );
            }
        }
        for issue in &index.models.issues {
            let in_subtree_model = |element: NodeId| {
                self.doc.name(element) == Some("SubTree")
                    || self.doc.parent(element).and_then(|p| self.doc.name(p)) == Some("SubTree")
            };
            let (severity, message) = match &issue.kind {
                ModelIssueKind::UnknownModelTag(tag) => (
                    Severity::Warning,
                    format!(
                        "<{tag}> is not a node model, so FastTick and BT.CPP ignore it. Use \
                         <Action>, <Condition>, <Control>, <Decorator> or <SubTree>."
                    ),
                ),
                ModelIssueKind::MissingId if in_subtree_model(issue.element) => (
                    Severity::Error,
                    "The SubTree model has no ID attribute. BT.CPP stops with \"Missing \
                     attribute 'ID' in SubTree element within TreeNodesModel\". Add the ID \
                     of the tree."
                        .to_owned(),
                ),
                ModelIssueKind::MissingId => (
                    Severity::Warning,
                    "The node model has no ID attribute, so FastTick ignores it. Add the ID \
                     of the node type."
                        .to_owned(),
                ),
                ModelIssueKind::MissingPortName if in_subtree_model(issue.element) => (
                    Severity::Error,
                    "The port of the SubTree model has no name attribute. BT.CPP stops with \
                     \"Missing attribute [name] in port\". Add the port name."
                        .to_owned(),
                ),
                ModelIssueKind::MissingPortName => (
                    Severity::Warning,
                    "The port has no name attribute, so FastTick ignores it. Add the port \
                     name."
                        .to_owned(),
                ),
                ModelIssueKind::DuplicatePort(name) => (
                    Severity::Warning,
                    format!(
                        "The port \"{name}\" is defined more than once in this model. FastTick \
                         uses the first definition. Remove the other definitions."
                    ),
                ),
                ModelIssueKind::DuplicateModel(id) => (
                    Severity::Warning,
                    format!(
                        "The node model \"{id}\" is defined more than once in this file. \
                         FastTick uses the first definition. Remove the other definitions."
                    ),
                ),
            };
            self.push(
                severity,
                ProblemCode::BadModel,
                None,
                Some(issue.element),
                None,
                message,
            );
        }
        for parsed in &index.models.models {
            self.check_model_names(parsed.element, &parsed.model);
        }
    }

    fn check_model_names(&mut self, element: NodeId, model: &NodeModel) {
        // A model with a built-in ID describes the built-in node (the output
        // of writeTreeNodesModelXML has them). The built-in "SubTree"
        // manifest has the port "_autoremap"; it is not the model of a tree.
        if builtin::is_builtin(&model.id) {
            return;
        }
        let is_subtree = model.kind == NodeKind::SubTree;
        if !is_subtree {
            // BT.CPP does not read these models, but validateModelName
            // rejects the name in the compact form <Name/>.
            match validate_model_name(&model.id) {
                Ok(()) => {}
                Err(NameError::ReservedRoot) => self.push(
                    Severity::Warning,
                    ProblemCode::ReservedNodeName,
                    None,
                    Some(element),
                    Some("ID"),
                    format!(
                        "\"{}\" is a reserved name. BT.CPP rejects a node <{}/> in a tree. \
                         Rename the node type.",
                        model.id, model.id
                    ),
                ),
                Err(error) => self.push(
                    Severity::Warning,
                    ProblemCode::InvalidName,
                    None,
                    Some(element),
                    Some("ID"),
                    format!(
                        "The node type name \"{}\" is not valid: {error}. BT.CPP rejects it in \
                         the compact form. Rename the node type.",
                        model.id
                    ),
                ),
            }
        }
        for port in &model.ports {
            if is_subtree {
                // parseSubtreeModelPorts calls validatePortName when BT.CPP
                // loads the file. BT.CPP does not load a project model file
                // unless the executor registers it.
                if let Err(error) = validate_port_name(&port.name) {
                    let (severity, when) = if self.is_models_file() {
                        (Severity::Warning, "BT.CPP stops if it loads this file")
                    } else {
                        (Severity::Error, "BT.CPP stops when it loads the file")
                    };
                    self.push(
                        severity,
                        ProblemCode::InvalidName,
                        None,
                        Some(element),
                        None,
                        format!(
                            "The port name \"{}\" of the SubTree model \"{}\" is not valid: \
                             {error}. {when}. Rename the port.",
                            port.name, model.id
                        ),
                    );
                    continue;
                }
            }
            if !is_allowed_port_name(&port.name) {
                // CreatePort rejects such a name; createNodeFromXML and
                // recursivelyCreateSubtree do not read such an attribute.
                self.push(
                    Severity::Warning,
                    ProblemCode::InvalidName,
                    None,
                    Some(element),
                    None,
                    format!(
                        "The port name \"{}\" of the model \"{}\" is not a valid port name in \
                         BT.CPP. A port name starts with a letter, has no spaces or special \
                         characters, and is not name or ID. The XML cannot set this port. \
                         Rename the port.",
                        port.name, model.id
                    ),
                );
            }
        }
    }

    fn check_other_elements(&mut self) {
        let index = self.bt.index();
        let has_models = !index.model_sections.is_empty();
        for &element in &index.other_elements {
            self.check_amp_tree(element, None);
            let name = self.doc.name(element).unwrap_or_default();
            let is_kind_tag = NodeKind::from_tag(name).is_some();
            if has_models && is_kind_tag && self.doc.attribute(element, "ID").is_none() {
                // VerifyXML checks the ID of these root children when the
                // file has a <TreeNodesModel>.
                self.push(
                    Severity::Error,
                    ProblemCode::MissingId,
                    None,
                    Some(element),
                    None,
                    format!(
                        "The element <{name}> in <root> has no ID attribute. BT.CPP rejects \
                         it. Move the element into the <TreeNodesModel> and add an ID, or \
                         remove it."
                    ),
                );
            } else {
                self.push(
                    Severity::Warning,
                    ProblemCode::RootElement,
                    None,
                    Some(element),
                    None,
                    format!(
                        "BT.CPP ignores the element <{name}> in <root>. Use <BehaviorTree>, \
                         <include> or <TreeNodesModel>, or remove the element."
                    ),
                );
            }
        }
    }

    /// The main tree, as BT.CPP selects it when the file is loaded alone.
    fn main_tree(&self) -> MainTree<'p> {
        let index = self.bt.index();
        if let Some(id) = &index.main_tree_to_execute {
            return MainTree::Named(id);
        }
        // detectMainTreeId: one <BehaviorTree> with an ID.
        if let [tree] = index.trees.as_slice()
            && tree.has_explicit_id
            && !tree.id.is_empty()
        {
            return MainTree::Single(&tree.id);
        }
        // loadXmlAndResolveTreeId: the only tree that the load registers.
        let ids = self.ctx.closure_tree_ids(self.file);
        match ids.len() {
            0 => MainTree::None,
            1 => ids
                .into_iter()
                .next()
                .map_or(MainTree::None, MainTree::Single),
            _ => MainTree::Ambiguous,
        }
    }

    fn check_main_tree(&mut self, main: MainTree<'p>) {
        let root = self.bt.index().root;
        match main {
            MainTree::Named(id) => match self.ctx.resolve(self.file, id) {
                Resolved::InClosure(_) => {}
                Resolved::Elsewhere(tree) => {
                    let other = self.file_name(tree.file);
                    self.push(
                        Severity::Info,
                        ProblemCode::TreeNotIncluded,
                        None,
                        root,
                        Some("main_tree_to_execute"),
                        format!(
                            "The main tree \"{id}\" is in {other}, which this file does not \
                             include. BT.CPP finds it only if the executor also loads {other}. \
                             To make the file complete, add <include path=\"...\"/> for \
                             {other}."
                        ),
                    );
                }
                Resolved::Missing => self.push(
                    Severity::Error,
                    ProblemCode::MissingMainTree,
                    None,
                    root,
                    Some("main_tree_to_execute"),
                    format!(
                        "The attribute main_tree_to_execute names the tree \"{id}\", but no tree \
                         has this ID. BT.CPP cannot create the main tree. Correct the name or add \
                         the tree."
                    ),
                ),
            },
            MainTree::Ambiguous => {
                // Only for a file that an executor loads directly: a listed
                // tree file that no other file includes.
                if !self.ctx.is_listed_tree_file(self.file) || self.ctx.is_included(self.file) {
                    return;
                }
                let (severity, message) = if self.is_library() {
                    (
                        Severity::Info,
                        "The file has more than one tree and no main_tree_to_execute. This is \
                         correct for a library, because other files use its trees as SubTrees. \
                         To load the file as the main file, add \
                         main_tree_to_execute=\"<tree ID>\" to <root>, or select the tree in \
                         the executor.",
                    )
                } else {
                    (
                        Severity::Warning,
                        "The file has more than one tree and no main_tree_to_execute. BT.CPP \
                         cannot select the tree to execute when it loads the file. Add \
                         main_tree_to_execute=\"<tree ID>\" to <root>, or select the tree in \
                         the executor.",
                    )
                };
                self.push(
                    severity,
                    ProblemCode::AmbiguousMainTree,
                    None,
                    root,
                    Some("main_tree_to_execute"),
                    message.to_owned(),
                );
            }
            MainTree::Single(_) | MainTree::None => {}
        }
    }

    /// True if a `<SubTree>` of another file of the project uses a tree ID
    /// of this file: the file is a library.
    fn is_library(&self) -> bool {
        let ids: Vec<&str> = self
            .bt
            .index()
            .trees
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        self.ctx.project().files().any(|(file, _, bt)| {
            let doc = bt.doc();
            file != self.file
                && bt.index().root.is_some_and(|root| {
                    doc.descendants(root).any(|node| {
                        doc.name(node) == Some("SubTree")
                            && doc
                                .attribute(node, "ID")
                                .is_some_and(|id| ids.contains(&id))
                    })
                })
        })
    }

    fn file_name(&self, file: FileId) -> String {
        self.ctx
            .project()
            .file(file)
            .map_or_else(|| file.to_string(), BtFile::display_name)
    }

    // ----------------------------------------------------------------
    // Trees
    // ----------------------------------------------------------------

    fn check_tree(&mut self, tree: &'p TreeEntry, is_main: bool) {
        let name = tree.id.as_str();
        let element = tree.element;
        self.check_amp_attributes(element, Some(name));
        self.check_tree_id(tree);
        match tree.root {
            None => self.push(
                Severity::Error,
                ProblemCode::EmptyTree,
                Some(name),
                Some(element),
                None,
                format!(
                    "The tree \"{name}\" has no root node. BT.CPP requires exactly one child \
                     element in <BehaviorTree>. Add a root node."
                ),
            ),
            Some(_) => {
                for &extra in &tree.extra_roots {
                    self.push(
                        Severity::Error,
                        ProblemCode::MultipleRoots,
                        Some(name),
                        Some(extra),
                        None,
                        format!(
                            "The tree \"{name}\" has more than one root node. BT.CPP requires \
                             exactly one child element in <BehaviorTree>. Put the nodes under \
                             one control node, for example a Sequence."
                        ),
                    );
                }
            }
        }
        let mut state = TreeState {
            name,
            attached: true,
            is_main,
            subtree_names: HashMap::new(),
            too_deep_reported: false,
        };
        let roots: Vec<NodeId> = self.doc.element_children(element).collect();
        self.walk(&roots, &mut state);
    }

    fn check_tree_id(&mut self, tree: &'p TreeEntry) {
        let name = tree.id.as_str();
        let element = tree.element;
        if !tree.has_explicit_id || tree.id.is_empty() {
            // VerifyXML: an ID is required when <root> has more than one
            // child element.
            let siblings = self
                .bt
                .index()
                .root
                .map_or(0, |r| self.doc.element_children(r).count());
            if siblings > 1 {
                self.push(
                    Severity::Error,
                    ProblemCode::MissingId,
                    Some(name),
                    Some(element),
                    Some("ID"),
                    "The <BehaviorTree> element has no ID. BT.CPP requires an ID when <root> \
                     has more than one child element. Add an ID attribute."
                        .to_owned(),
                );
            }
            return;
        }
        // VerifyXML: validateModelName on the tree ID.
        match validate_model_name(name) {
            Ok(()) => {}
            Err(NameError::ReservedRoot) => self.push(
                Severity::Error,
                ProblemCode::ReservedNodeName,
                Some(name),
                Some(element),
                Some("ID"),
                format!(
                    "\"{name}\" is a reserved name. BT.CPP does not accept it as a tree ID. \
                     Rename the tree."
                ),
            ),
            Err(error) => self.push(
                Severity::Error,
                ProblemCode::InvalidName,
                Some(name),
                Some(element),
                Some("ID"),
                format!(
                    "The tree ID \"{name}\" is not valid: {error}. BT.CPP stops when it loads \
                     the file. Rename the tree."
                ),
            ),
        }
        if self.is_registered_type(name) {
            self.push(
                Severity::Error,
                ProblemCode::TreeIdConflict,
                Some(name),
                Some(element),
                Some("ID"),
                format!(
                    "The tree ID \"{name}\" is also the name of a node type. BT.CPP does not \
                     accept a tree with the name of a registered node. Rename the tree."
                ),
            );
        }
        self.check_duplicate_tree(tree);
    }

    fn check_duplicate_tree(&mut self, tree: &'p TreeEntry) {
        let name = tree.id.as_str();
        let others: Vec<FileId> = self
            .ctx
            .trees_with_id(name)
            .iter()
            .filter(|t| {
                !(t.file == self.file
                    && self.bt.trees().get(t.index).map(|e| e.element) == Some(tree.element))
            })
            .map(|t| t.file)
            .collect();
        if others.is_empty() {
            return;
        }
        let (severity, message) = if others.contains(&self.file) {
            (
                Severity::Error,
                format!(
                    "The tree ID \"{name}\" is used more than once in this file. BT.CPP keeps \
                     only the last tree with this ID and ignores the others. Give each tree a \
                     different ID."
                ),
            )
        } else if let Some(&other) = others.iter().find(|&&f| {
            self.ctx.closure(self.file).contains(&f) || self.ctx.closure(f).contains(&self.file)
        }) {
            let other_name = self.file_name(other);
            let relation = if self.ctx.closure(self.file).contains(&other) {
                "which this file includes"
            } else {
                "which includes this file"
            };
            (
                Severity::Error,
                format!(
                    "The tree ID \"{name}\" is also used in {other_name}, {relation}. BT.CPP \
                     keeps only one of the two trees. Give each tree a different ID."
                ),
            )
        } else if self.is_main_tree_of(self.file, name)
            && others.iter().all(|&f| self.is_main_tree_of(f, name))
        {
            // Each file is a different program, for example one file for
            // each task. An executor usually loads only one of them.
            let other_name = self.file_name(others[0]);
            (
                Severity::Info,
                format!(
                    "The tree \"{name}\" is also the main tree of {other_name}. This is \
                     correct if the executor loads only one of these files. If it loads both, \
                     it keeps only one of the two trees."
                ),
            )
        } else {
            let other_name = self.file_name(others[0]);
            (
                Severity::Warning,
                format!(
                    "The tree ID \"{name}\" is also used in {other_name}. An executor that \
                     loads both files keeps only one of the two trees. Give each tree a \
                     different ID."
                ),
            )
        };
        self.push(
            severity,
            ProblemCode::DuplicateTreeId,
            Some(name),
            Some(tree.element),
            Some("ID"),
            message,
        );
    }

    /// True if `main_tree_to_execute` of the file names the tree `id`.
    fn is_main_tree_of(&self, file: FileId, id: &str) -> bool {
        self.ctx
            .project()
            .file(file)
            .is_some_and(|bt| bt.index().main_tree_to_execute.as_deref() == Some(id))
    }

    /// True if BT.CPP has a registered node type with this ID: a built-in
    /// type, or a declared model that is not a SubTree model (the manifests
    /// of `BehaviorTreeFactory`).
    fn is_registered_type(&self, id: &str) -> bool {
        let registry = self.ctx.project().registry();
        builtin::is_builtin(id)
            || registry
                .lookup_declared(self.file, id)
                .is_some_and(|m| m.kind != NodeKind::SubTree)
    }

    // ----------------------------------------------------------------
    // Nodes
    // ----------------------------------------------------------------

    /// Checks the nodes below `roots`, depth first, in document order. The
    /// roots have depth 1 (the depth in `VerifyXML`, where the
    /// `<BehaviorTree>` has depth 0).
    fn walk(&mut self, roots: &[NodeId], state: &mut TreeState<'p>) {
        let mut stack: Vec<(NodeId, usize)> = roots.iter().rev().map(|&n| (n, 1)).collect();
        while let Some((node, depth)) = stack.pop() {
            let Some(view) = NodeView::new(self.doc, node) else {
                continue;
            };
            if state.attached && depth > MAX_NESTING_DEPTH && !state.too_deep_reported {
                state.too_deep_reported = true;
                self.push(
                    Severity::Error,
                    ProblemCode::NestingTooDeep,
                    Some(state.name),
                    Some(node),
                    None,
                    format!(
                        "This node is {depth} levels below <BehaviorTree>. BT.CPP accepts at \
                         most {MAX_NESTING_DEPTH} levels. Move a part of the tree into a \
                         separate tree and use it with <SubTree>."
                    ),
                );
            }
            self.check_node(view, state);
            let first = stack.len();
            stack.extend(self.doc.element_children(node).map(|c| (c, depth + 1)));
            stack[first..].reverse();
        }
    }

    fn check_node(&mut self, view: NodeView<'p>, state: &mut TreeState<'p>) {
        let node_type = if view.is_subtree() {
            self.check_subtree_target(view, state)
        } else {
            self.check_node_type(view, state.name)
        };
        self.check_children(view, &node_type, state.name);
        self.check_attributes(view, &node_type, state);
        self.check_required_ports(view, &node_type, state.name);
    }

    /// The node form and the type of a node that is not a `<SubTree>`.
    fn check_node_type(&mut self, view: NodeView<'p>, tree: &str) -> NodeType<'p> {
        let node = Some(view.id());
        let tag = view.tag();
        let explicit = view.category_tag();
        let type_id = match explicit {
            Some(_) => match view.attribute("ID").filter(|id| !id.is_empty()) {
                Some(id) => id,
                None => {
                    // VerifyXML: "The tag <Action> must have the attribute [ID]".
                    self.push(
                        Severity::Error,
                        ProblemCode::MissingId,
                        Some(tree),
                        node,
                        Some("ID"),
                        format!(
                            "The <{tag}> element has no ID. BT.CPP requires the node type in \
                             the ID attribute, for example <{tag} ID=\"MyNode\"/>. Add the ID."
                        ),
                    );
                    return NodeType::Unknown;
                }
            },
            None => {
                if view.attribute("ID").is_some() {
                    // createNodeFromXML: "Attribute [ID] is not allowed".
                    self.push(
                        Severity::Error,
                        ProblemCode::UnexpectedId,
                        Some(tree),
                        node,
                        Some("ID"),
                        format!(
                            "The attribute ID is not allowed in <{tag}>. BT.CPP accepts ID only \
                             in <Action>, <Condition>, <Control>, <Decorator> and <SubTree>. \
                             Remove the attribute."
                        ),
                    );
                }
                // VerifyXML: validateModelName on the element name.
                match validate_model_name(tag) {
                    Ok(()) => {}
                    Err(NameError::ReservedRoot) => {
                        self.push(
                            Severity::Error,
                            ProblemCode::ReservedNodeName,
                            Some(tree),
                            node,
                            None,
                            format!(
                                "\"{tag}\" is a reserved name. BT.CPP does not accept it as a \
                                 node type. Rename the node type."
                            ),
                        );
                        return NodeType::Unknown;
                    }
                    Err(error) => {
                        self.push(
                            Severity::Error,
                            ProblemCode::InvalidName,
                            Some(tree),
                            node,
                            None,
                            format!(
                                "The node type name \"{tag}\" is not valid: {error}. BT.CPP \
                                 stops when it loads the file. Rename the node type."
                            ),
                        );
                        return NodeType::Unknown;
                    }
                }
                tag
            }
        };
        let registry = self.ctx.project().registry();
        let model = registry
            .lookup_declared(self.file, type_id)
            .filter(|m| m.kind != NodeKind::SubTree);
        let Some(model) = model else {
            self.report_unknown_type(view, type_id, tree);
            return NodeType::Unknown;
        };
        if let Some(tag_kind) = explicit
            && tag_kind != model.kind
        {
            let kind = model.kind;
            self.push(
                Severity::Warning,
                ProblemCode::KindMismatch,
                Some(tree),
                node,
                None,
                format!(
                    "The tag <{tag}> does not match the node type \"{type_id}\", which is a \
                     {kind} node. BT.CPP uses the kind of the node type and ignores the tag. \
                     Use <{kind} ID=\"{type_id}\"> or <{type_id}>."
                ),
            );
        }
        NodeType::Known(model)
    }

    fn report_unknown_type(&mut self, view: NodeView<'p>, type_id: &str, tree: &str) {
        let registry = self.ctx.project().registry();
        let is_subtree_model = registry
            .lookup_declared(self.file, type_id)
            .is_some_and(|m| m.kind == NodeKind::SubTree);
        let is_tree = matches!(
            self.ctx.resolve(self.file, type_id),
            Resolved::InClosure(_) | Resolved::Elsewhere(_)
        );
        let message = if type_id == "SubTree" {
            "\"SubTree\" is not a node type that BT.CPP can use in this form. Use \
             <SubTree ID=\"TreeName\"/> to use a tree."
                .to_owned()
        } else if is_tree || is_subtree_model {
            format!(
                "\"{type_id}\" is a tree, not a node type. BT.CPP does not accept a tree in \
                 this form. Use <SubTree ID=\"{type_id}\"/>."
            )
        } else {
            format!(
                "The node type \"{type_id}\" is not known. BT.CPP stops with \"Node not \
                 recognized\". Add a model for \"{type_id}\" (in a <TreeNodesModel> or in a \
                 project model file), or correct the name."
            )
        };
        self.push(
            Severity::Error,
            ProblemCode::UnknownNodeType,
            Some(tree),
            Some(view.id()),
            None,
            message,
        );
    }

    /// Checks the target of a `<SubTree>` and returns its model.
    fn check_subtree_target(
        &mut self,
        view: NodeView<'p>,
        state: &mut TreeState<'p>,
    ) -> NodeType<'p> {
        let tree = state.name;
        let node = Some(view.id());
        let Some(target) = view.subtree_id().filter(|id| !id.is_empty()) else {
            // VerifyXML: "The tag <SubTree> must have the attribute [ID]".
            self.push(
                Severity::Error,
                ProblemCode::MissingId,
                Some(tree),
                node,
                Some("ID"),
                "The <SubTree> element has no ID. Add the ID of the tree to use, for \
                 example <SubTree ID=\"MyTree\"/>."
                    .to_owned(),
            );
            return NodeType::SubTree(None);
        };
        // VerifyXML: validateModelName on the SubTree ID.
        match validate_model_name(target) {
            Ok(()) => {}
            Err(NameError::ReservedRoot) => self.push(
                Severity::Error,
                ProblemCode::ReservedNodeName,
                Some(tree),
                node,
                Some("ID"),
                format!(
                    "\"{target}\" is a reserved name. BT.CPP does not accept it as a tree ID. \
                     Rename the tree."
                ),
            ),
            Err(error) => self.push(
                Severity::Error,
                ProblemCode::InvalidName,
                Some(tree),
                node,
                Some("ID"),
                format!(
                    "The SubTree ID \"{target}\" is not valid: {error}. BT.CPP stops when it \
                     loads the file. Rename the tree."
                ),
            ),
        }
        if self.is_registered_type(target) {
            // VerifyXML: "The attribute [ID] of tag <SubTree> must not use
            // the name of a registered Node".
            self.push(
                Severity::Error,
                ProblemCode::TreeIdConflict,
                Some(tree),
                node,
                Some("ID"),
                format!(
                    "The SubTree ID \"{target}\" is the name of a node type. BT.CPP does not \
                     accept it. Use <{target}/> for the node, or rename the tree."
                ),
            );
            return NodeType::SubTree(None);
        }
        match self.ctx.resolve(self.file, target) {
            Resolved::InClosure(_) => {}
            Resolved::Elsewhere(found) => {
                let other = self.file_name(found.file);
                self.push(
                    Severity::Info,
                    ProblemCode::TreeNotIncluded,
                    Some(tree),
                    node,
                    Some("ID"),
                    format!(
                        "The tree \"{target}\" is in {other}, which this file does not include. \
                         BT.CPP finds it only if the executor also loads {other}. To make the \
                         file complete, add <include path=\"...\"/> for {other}."
                    ),
                );
            }
            Resolved::Missing => self.push(
                Severity::Error,
                ProblemCode::MissingSubTree,
                Some(tree),
                node,
                Some("ID"),
                format!(
                    "The SubTree refers to the tree \"{target}\", but no tree has this ID. \
                     BT.CPP stops with \"Can't find a tree with name\". Add the tree, include \
                     the file that has it, or correct the ID."
                ),
            ),
        }
        if state.attached {
            match self.ctx.cycle(self.file, view.id()) {
                Some(Cycle::SelfRef) => self.push(
                    Severity::Error,
                    ProblemCode::SelfSubTree,
                    Some(tree),
                    node,
                    Some("ID"),
                    format!(
                        "The tree \"{target}\" uses itself as a SubTree. BT.CPP stops with \
                         \"Recursive behavior tree cycle detected\". Remove this SubTree or \
                         use a different tree."
                    ),
                ),
                Some(Cycle::Path(path)) => {
                    let path = path.join(" -> ");
                    self.push(
                        Severity::Error,
                        ProblemCode::CircularSubTree,
                        Some(tree),
                        node,
                        Some("ID"),
                        format!(
                            "This SubTree makes a cycle of trees: {path}. BT.CPP stops with \
                             \"Recursive behavior tree cycle detected\". Remove one SubTree of \
                             the cycle."
                        ),
                    );
                }
                None => {}
            }
        }
        self.check_subtree_name(view, state);
        // recursivelyCreateSubtree uses the <SubTree> model of the
        // <TreeNodesModel> (subtree_models).
        let model = self
            .ctx
            .project()
            .registry()
            .lookup_declared(self.file, target)
            .filter(|m| m.kind == NodeKind::SubTree);
        NodeType::SubTree(model)
    }

    /// recursivelyCreateSubtree: the path of a SubTree instance is the path
    /// of its tree, `/` and the `name` attribute (or `ID::UID` without a
    /// name). Two equal paths stop the creation.
    fn check_subtree_name(&mut self, view: NodeView<'p>, state: &mut TreeState<'p>) {
        let Some(name) = view.name_attribute() else {
            return;
        };
        let tree = state.name;
        let node = Some(view.id());
        if let Some(&first) = state.subtree_names.get(name) {
            let line = self
                .doc
                .node_line_col(first)
                .map(|(l, _)| format!(" (line {l})"))
                .unwrap_or_default();
            self.push(
                Severity::Error,
                ProblemCode::DuplicateSubTreeName,
                Some(tree),
                node,
                Some("name"),
                format!(
                    "Another SubTree node in the tree \"{tree}\"{line} has the name \"{name}\". \
                     BT.CPP stops with \"Duplicate SubTree path detected\". Give each SubTree \
                     node a different name, or remove the name attribute."
                ),
            );
            return;
        }
        state.subtree_names.insert(name, view.id());
        if name.is_empty() {
            // The path of the SubTree is then the path of the root tree ("").
            let (severity, when) = if state.is_main {
                (Severity::Error, "This tree is the main tree, so")
            } else {
                (Severity::Warning, "When this tree is the root tree,")
            };
            self.push(
                severity,
                ProblemCode::DuplicateSubTreeName,
                Some(tree),
                node,
                Some("name"),
                format!(
                    "The SubTree node has an empty name. {when} the SubTree gets the same path \
                     as the root tree and BT.CPP stops with \"Duplicate SubTree path \
                     detected\". Give the SubTree node a name, or remove the name attribute."
                ),
            );
        }
    }

    fn check_children(&mut self, view: NodeView<'p>, node_type: &NodeType<'p>, tree: &str) {
        let (rule, label) = match node_type {
            // VerifyXML: "<SubTree> ... should not have any child".
            NodeType::SubTree(_) => (ChildRule::Exactly(0), "SubTree"),
            // BT.CPP keys the special rules on the registered ID (a
            // second registration of a built-in ID is not possible), so
            // they apply also when a project model re-declares the type.
            NodeType::Known(model) => match builtin::child_rule(&model.id, Some(model.kind)) {
                Some(rule) => (rule, model.id.as_str()),
                None => return,
            },
            NodeType::Unknown => return,
        };
        let count = view.child_count();
        if !rule.allows(count) {
            let has = match count {
                0 => "it has no children".to_owned(),
                1 => "it has 1 child".to_owned(),
                n => format!("it has {n} children"),
            };
            let fix = if count < rule.min() {
                let missing = rule.min() - count;
                if missing == 1 {
                    "Add a child node.".to_owned()
                } else {
                    format!("Add {missing} child nodes.")
                }
            } else if rule.max() == Some(0) {
                "Remove the child nodes: this node cannot have children.".to_owned()
            } else if rule.max() == Some(1) {
                "Put the child nodes under one control node, for example a Sequence.".to_owned()
            } else {
                let extra = count - rule.max().unwrap_or(count);
                if extra == 1 {
                    "Remove 1 child node.".to_owned()
                } else {
                    format!("Remove {extra} child nodes.")
                }
            };
            self.push(
                Severity::Error,
                ProblemCode::ChildCount,
                Some(tree),
                Some(view.id()),
                None,
                format!("\"{label}\" must have {rule}, but {has}. {fix}"),
            );
        }
        // VerifyXML: `registered_name == "ReactiveSequence"`.
        if let NodeType::Known(model) = node_type
            && model.id == "ReactiveSequence"
        {
            self.check_reactive_children(view, tree);
        }
    }

    /// The `ReactiveSequence` rules of `VerifyXML`.
    fn check_reactive_children(&mut self, view: NodeView<'p>, tree: &str) {
        let registry = self.ctx.project().registry();
        let mut async_count = 0;
        for child in view.children() {
            let tag = child.tag();
            let registered = registry
                .lookup_declared(self.file, tag)
                .filter(|m| m.kind != NodeKind::SubTree || tag == "SubTree");
            match registered {
                None if child.is_explicit_form() => {
                    let id = child.type_id().unwrap_or("MyNode");
                    self.push(
                        Severity::Error,
                        ProblemCode::ReactiveSequenceChild,
                        Some(tree),
                        Some(child.id()),
                        None,
                        format!(
                            "BT.CPP 4.9 does not accept the explicit form <{tag} ID=\"{id}\"> as \
                             a child of a ReactiveSequence: it stops with \"Unknown node type: \
                             {tag}\". Use the compact form <{id}/>."
                        ),
                    );
                }
                Some(m) if m.kind == NodeKind::Control && ASYNC_CHILD_TYPES.contains(&tag) => {
                    async_count += 1;
                    if async_count == 2 {
                        self.push(
                            Severity::Error,
                            ProblemCode::ReactiveSequenceChild,
                            Some(tree),
                            Some(child.id()),
                            None,
                            format!(
                                "A ReactiveSequence can have only one asynchronous child such as \
                                 {tag}. BT.CPP stops with \"A ReactiveSequence cannot have more \
                                 than one async child\". Use a Sequence, or move the children \
                                 into one asynchronous node."
                            ),
                        );
                    }
                }
                _ => {}
            }
        }
    }

    fn check_attributes(
        &mut self,
        view: NodeView<'p>,
        node_type: &NodeType<'p>,
        state: &TreeState<'p>,
    ) {
        let tree = state.name;
        let node = Some(view.id());
        let is_subtree = matches!(node_type, NodeType::SubTree(_));
        for (attr, raw) in view.attributes().zip(view.element().attributes()) {
            let name = attr.name;
            let value = attr.value;
            match attr.class {
                AttributeClass::Name => {
                    // createNodeFromXML: validateInstanceName.
                    if let Err(error) = validate_instance_name(value) {
                        self.push(
                            Severity::Error,
                            ProblemCode::InvalidName,
                            Some(tree),
                            node,
                            Some(name),
                            format!(
                                "The node name is not valid: {error}. BT.CPP stops when it \
                                 creates the tree. Remove the character."
                            ),
                        );
                    }
                }
                AttributeClass::AutoRemap if !is_subtree => self.push(
                    Severity::Warning,
                    ProblemCode::BadRemapping,
                    Some(tree),
                    node,
                    Some(name),
                    "The attribute _autoremap has an effect only on a <SubTree> node. BT.CPP \
                     ignores it here. Remove the attribute."
                        .to_owned(),
                ),
                AttributeClass::AutoRemap => {
                    if let Some(Err(text)) = view.autoremap() {
                        // recursivelyCreateSubtree: convertFromString<bool>.
                        self.push(
                            Severity::Error,
                            ProblemCode::BadRemapping,
                            Some(tree),
                            node,
                            Some(name),
                            format!(
                                "The value \"{text}\" of _autoremap is not a boolean. BT.CPP \
                                 stops with \"invalid bool conversion\". Use true or false."
                            ),
                        );
                    }
                }
                AttributeClass::PreCondition(_) | AttributeClass::PostCondition(_) => {
                    let syntax = crate::script::check_script(value);
                    if let Err(error) = &syntax
                        && error.message != crate::script::EMPTY_SCRIPT
                    {
                        // instantiateTreeNode: AssignConditions.
                        self.push(
                            Severity::Error,
                            ProblemCode::ScriptSyntax,
                            Some(tree),
                            node,
                            Some(name),
                            format!(
                                "The script of {name} has an error: {}. BT.CPP stops when it \
                                 creates the tree. Correct the script.",
                                error.message
                            ),
                        );
                    }
                    let value_is_used = matches!(attr.class, AttributeClass::PreCondition(_));
                    self.check_script_effect(view, name, value, value_is_used, tree);
                    if value.trim_matches([' ', '\t', '\n', '\r']).is_empty() {
                        self.push(
                            Severity::Error,
                            ProblemCode::EmptyCondition,
                            Some(tree),
                            node,
                            Some(name),
                            format!(
                                "The script of {name} is empty. BT.CPP stops with \"Empty \
                                 Script\" when it creates the tree. Write a script, or remove \
                                 the attribute."
                            ),
                        );
                    }
                }
                AttributeClass::NotAPort => self.push(
                    Severity::Warning,
                    ProblemCode::InvalidName,
                    Some(tree),
                    node,
                    Some(name),
                    format!(
                        "BT.CPP ignores the attribute \"{name}\" because it is not a valid port \
                         name. A port name starts with a letter and has no spaces or special \
                         characters such as . : / Rename or remove the attribute."
                    ),
                ),
                AttributeClass::Port => {
                    if is_subtree {
                        self.check_remapping(view, node_type, name, value, tree);
                    } else {
                        self.check_port(view, node_type, name, value, tree);
                    }
                }
                AttributeClass::OtherUnderscore if is_subtree && name == "__shared_blackboard" => {
                    // BT.CPP 3 used this attribute. BT.CPP 4 does not read
                    // it (recursivelyCreateSubtree reads only _autoremap
                    // and the allowed port names).
                    self.push(
                        Severity::Warning,
                        ProblemCode::BadRemapping,
                        Some(tree),
                        node,
                        Some(name),
                        "BT.CPP 4 ignores the attribute __shared_blackboard of BT.CPP 3, so \
                         the SubTree does not share the blackboard. Use _autoremap=\"true\" \
                         or remap the ports one by one."
                            .to_owned(),
                    );
                }
                AttributeClass::Id
                | AttributeClass::RuntimeMeta(_)
                | AttributeClass::OtherUnderscore => {}
            }
            self.check_amp(raw, tree.into(), node);
        }
    }

    /// Warns about each statement of a script that changes nothing (see
    /// [`crate::script::statements_without_effect`]).
    fn check_script_effect(
        &mut self,
        view: NodeView<'p>,
        name: &str,
        script: &str,
        value_is_used: bool,
        tree: &str,
    ) {
        for range in crate::script::statements_without_effect(script, value_is_used) {
            let statement = &script[range];
            let advice = match crate::script::assignment_for(statement) {
                Some(fixed) => format!("To set the value, write \"{fixed}\"."),
                None => "Remove the statement, or use := to set a value.".to_owned(),
            };
            self.push(
                Severity::Warning,
                ProblemCode::ScriptNoEffect,
                Some(tree),
                Some(view.id()),
                Some(name),
                format!(
                    "The statement \"{statement}\" in {name} does not change a value, and \
                     BT.CPP does not use its result. {advice}"
                ),
            );
        }
    }

    /// Checks a port attribute of a node that is not a SubTree.
    fn check_port(
        &mut self,
        view: NodeView<'p>,
        node_type: &NodeType<'p>,
        name: &str,
        value: &str,
        tree: &str,
    ) {
        let node = Some(view.id());
        self.check_blackboard_value(view, name, value, tree);
        let NodeType::Known(model) = node_type else {
            return;
        };
        // The constructors of these built-in nodes parse their script
        // (loadExecutor), so a script with an error stops the creation of
        // the tree. A blackboard reference is read later.
        let script_port = matches!(
            (model.id.as_str(), name),
            ("Script" | "ScriptCondition", "code") | ("Precondition", "if")
        );
        if script_port
            && builtin::is_builtin(&model.id)
            && blackboard_pointer(value).is_none()
            && let Err(error) = crate::script::check_script(value)
        {
            self.push(
                Severity::Error,
                ProblemCode::ScriptSyntax,
                Some(tree),
                node,
                Some(name),
                format!(
                    "The script in the port {name} has an error: {}. BT.CPP stops when it \
                     creates the node. Correct the script.",
                    error.message
                ),
            );
        }
        if script_port && builtin::is_builtin(&model.id) && blackboard_pointer(value).is_none() {
            // ScriptNode::tick ignores the value of the script.
            let value_is_used = model.id != "Script";
            self.check_script_effect(view, name, value, value_is_used, tree);
        }
        let Some(port) = model.port(name) else {
            // createNodeFromXML: "a port with name [..] is found in the XML
            // but not in the providedPorts() of its registered node type".
            self.push(
                Severity::Error,
                ProblemCode::UnknownPort,
                Some(tree),
                node,
                Some(name),
                format!(
                    "The node type \"{}\" has no port \"{name}\". BT.CPP stops when it \
                     creates the tree. {} Correct the port name, or remove the attribute.",
                    model.id,
                    port_list(model)
                ),
            );
            return;
        };
        if port.direction == PortDirection::Output
            && matches!(PortValue::parse(value), PortValue::Literal(_))
        {
            // TreeNode::setOutput: "setOutput requires a blackboard pointer".
            self.push(
                Severity::Warning,
                ProblemCode::OutputPortLiteral,
                Some(tree),
                node,
                Some(name),
                format!(
                    "The output port \"{name}\" has the fixed value \"{value}\". A node can \
                     write an output only to a blackboard entry, so the write fails when the \
                     tree runs. Use a blackboard reference such as {{{name}}}."
                ),
            );
        }
        self.check_conversion(view, port, value, tree);
    }

    /// createNodeFromXML: a literal value of a strongly typed port must
    /// convert to the port type.
    fn check_conversion(&mut self, view: NodeView<'p>, port: &PortModel, value: &str, tree: &str) {
        if is_blackboard_for_conversion(value) {
            return;
        }
        let Some(type_name) = port.type_name.as_deref() else {
            return;
        };
        if literal_converts(type_name, value) == Some(false) {
            let name = &port.name;
            self.push(
                Severity::Error,
                ProblemCode::InvalidPortValue,
                Some(tree),
                Some(view.id()),
                Some(name),
                format!(
                    "The value \"{value}\" of the port \"{name}\" cannot be converted to \
                     {type_name}. BT.CPP stops when it creates the tree. Use a valid value, or \
                     a blackboard reference such as {{{name}}}."
                ),
            );
        }
    }

    /// Checks a remapping attribute of a `<SubTree>` node.
    fn check_remapping(
        &mut self,
        view: NodeView<'p>,
        node_type: &NodeType<'p>,
        name: &str,
        value: &str,
        tree: &str,
    ) {
        let node = Some(view.id());
        if value == "=" {
            // recursivelyCreateSubtree changes only "{=}", not "=".
            self.push(
                Severity::Warning,
                ProblemCode::BadRemapping,
                Some(tree),
                node,
                Some(name),
                format!(
                    "In a SubTree, \"=\" is the text \"=\", not a remapping. Use \"{{=}}\" to \
                     connect the port \"{name}\" to the blackboard entry with the same name."
                ),
            );
        } else {
            self.check_blackboard_value(view, name, value, tree);
        }
        let NodeType::SubTree(Some(model)) = node_type else {
            return;
        };
        match model.port(name) {
            None => self.push(
                Severity::Warning,
                ProblemCode::UnknownPort,
                Some(tree),
                node,
                Some(name),
                format!(
                    "The model of the tree \"{}\" has no port \"{name}\". BT.CPP still copies \
                     the value into the blackboard of the SubTree. {} Add the port to the \
                     model, or correct the name.",
                    model.id,
                    port_list(model)
                ),
            ),
            Some(port)
                if port.direction == PortDirection::Output
                    && value != "="
                    && matches!(PortValue::parse_remapping(value), PortValue::Literal(_)) =>
            {
                self.push(
                    Severity::Warning,
                    ProblemCode::OutputPortLiteral,
                    Some(tree),
                    node,
                    Some(name),
                    format!(
                        "The output port \"{name}\" of the SubTree has the fixed value \
                         \"{value}\". The SubTree writes the output only into its own \
                         blackboard, so this tree never gets it. Use a blackboard reference \
                         such as {{{name}}}."
                    ),
                );
            }
            Some(_) => {}
        }
    }

    fn check_blackboard_value(&mut self, view: NodeView<'p>, name: &str, value: &str, tree: &str) {
        let Some(problem) = check_blackboard_ref(value) else {
            return;
        };
        let message = match problem {
            RefProblem::NoClosingBrace => format!(
                "The value \"{value}\" of \"{name}\" starts with {{ but does not end with }}. \
                 BT.CPP reads it as text, not as a blackboard reference. Add the closing }}."
            ),
            RefProblem::NoOpeningBrace => format!(
                "The value \"{value}\" of \"{name}\" ends with }} but does not start with {{. \
                 BT.CPP reads it as text, not as a blackboard reference. Add the opening {{."
            ),
            RefProblem::EmptyKey => format!(
                "The blackboard reference \"{value}\" of \"{name}\" has no key. Write the key \
                 between the braces, for example {{{name}}}."
            ),
            RefProblem::BraceInKey(key) => format!(
                "BT.CPP reads \"{value}\" as a blackboard reference with the key \"{key}\". \
                 A key with braces is probably a mistake. Use one key, for example {{{name}}}. \
                 For a JSON value, write json: before it."
            ),
            RefProblem::SpaceInKey(key) => format!(
                "The blackboard key \"{key}\" of \"{name}\" starts or ends with white space. \
                 BT.CPP keeps the spaces as part of the key. Remove them."
            ),
        };
        self.push(
            Severity::Warning,
            ProblemCode::BadBlackboardRef,
            Some(tree),
            Some(view.id()),
            Some(name),
            message,
        );
    }

    fn check_required_ports(&mut self, view: NodeView<'p>, node_type: &NodeType<'p>, tree: &str) {
        let node = Some(view.id());
        match node_type {
            NodeType::Known(model) => {
                for port in &model.ports {
                    if model.is_port_required(port) && view.attribute(&port.name).is_none() {
                        let name = &port.name;
                        self.push(
                            Severity::Warning,
                            ProblemCode::MissingRequiredPort,
                            Some(tree),
                            node,
                            Some(name),
                            format!(
                                "The {} port \"{name}\" of \"{}\" has no value and no default \
                                 value. The node fails when it reads the port. Set a value or \
                                 a blackboard reference such as {{{name}}}.",
                                port.direction, model.id
                            ),
                        );
                    }
                }
            }
            NodeType::SubTree(Some(model)) => {
                // recursivelyCreateSubtree: each port of the SubTree model
                // without a default must be remapped, unless _autoremap.
                match view.autoremap() {
                    Some(Ok(true)) | Some(Err(_)) => return,
                    Some(Ok(false)) | None => {}
                }
                for port in &model.ports {
                    let remapped =
                        is_allowed_port_name(&port.name) && view.attribute(&port.name).is_some();
                    if model.is_port_required(port) && !remapped {
                        let name = &port.name;
                        self.push(
                            Severity::Error,
                            ProblemCode::MissingRequiredPort,
                            Some(tree),
                            node,
                            Some(name),
                            format!(
                                "The model of the tree \"{}\" has the port \"{name}\" without a \
                                 default value, but this SubTree node does not remap it. BT.CPP \
                                 stops when it creates the tree. Set the port, for example \
                                 {name}=\"{{{name}}}\", add _autoremap=\"true\", or give the \
                                 port a default value in the model.",
                                model.id
                            ),
                        );
                    }
                }
            }
            NodeType::SubTree(None) | NodeType::Unknown => {}
        }
    }

    // ----------------------------------------------------------------
    // Unconnected nodes
    // ----------------------------------------------------------------

    fn check_unconnected(&mut self) {
        let project = self.ctx.project();
        for (file, tree, item) in project.all_unconnected() {
            if file != self.file {
                continue;
            }
            let node = item.node;
            // A node that has a parent again is part of the tree.
            if !self.doc.is_element(node) || self.doc.parent(node).is_some() {
                continue;
            }
            self.push(
                Severity::Warning,
                ProblemCode::UnconnectedNode,
                Some(tree),
                Some(node),
                None,
                "This node has no parent. FastTick does not write it to the XML file. Connect \
                 it to a node of the tree, or delete it."
                    .to_owned(),
            );
            let mut state = TreeState {
                name: tree,
                attached: false,
                is_main: false,
                subtree_names: HashMap::new(),
                too_deep_reported: false,
            };
            self.walk(&[node], &mut state);
        }
    }

    // ----------------------------------------------------------------
    // Bare ampersands
    // ----------------------------------------------------------------

    fn check_amp(&mut self, attribute: &Attribute, tree: Option<&str>, node: Option<NodeId>) {
        if bad_ampersand(attribute.raw_value()).is_none() {
            return;
        }
        let name = attribute.name();
        let strict = decode_strict(attribute.raw_value());
        let changed = if strict != attribute.value() {
            format!(" BT.CPP reads this value as \"{}\".", attribute.value())
        } else {
            String::new()
        };
        self.push(
            Severity::Warning,
            ProblemCode::SuspiciousAmpersand,
            tree,
            node,
            Some(name),
            format!(
                "The value of \"{name}\" contains an & that does not start a reference such as \
                 &amp;. Other XML tools reject the file.{changed} Write &amp; for the \
                 character &."
            ),
        );
    }

    /// Checks the attributes of one element.
    fn check_amp_attributes(&mut self, element: NodeId, tree: Option<&str>) {
        let doc = self.doc;
        for attribute in doc.attributes(element) {
            self.check_amp(attribute, tree, Some(element));
        }
    }

    /// Checks the attributes of an element and of all elements below it.
    fn check_amp_tree(&mut self, element: NodeId, tree: Option<&str>) {
        let doc = self.doc;
        for node in doc.descendants(element) {
            if doc.is_element(node) {
                self.check_amp_attributes(node, tree);
            }
        }
    }
}

/// Writes the control characters of a message (which can come from quoted
/// names and values) as escapes, so that the message is one line: `\n`,
/// `\r`, `\t` and `\u{..}`.
fn one_line(message: String) -> String {
    if !message.chars().any(char::is_control) {
        return message;
    }
    let mut out = String::with_capacity(message.len() + 8);
    for c in message.chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// The list of the ports of a model, for a message.
fn port_list(model: &NodeModel) -> String {
    if model.ports.is_empty() {
        return format!("\"{}\" has no ports.", model.id);
    }
    let names: Vec<&str> = model.ports.iter().map(|p| p.name.as_str()).collect();
    format!("The ports of \"{}\" are: {}.", model.id, names.join(", "))
}
