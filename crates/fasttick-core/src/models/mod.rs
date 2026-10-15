//! Node models: the node types, their ports and their metadata.
//!
//! A node model describes one node type: the ID, the kind and the ports. The
//! models come from four places (see [`ModelSource`] and `docs/DESIGN.md`
//! section 2.4):
//!
//! 1. the `<TreeNodesModel>` of a tree file and of the files that it
//!    includes;
//! 2. the project model files (for example the output of
//!    `BT::writeTreeNodesModelXML`);
//! 3. the built-in models of BT.CPP 4.9.0 ([`crate::builtin`]);
//! 4. for a SubTree: a model made from the target `<BehaviorTree>`.
//!
//! [`parse_models_document`] reads models. [`add_model`],
//! [`replace_model`] and [`remove_model`] change the `<TreeNodesModel>` of
//! a document and return the changes that undo them. [`ModelRegistry`]
//! finds the model of a node type.
//!
//! # The model XML format
//!
//! `addNodeModelToXML` (`src/xml_parsing.cpp`) writes this format:
//!
//! ```xml
//! <root BTCPP_format="4">
//!     <TreeNodesModel>
//!         <Action ID="SayMessage">
//!             <input_port name="message" type="std::string" default="hello">Text to say</input_port>
//!             <MetadataFields>
//!                 <Metadata description="Says a message"/>
//!             </MetadataFields>
//!         </Action>
//!         <SubTree ID="Navigate">
//!             <input_port name="goal"/>
//!         </SubTree>
//!     </TreeNodesModel>
//! </root>
//! ```
//!
//! The port description is the text of the port element. BT.CPP itself reads
//! back only the `<SubTree>` models (`loadSubtreeModel` and
//! `parseSubtreeModelPorts`), and there it reads the description from a
//! `description` attribute. FastTick reads both: the text first, else the
//! attribute. It writes the text.

mod parse;
mod registry;
mod write;

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::project::FileId;

pub use parse::{
    ModelIssue, ModelIssueKind, ParsedModel, ParsedModels, is_model_section, model_sections,
    parse_model_element, parse_model_section, parse_models_document, parse_models_text,
};
pub use registry::{ModelRegistry, RegistryFile};
pub use write::{
    ModelWriteError, add_model, build_model_element, find_model_element, remove_model,
    replace_model, write_models_document,
};

/// The kind of a node (`BT::NodeType` without `UNDEFINED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NodeKind {
    /// A leaf that does work.
    Action,
    /// A leaf that checks a condition.
    Condition,
    /// A node with one or more children.
    Control,
    /// A node with exactly one child.
    Decorator,
    /// A reference to another tree.
    SubTree,
}

impl NodeKind {
    /// All kinds, in this order: Action, Condition, Control, Decorator,
    /// SubTree.
    pub const ALL: [NodeKind; 5] = [
        NodeKind::Action,
        NodeKind::Condition,
        NodeKind::Control,
        NodeKind::Decorator,
        NodeKind::SubTree,
    ];

    /// The XML tag of the kind (`toStr(NodeType)` in `src/basic_types.cpp`),
    /// for example `Action`.
    pub fn tag(self) -> &'static str {
        match self {
            NodeKind::Action => "Action",
            NodeKind::Condition => "Condition",
            NodeKind::Control => "Control",
            NodeKind::Decorator => "Decorator",
            NodeKind::SubTree => "SubTree",
        }
    }

    /// The kind for a tag (`convertFromString<NodeType>` in
    /// `src/basic_types.cpp`). The match is case-sensitive.
    pub fn from_tag(tag: &str) -> Option<NodeKind> {
        NodeKind::ALL.into_iter().find(|k| k.tag() == tag)
    }

    /// True for Action and Condition.
    pub fn is_leaf(self) -> bool {
        matches!(self, NodeKind::Action | NodeKind::Condition)
    }
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

/// The direction of a port (`BT::PortDirection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PortDirection {
    /// The node reads the port.
    Input,
    /// The node writes the port.
    Output,
    /// The node reads and writes the port.
    InOut,
}

