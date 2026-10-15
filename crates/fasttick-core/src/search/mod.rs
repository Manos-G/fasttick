//! Search for nodes by name, type, port or condition.
//!
//! A [`Query`] has a text, the fields to look in ([`Fields`]) and three
//! options: case sensitivity, whole words and wildcards. Three entry points
//! use it:
//!
//! - [`search_project`]: all trees of all files of a project, and the
//!   unconnected nodes.
//! - [`search_tree`]: one tree of one file.
//! - [`search_runtime`]: a [`RuntimeTree`] (the live and replay views).
//!
//! # Matching rules
//!
//! - Without wildcards, the text matches as a substring. With
//!   [`Query::whole_word`], the characters next to the match must not be
//!   letters, digits or `_`.
//! - With [`Query::wildcard`] and a text that contains `*` or `?`, the text
//!   is a pattern for the whole field value: `*` matches any sequence and
//!   `?` matches one character. For example `Move*` matches `MoveBase`.
//! - By default the match ignores case.
//! - An empty text matches nothing.
//!
//! # Results
//!
//! The results are in document order: the files in project order, the
//! trees in file order, the nodes in pre-order, and for each node the
//! fields in this order: name, type, port names and values (in attribute
//! order), conditions. A node can match in more than one field; each field
//! gives one result. [`SearchMatch::range`] is the byte range of the match
//! in [`SearchMatch::text`].
//!
//! # Example
//!
//! ```
//! use fasttick_core::bt::BtFile;
//! use fasttick_core::project::Project;
//! use fasttick_core::search::{Field, Query, search_project};
//!
//! let xml = r#"<root BTCPP_format="4"><BehaviorTree ID="Main">
//!   <Sequence name="patrol"><MoveBase goal="{target}"/></Sequence>
//! </BehaviorTree></root>"#;
//! let file = BtFile::parse(xml, None).unwrap();
//! let (project, _) = Project::with_file("demo", file);
//!
//! let found = search_project(&project, &Query::new("target"));
//! assert_eq!(found.len(), 1);
//! assert_eq!(found[0].field, Field::PortValue);
//! assert_eq!(found[0].attribute.as_deref(), Some("goal"));
//! assert_eq!(&found[0].text[found[0].range.clone()], "target");
//! ```

mod matcher;

use std::ops::Range;

use crate::bt::{AttributeClass, BtFile, NodeView};
use crate::project::{FileId, Project};
use crate::runtime::{NodeIndex, RuntimeTree};
use crate::xml::NodeId;

use matcher::Matcher;

/// The fields that a [`Query`] looks in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fields {
    /// The instance name (the `name` attribute).
    pub name: bool,
    /// The type ID. For a `<SubTree>` node this is the ID of the tree that
    /// it refers to.
    pub type_id: bool,
    /// The names of the ports (and of SubTree remappings).
    pub port_names: bool,
    /// The values of the ports (and of SubTree remappings).
    pub port_values: bool,
    /// The pre-condition and post-condition scripts.
    pub conditions: bool,
}

impl Fields {
    /// All fields.
    pub const ALL: Fields = Fields {
        name: true,
        type_id: true,
        port_names: true,
        port_values: true,
        conditions: true,
    };

    /// No field.
    pub const NONE: Fields = Fields {
        name: false,
        type_id: false,
        port_names: false,
        port_values: false,
        conditions: false,
    };

    /// True if no field is selected.
    pub fn is_none(&self) -> bool {
        *self == Fields::NONE
    }
}

impl Default for Fields {
    fn default() -> Fields {
        Fields::ALL
    }
}

/// A search query. See the [module documentation](self) for the rules.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Query {
    /// The text to find.
    pub text: String,
    /// The fields to look in.
    pub fields: Fields,
    /// True: upper and lower case must be the same.
    pub case_sensitive: bool,
    /// True: the match must be a whole word (substring mode only).
    pub whole_word: bool,
    /// True: `*` and `?` are wildcards, and the pattern must match the
    /// whole field value.
    pub wildcard: bool,
}

impl Query {
    /// A query for `text` in all fields: substring, case ignored.
    pub fn new(text: impl Into<String>) -> Query {
        Query {
            text: text.into(),
            fields: Fields::ALL,
            case_sensitive: false,
            whole_word: false,
            wildcard: false,
        }
    }

