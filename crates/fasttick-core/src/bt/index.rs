//! The index of a BT.CPP file: trees, includes and model sections.

use crate::models::{
    ModelSource, ParsedModel, ParsedModels, is_model_section, parse_model_section,
};
use crate::xml::{Document, NodeId};

/// One `<BehaviorTree>` element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// The tree ID. If the element has no `ID` attribute, this is
    /// `BehaviorTree_<n>` (see [`BtIndex`]).
    pub id: String,
    /// True if the element has an `ID` attribute. The attribute can be
    /// empty; `VerifyXML` treats an empty ID like a missing one ("must have
    /// the attribute \[ID\]" when the root has more than one child element).
    pub has_explicit_id: bool,
    /// The `<BehaviorTree>` element.
    pub element: NodeId,
    /// The first child element: the root node of the tree.
    pub root: Option<NodeId>,
    /// More child elements. BT.CPP requires exactly one child ("must have
    /// exactly 1 child" in `VerifyXML`), so each one is an error.
    pub extra_roots: Vec<NodeId>,
}

/// One `<include>` element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeEntry {
    /// The `<include>` element.
    pub element: NodeId,
    /// The `path` attribute. BT.CPP requires it ("missing 'path'
    /// attribute").
    pub path: Option<String>,
    /// The `ros_pkg` attribute.
    pub ros_pkg: Option<String>,
}

/// The index of a BT.CPP file. It follows `loadDocImpl`
/// (`src/xml_parsing.cpp`).
///
/// - The root element is the first element of the document (tinyxml2
///   `RootElement`). BT.CPP requires the name `root` (`VerifyXML`); the
///   index accepts any name, and the validator reports it.
/// - `BTCPP_format` and `main_tree_to_execute` are attributes of the root
///   element.
/// - The trees, includes and model sections are child elements of the root
///   element, in document order.
/// - A `<BehaviorTree>` without `ID` gets the ID `BehaviorTree_<n>`. BT.CPP
///   counts `n` from 0 over all such trees in one parser, also in included
///   files (the includes load first). The index counts in this file only,
///   so the name is exact for a file that is loaded alone. `VerifyXML`
///   accepts a tree without `ID` only if it is the only child element of
///   the root.
/// - BT.CPP keeps the last of two trees with the same ID (`tree_roots` is a
///   map). [`BtIndex::tree`] does the same.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BtIndex {
    /// The root element.
    pub root: Option<NodeId>,
    /// The value of `BTCPP_format`. BT.CPP 4 warns when it is missing.
    pub format: Option<String>,
    /// The value of `main_tree_to_execute`.
    pub main_tree_to_execute: Option<String>,
    /// The `<BehaviorTree>` elements, in document order.
    pub trees: Vec<TreeEntry>,
    /// The `<include>` elements, in document order.
    pub includes: Vec<IncludeEntry>,
    /// The `<TreeNodesModel>` elements, in document order. `VerifyXML`
    /// accepts only one.
    pub model_sections: Vec<NodeId>,
    /// The models of all model sections, with [`ModelSource::Unattached`].
    pub models: ParsedModels,
    /// Other child elements of the root element.
    pub other_elements: Vec<NodeId>,
}

impl BtIndex {
    /// Builds the index of a document. The function reads only the child
    /// elements of the root element and the content of the model sections.
    pub fn build(doc: &Document) -> BtIndex {
        let mut index = BtIndex::default();
        let Some(root) = doc.document_element() else {
            return index;
        };
        index.root = Some(root);
        if is_model_section(doc, root) {
            // A models file with <TreeNodesModel> as the root element.
            index.model_sections.push(root);
            parse_model_section(doc, root, ModelSource::Unattached, &mut index.models);
            return index;
        }
        index.format = doc.attribute(root, "BTCPP_format").map(str::to_owned);
        index.main_tree_to_execute = doc
            .attribute(root, "main_tree_to_execute")
            .map(str::to_owned);
        let mut unnamed = 0usize;
        for child in doc.element_children(root) {
            match doc.name(child) {
                Some("BehaviorTree") => {
                    let explicit = doc.attribute(child, "ID");
                    let id = match explicit {
                        Some(id) => id.to_owned(),
                        None => {
                            let id = format!("BehaviorTree_{unnamed}");
                            unnamed += 1;
                            id
                        }
                    };
                    let mut roots = doc.element_children(child);
                    let first = roots.next();
                    index.trees.push(TreeEntry {
                        id,
                        has_explicit_id: explicit.is_some(),
                        element: child,
                        root: first,
                        extra_roots: roots.collect(),
                    });
                }
                Some("include") => index.includes.push(IncludeEntry {
                    element: child,
                    path: doc.attribute(child, "path").map(str::to_owned),
                    ros_pkg: doc.attribute(child, "ros_pkg").map(str::to_owned),
                }),
                _ if is_model_section(doc, child) => {
                    index.model_sections.push(child);
                    parse_model_section(doc, child, ModelSource::Unattached, &mut index.models);
                }
                _ => index.other_elements.push(child),
            }
        }
        index
    }