impl PortDirection {
    /// All directions.
    pub const ALL: [PortDirection; 3] = [
        PortDirection::Input,
        PortDirection::Output,
        PortDirection::InOut,
    ];

    /// The element name in the model XML: `input_port`, `output_port` or
    /// `inout_port`.
    pub fn tag(self) -> &'static str {
        match self {
            PortDirection::Input => "input_port",
            PortDirection::Output => "output_port",
            PortDirection::InOut => "inout_port",
        }
    }

    /// The direction for an element name of the model XML.
    pub fn from_tag(tag: &str) -> Option<PortDirection> {
        PortDirection::ALL.into_iter().find(|d| d.tag() == tag)
    }

    /// True for Input and InOut: the node reads the port.
    pub fn is_input(self) -> bool {
        self != PortDirection::Output
    }

    /// True for Output and InOut: the node writes the port.
    pub fn is_output(self) -> bool {
        self != PortDirection::Input
    }
}

impl fmt::Display for PortDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PortDirection::Input => "input",
            PortDirection::Output => "output",
            PortDirection::InOut => "in/out",
        })
    }
}

/// The model of one port.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PortModel {
    /// The port name.
    pub name: String,
    /// The direction.
    pub direction: PortDirection,
    /// The type name, as `BT::demangle` writes it (for example `int` or
    /// `std::string`). `None` if the model XML has no `type` attribute.
    pub type_name: Option<String>,
    /// The default value as text. `None` if the model has no default.
    /// `Some("")` is an empty default (see [`PortModel::is_required`]).
    pub default: Option<String>,
    /// The description. `None` if it is missing or empty.
    pub description: Option<String>,
}

impl PortModel {
    /// Makes a port without type, default and description.
    pub fn new(name: impl Into<String>, direction: PortDirection) -> PortModel {
        PortModel {
            name: name.into(),
            direction,
            type_name: None,
            default: None,
            description: None,
        }
    }

    /// True if the port must have a value in the XML: it is an input or
    /// in/out port and it has no default value.
    ///
    /// This is the rule of the FastTick validator (`MissingRequiredPort`)
    /// for nodes that are not SubTrees. BT.CPP itself does not check it
    /// when it creates the tree: `createNodeFromXML` only copies the default
    /// values into the ports that the XML does not set. A missing input
    /// without default fails later, when the node calls `getInput` for it
    /// ("nor the manifest or the XML contain the key"). An empty default
    /// (`default=""`) counts as a default here, because `getInput` uses it.
    ///
    /// For SubTree models BT.CPP has a different rule; see
    /// [`NodeModel::is_port_required`].
    pub fn is_required(&self) -> bool {
        self.direction.is_input() && self.default.is_none()
    }
}

/// Where a model comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelSource {
    /// The embedded built-in models of BT.CPP 4.9.0.
    Builtin,
    /// The `<TreeNodesModel>` of a tree file of the project (the file
    /// itself or a file that it includes).
    File(FileId),
    /// A project model file.
    ProjectModels(FileId),
    /// Made from a `<BehaviorTree>` element in this file (for a SubTree
    /// without a declared model). It has no ports.
    InferredSubTree(FileId),
    /// Parsed outside a project (for example by [`parse_models_text`] or
    /// for the index of a file).
    Unattached,
}

/// The model of a node type (`BT::TreeNodeManifest`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeModel {
    /// The registration ID (for a SubTree model: the tree ID).
    pub id: String,
    /// The kind.
    pub kind: NodeKind,
    /// The ports, in the order of the XML.
    pub ports: Vec<PortModel>,
    /// The metadata (`<MetadataFields>`), as name/value pairs in order.
    pub metadata: Vec<(String, String)>,
    /// Where the model comes from.
    pub source: ModelSource,
}

impl NodeModel {
    /// Makes a model without ports and metadata.
    pub fn new(id: impl Into<String>, kind: NodeKind) -> NodeModel {
        NodeModel {
            id: id.into(),
            kind,
            ports: Vec::new(),
            metadata: Vec::new(),
            source: ModelSource::Unattached,
        }
    }