    /// The same query, but only in these fields.
    pub fn in_fields(mut self, fields: Fields) -> Query {
        self.fields = fields;
        self
    }

    /// The same query with case sensitivity on or off.
    pub fn with_case_sensitive(mut self, on: bool) -> Query {
        self.case_sensitive = on;
        self
    }

    /// The same query with whole-word matching on or off.
    pub fn with_whole_word(mut self, on: bool) -> Query {
        self.whole_word = on;
        self
    }

    /// The same query with wildcards on or off.
    pub fn with_wildcard(mut self, on: bool) -> Query {
        self.wildcard = on;
        self
    }

    /// True if the query cannot match anything: the text is empty or no
    /// field is selected.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() || self.fields.is_none()
    }

    fn matcher(&self) -> Matcher {
        Matcher::new(
            &self.text,
            self.case_sensitive,
            self.whole_word,
            self.wildcard,
        )
    }
}

/// The field in which a match was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Field {
    /// The instance name.
    Name,
    /// The type ID (for a SubTree: the target tree ID).
    Type,
    /// The name of a port or remapping. [`SearchMatch::attribute`] holds it.
    PortName,
    /// The value of a port or remapping.
    PortValue,
    /// A pre-condition or post-condition script. The attribute holds the
    /// condition name (for example `_skipIf`).
    Condition,
}

/// Where a node of a project is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Location {
    /// In a tree: the index of the tree in the file and its ID.
    Tree {
        /// The index in [`crate::bt::BtIndex::trees`].
        index: usize,
        /// The tree ID.
        id: String,
    },
    /// An unconnected node that belongs to the tree with this ID.
    Unconnected {
        /// The tree ID.
        tree: String,
    },
}

impl Location {
    /// The tree ID.
    pub fn tree_id(&self) -> &str {
        match self {
            Location::Tree { id, .. } => id,
            Location::Unconnected { tree } => tree,
        }
    }
}

/// One match in a project.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SearchMatch {
    /// The file.
    pub file: FileId,
    /// The tree (or the unconnected area of a tree).
    pub location: Location,
    /// The node element.
    pub node: NodeId,
    /// The field.
    pub field: Field,
    /// The attribute name for port and condition fields.
    pub attribute: Option<String>,
    /// The text that matched (the field value).
    pub text: String,
    /// The byte range of the match in `text`.
    pub range: Range<usize>,
}

/// One match in a runtime tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeMatch {
    /// The node.
    pub node: NodeIndex,
    /// The field.
    pub field: Field,
    /// The attribute name for port and condition fields.
    pub attribute: Option<String>,
    /// The text that matched.
    pub text: String,
    /// The byte range of the match in `text`.
    pub range: Range<usize>,
}

/// One field value of a node, before matching.
struct Candidate<'a> {
    field: Field,
    attribute: Option<&'a str>,
    text: &'a str,
}

/// Collects the field values of one node in result order.
fn collect_candidates<'a>(
    name: Option<&'a str>,
    type_id: Option<&'a str>,
    attributes: impl Iterator<Item = (&'a str, &'a str)>,
    fields: &Fields,
    out: &mut Vec<Candidate<'a>>,
    conditions: &mut Vec<Candidate<'a>>,
) {
    out.clear();
    conditions.clear();
    if fields.name
        && let Some(name) = name
    {
        out.push(Candidate {
            field: Field::Name,
            attribute: None,
            text: name,
        });
    }
    if fields.type_id
        && let Some(type_id) = type_id
    {
        out.push(Candidate {
            field: Field::Type,
            attribute: None,
            text: type_id,
        });
    }
    for (key, value) in attributes {
        match AttributeClass::of(key) {
            AttributeClass::Port => {
                if fields.port_names {
                    out.push(Candidate {
                        field: Field::PortName,
                        attribute: Some(key),
                        text: key,
                    });
                }
                if fields.port_values {
                    out.push(Candidate {
                        field: Field::PortValue,
                        attribute: Some(key),
                        text: value,
                    });
                }
            }
            AttributeClass::PreCondition(_) | AttributeClass::PostCondition(_)
                if fields.conditions =>
            {
                conditions.push(Candidate {
                    field: Field::Condition,
                    attribute: Some(key),
                    text: value,
                });
            }
            _ => {}
        }
    }
    out.append(conditions);
}