    /// The tree with this ID. If two trees have the ID, this is the last one,
    /// as in BT.CPP.
    pub fn tree(&self, id: &str) -> Option<&TreeEntry> {
        self.trees.iter().rev().find(|t| t.id == id)
    }

    /// The position of [`BtIndex::tree`] in [`BtIndex::trees`].
    pub fn tree_index(&self, id: &str) -> Option<usize> {
        self.trees.iter().rposition(|t| t.id == id)
    }

    /// The position of the tree with this `<BehaviorTree>` element.
    pub fn tree_index_of_element(&self, element: NodeId) -> Option<usize> {
        self.trees.iter().position(|t| t.element == element)
    }

    /// The tree that BT.CPP runs when this file is loaded alone and no tree
    /// ID is given (`XMLParser::instantiateTree`): the tree named by
    /// `main_tree_to_execute`, else the only tree. With includes, BT.CPP
    /// also counts the trees of the included files; see
    /// [`crate::project::Project::main_tree`].
    pub fn main_tree(&self) -> Option<&TreeEntry> {
        match &self.main_tree_to_execute {
            Some(id) => self.tree(id),
            None if self.trees.len() == 1 => self.trees.first(),
            None => None,
        }
    }

    /// The first local model with this ID.
    pub fn model(&self, id: &str) -> Option<&ParsedModel> {
        self.models.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_parts() {
        let doc = Document::parse(
            r#"<?xml version="1.0"?>
<root BTCPP_format="4" main_tree_to_execute = "B">
  <include path="a.xml"/>
  <BehaviorTree ID="A"><Sequence><X/></Sequence></BehaviorTree>
  <include path="b.xml" ros_pkg="pkg"/>
  <BehaviorTree ID="B"><X/><!-- c --><Y/><Z/></BehaviorTree>
  <BehaviorTree/>
  <BehaviorTree ID=""/>
  <BehaviorTree><Q/></BehaviorTree>
  <TreeNodesModel><Action ID="X"/></TreeNodesModel>
  <TreeNodesModel><Action ID="Y"/></TreeNodesModel>
  <include/>
  <Other/>
</root>"#,
        )
        .unwrap();
        let index = BtIndex::build(&doc);
        assert_eq!(index.root, doc.document_element());
        assert_eq!(index.format.as_deref(), Some("4"));
        assert_eq!(index.main_tree_to_execute.as_deref(), Some("B"));
        let ids: Vec<&str> = index.trees.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B", "BehaviorTree_0", "", "BehaviorTree_1"]);
        let explicit: Vec<bool> = index.trees.iter().map(|t| t.has_explicit_id).collect();
        assert_eq!(explicit, vec![true, true, false, true, false]);
        assert_eq!(doc.name(index.trees[0].root.unwrap()), Some("Sequence"));
        assert!(index.trees[0].extra_roots.is_empty());
        assert_eq!(doc.name(index.trees[1].root.unwrap()), Some("X"));
        assert_eq!(index.trees[1].extra_roots.len(), 2);
        assert_eq!(index.trees[2].root, None);
        assert_eq!(index.includes.len(), 3);
        assert_eq!(index.includes[0].path.as_deref(), Some("a.xml"));
        assert_eq!(index.includes[0].ros_pkg, None);
        assert_eq!(index.includes[1].ros_pkg.as_deref(), Some("pkg"));
        assert_eq!(index.includes[2].path, None);
        assert_eq!(index.model_sections.len(), 2);
        assert_eq!(index.models.models.len(), 2);
        assert!(index.model("Y").is_some());
        assert_eq!(index.other_elements.len(), 1);
        assert_eq!(index.main_tree().unwrap().id, "B");
        assert_eq!(index.tree_index("A"), Some(0));
        assert_eq!(index.tree_index_of_element(index.trees[1].element), Some(1));
        assert_eq!(index.tree_index_of_element(doc.root()), None);
    }

    #[test]
    fn duplicate_ids_last_wins() {
        let doc = Document::parse(
            r#"<root><BehaviorTree ID="T"><A/></BehaviorTree><BehaviorTree ID="T"><B/></BehaviorTree></root>"#,
        )
        .unwrap();
        let index = BtIndex::build(&doc);
        assert_eq!(index.tree_index("T"), Some(1));
        assert_eq!(doc.name(index.tree("T").unwrap().root.unwrap()), Some("B"));
        // Two trees and no main_tree_to_execute: no main tree.
        assert!(index.main_tree().is_none());
    }

    #[test]
    fn single_tree_is_main() {
        let doc = Document::parse("<root><BehaviorTree><A/></BehaviorTree></root>").unwrap();
        let index = BtIndex::build(&doc);
        assert_eq!(index.main_tree().unwrap().id, "BehaviorTree_0");
        assert_eq!(index.format, None);
    }

    #[test]
    fn missing_main_tree() {
        let doc = Document::parse(
            r#"<root main_tree_to_execute="Nope"><BehaviorTree ID="A"><A/></BehaviorTree></root>"#,
        )
        .unwrap();
        assert!(BtIndex::build(&doc).main_tree().is_none());
    }

    #[test]
    fn no_root_element() {
        let doc = Document::parse("<!-- nothing -->").unwrap();
        assert_eq!(BtIndex::build(&doc), BtIndex::default());
    }
}