    /// The port with this name.
    pub fn port(&self, name: &str) -> Option<&PortModel> {
        self.ports.iter().find(|p| p.name == name)
    }

    /// The value of a metadata field.
    pub fn metadata_value(&self, name: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// A short description of the type: the `description` metadata field,
    /// else the FastTick text of a built-in node.
    pub fn description(&self) -> Option<&str> {
        self.metadata_value("description")
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .or_else(|| match self.source {
                ModelSource::Builtin => crate::builtin::description(&self.id),
                _ => None,
            })
    }

    /// True if a node of this type must set the port.
    ///
    /// - For a SubTree model, BT.CPP checks the rule when it creates the
    ///   tree (`recursivelyCreateSubtree` in `src/xml_parsing.cpp`): each
    ///   port of the model, of each direction, must be remapped if the
    ///   model has no default for it, unless `_autoremap` is true. An empty
    ///   default counts as no default there (`defaultValueString().empty()`).
    ///   This function does not know `_autoremap`; the caller checks it.
    /// - For other kinds, the rule is [`PortModel::is_required`].
    pub fn is_port_required(&self, port: &PortModel) -> bool {
        if self.kind == NodeKind::SubTree {
            port.default.as_deref().is_none_or(str::is_empty)
        } else {
            port.is_required()
        }
    }

    /// The ports that a node of this type must set (see
    /// [`NodeModel::is_port_required`]).
    pub fn required_ports(&self) -> impl Iterator<Item = &PortModel> {
        self.ports.iter().filter(|p| self.is_port_required(p))
    }

    /// Returns a copy with another source.
    pub fn with_source(mut self, source: ModelSource) -> NodeModel {
        self.source = source;
        self
    }

    /// True if the two models have the same content (all fields except the
    /// source).
    pub fn same_content(&self, other: &NodeModel) -> bool {
        self.id == other.id
            && self.kind == other.kind
            && self.ports == other.ports
            && self.metadata == other.metadata
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_directions() {
        for kind in NodeKind::ALL {
            assert_eq!(NodeKind::from_tag(kind.tag()), Some(kind));
            assert_eq!(kind.to_string(), kind.tag());
        }
        assert_eq!(NodeKind::from_tag("action"), None);
        assert!(NodeKind::Action.is_leaf());
        assert!(NodeKind::Condition.is_leaf());
        assert!(!NodeKind::SubTree.is_leaf());
        for d in PortDirection::ALL {
            assert_eq!(PortDirection::from_tag(d.tag()), Some(d));
        }
        assert!(PortDirection::InOut.is_input() && PortDirection::InOut.is_output());
        assert!(!PortDirection::Output.is_input());
        assert!(!PortDirection::Input.is_output());
        assert_eq!(PortDirection::InOut.to_string(), "in/out");
    }

    #[test]
    fn required_ports() {
        let mut model = NodeModel::new("A", NodeKind::Action);
        let mut p = PortModel::new("in", PortDirection::Input);
        assert!(p.is_required());
        p.default = Some(String::new());
        assert!(!p.is_required());
        model.ports.push(p.clone());
        model
            .ports
            .push(PortModel::new("out", PortDirection::Output));
        model.ports.push(PortModel::new("io", PortDirection::InOut));
        let names: Vec<&str> = model.required_ports().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["io"]);

        // SubTree: every port without a non-empty default.
        model.kind = NodeKind::SubTree;
        let names: Vec<&str> = model.required_ports().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["in", "out", "io"]);
        model.ports[0].default = Some("1".into());
        let names: Vec<&str> = model.required_ports().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["out", "io"]);
    }

    #[test]
    fn model_helpers() {
        let mut model = NodeModel::new("A", NodeKind::Action);
        model.metadata.push(("description".into(), "d".into()));
        model.ports.push(PortModel::new("x", PortDirection::Input));
        assert_eq!(model.metadata_value("description"), Some("d"));
        assert_eq!(model.metadata_value("other"), None);
        assert!(model.port("x").is_some());
        assert!(model.port("y").is_none());
        let other = model.clone().with_source(ModelSource::Builtin);
        assert!(model.same_content(&other));
        assert_ne!(model, other);
    }
}