/// Shared state of one project or tree search.
struct ProjectSearch<'q> {
    query: &'q Query,
    matcher: Matcher,
    out: Vec<SearchMatch>,
}

impl ProjectSearch<'_> {
    /// Searches all nodes below (and including) `root`, in pre-order.
    fn subtree(&mut self, file_id: FileId, file: &BtFile, location: &Location, root: NodeId) {
        let doc = file.doc();
        let mut candidates = Vec::new();
        let mut conditions = Vec::new();
        for node in doc.descendants(root) {
            let Some(view) = NodeView::new(doc, node) else {
                continue;
            };
            let attributes = view
                .element()
                .attributes()
                .iter()
                .map(|attribute| (attribute.name(), attribute.value()));
            collect_candidates(
                view.name_attribute(),
                view.type_id(),
                attributes,
                &self.query.fields,
                &mut candidates,
                &mut conditions,
            );
            for candidate in &candidates {
                if let Some(range) = self.matcher.find(candidate.text) {
                    self.out.push(SearchMatch {
                        file: file_id,
                        location: location.clone(),
                        node,
                        field: candidate.field,
                        attribute: candidate.attribute.map(str::to_owned),
                        text: candidate.text.to_owned(),
                        range,
                    });
                }
            }
        }
    }

    /// Searches one tree: its root node, the extra root nodes (an error
    /// that the validator reports) and all their descendants.
    fn tree(&mut self, file_id: FileId, file: &BtFile, tree_index: usize) {
        let Some(tree) = file.index().trees.get(tree_index) else {
            return;
        };
        let location = Location::Tree {
            index: tree_index,
            id: tree.id.clone(),
        };
        for root in tree.root.iter().chain(&tree.extra_roots) {
            self.subtree(file_id, file, &location, *root);
        }
    }
}

/// Searches one tree of a file. `tree_index` is the position in
/// [`crate::bt::BtIndex::trees`]; an index that does not exist gives no
/// result.
pub fn search_tree(
    file_id: FileId,
    file: &BtFile,
    tree_index: usize,
    query: &Query,
) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut search = ProjectSearch {
        query,
        matcher: query.matcher(),
        out: Vec::new(),
    };
    search.tree(file_id, file, tree_index);
    search.out
}

/// Searches all trees of all files of a project, then the unconnected
/// nodes. See the [module documentation](self) for the order.
pub fn search_project(project: &Project, query: &Query) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut search = ProjectSearch {
        query,
        matcher: query.matcher(),
        out: Vec::new(),
    };
    for (file_id, _role, file) in project.files() {
        for tree_index in 0..file.index().trees.len() {
            search.tree(file_id, file, tree_index);
        }
    }
    for (file_id, tree, unconnected) in project.all_unconnected() {
        let Some(file) = project.file(file_id) else {
            continue;
        };
        let location = Location::Unconnected {
            tree: tree.to_owned(),
        };
        search.subtree(file_id, file, &location, unconnected.node);
    }
    search.out
}

/// Searches a runtime tree (the tree of a live executor or of a log).
///
/// The fields are the same as for a project. The name field is the
/// instance name of the node ([`crate::runtime::RuntimeNode::name`]); the
/// metadata attributes `_uid` and `_fullpath` are not searched. The results
/// are in the order of [`RuntimeTree::nodes`] (document order of the XML).
pub fn search_runtime(tree: &RuntimeTree, query: &Query) -> Vec<RuntimeMatch> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let matcher = query.matcher();
    let mut candidates = Vec::new();
    let mut conditions = Vec::new();
    for (index, node) in tree.nodes.iter().enumerate() {
        let attributes = node
            .attributes
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()));
        collect_candidates(
            Some(node.name.as_str()),
            Some(node.type_id.as_str()),
            attributes,
            &query.fields,
            &mut candidates,
            &mut conditions,
        );
        for candidate in &candidates {
            if let Some(range) = matcher.find(candidate.text) {
                out.push(RuntimeMatch {
                    node: NodeIndex(index),
                    field: candidate.field,
                    attribute: candidate.attribute.map(str::to_owned),
                    text: candidate.text.to_owned(),
                    range,
                });
            }
        }
    }
    out
}
