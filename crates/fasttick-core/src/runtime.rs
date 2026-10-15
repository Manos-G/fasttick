//! The runtime tree: the flat XML that a running executor describes.
//!
//! `BT::WriteTreeToXML(tree, true, true)` writes this form. The publisher
//! sends it as the FULLTREE reply, and `FileLogger2` writes it at the start
//! of each `.btlog` file. It differs from an authoring file:
//!
//! - There is one `<BehaviorTree>` element for each SubTree *instance*.
//!   The attribute `_fullpath` holds the instance path (empty for the root).
//! - Each node has a `_uid` attribute (a `u16`).
//! - A `<SubTree>` node has `ID` and `_fullpath`, but no children and no
//!   `name`. Its `_fullpath` is equal to the `_fullpath` of the
//!   `<BehaviorTree>` element of the instance.
//! - A `<TreeNodesModel>` element follows the trees.
//!
//! [`RuntimeTree::parse`] accepts this form. It also accepts trees without
//! `_uid` attributes (the UID is then absent), so that it can show a plain
//! authoring file too.

use std::collections::HashMap;
use std::fmt;

use quick_xml::events::{BytesStart, Event};

/// The UID of a node in a running tree (`TreeNode::UID()` in BT.CPP).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct Uid(pub u16);

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Index of a node in [`RuntimeTree::nodes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeIndex(pub usize);

/// Index of an instance in [`RuntimeTree::instances`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceIndex(pub usize);

/// Error from [`RuntimeTree::parse`].
#[derive(Debug, thiserror::Error)]
pub enum RuntimeTreeError {
    /// The text is not well-formed XML.
    #[error("XML error at byte {position}: {message}")]
    Xml {
        /// Byte offset in the input.
        position: u64,
        /// Description from the XML parser.
        message: String,
    },
    /// The XML has no `<BehaviorTree>` element.
    #[error("the XML contains no <BehaviorTree> element")]
    NoTree,
    /// A `_uid` attribute is not a number from 0 to 65535.
    #[error("invalid _uid value {0:?}")]
    InvalidUid(String),
    /// Two nodes have the same UID.
    #[error("duplicate _uid {0}")]
    DuplicateUid(u16),
    /// A `<BehaviorTree>` element contains more than one root node.
    #[error("<BehaviorTree> {0:?} has more than one root node")]
    MultipleRoots(String),
}

/// One node of the runtime tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeNode {
    /// The UID, from the `_uid` attribute.
    pub uid: Option<Uid>,
    /// The element name, which is the registration ID (for example `Sequence`
    /// or `SubTree`). For the explicit form (`<Action ID="X">`) this is the
    /// value of `ID`.
    pub type_id: String,
    /// The element name as written in the XML.
    pub tag: String,
    /// The instance name: the `name` attribute, or for a SubTree the last
    /// part of `_fullpath`, or else the type ID.
    pub name: String,
    /// All attributes, in document order, with escapes resolved.
    pub attributes: Vec<(String, String)>,
    /// The parent node, if any.
    pub parent: Option<NodeIndex>,
    /// The children, in document order.
    pub children: Vec<NodeIndex>,
    /// The SubTree instance that contains this node.
    pub instance: InstanceIndex,
    /// For a `<SubTree>` node: the instance that it expands into.
    pub expands_to: Option<InstanceIndex>,
}

impl RuntimeNode {
    /// The value of an attribute.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// True if this is a `<SubTree>` node.
    pub fn is_subtree(&self) -> bool {
        self.type_id == "SubTree"
    }

    /// Attributes that are ports or remappings: all attributes except `name`,
    /// `ID`, and the attributes that start with `_` (metadata, `_autoremap`
    /// and the pre/post-condition scripts).
    pub fn port_attributes(&self) -> impl Iterator<Item = (&str, &str)> {
        self.attributes
            .iter()
            .filter(|(key, _)| key != "name" && key != "ID" && !key.starts_with('_'))
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

/// One SubTree instance: one `<BehaviorTree>` element of the runtime XML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreeInstance {
    /// The tree ID (the `ID` attribute).
    pub tree_id: String,
    /// The instance path (`_fullpath`). Empty for the root instance.
    pub fullpath: String,
    /// The root node of the instance.
    pub root: Option<NodeIndex>,
    /// The `<SubTree>` node in the parent instance that expands into this
    /// instance. `None` for the root instance.
    pub parent_node: Option<NodeIndex>,
}

impl SubtreeInstance {
    /// The name that the publisher uses for the blackboard of this instance:
    /// the instance path, or the tree ID when the path is empty.
    pub fn blackboard_name(&self) -> &str {
        if self.fullpath.is_empty() {
            &self.tree_id
        } else {
            &self.fullpath
        }
    }
}

/// A tree as a running executor describes it. See the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTree {
    /// The XML text that was parsed.
    pub xml: String,
    /// The SubTree instances, in document order. Index 0 is the root.
    pub instances: Vec<SubtreeInstance>,
    /// All nodes of all instances.
    pub nodes: Vec<RuntimeNode>,
    by_uid: HashMap<u16, NodeIndex>,
}

impl RuntimeTree {
    /// Parses the runtime XML.
    pub fn parse(xml: &str) -> Result<RuntimeTree, RuntimeTreeError> {
        let mut reader = quick_xml::Reader::from_str(xml);
        let mut instances: Vec<SubtreeInstance> = Vec::new();
        let mut nodes: Vec<RuntimeNode> = Vec::new();
        let mut by_uid: HashMap<u16, NodeIndex> = HashMap::new();

        // Element nesting: what each open element is.
        enum Open {
            Other,
            Tree(InstanceIndex),
            Node(NodeIndex),
        }
        let mut stack: Vec<Open> = Vec::new();
        // Depth of the open <TreeNodesModel> element, if any.
        let mut skip_depth: Option<usize> = None;

        loop {
            let position = reader.buffer_position();
            let event = reader.read_event().map_err(|err| RuntimeTreeError::Xml {
                position,
                message: err.to_string(),
            })?;
            let (start, is_empty) = match event {
                Event::Start(start) => (start, false),
                Event::Empty(start) => (start, true),
                Event::End(_) => {
                    stack.pop();
                    if skip_depth.is_some_and(|depth| stack.len() < depth) {
                        skip_depth = None;
                    }
                    continue;
                }
                Event::Eof => break,
                _ => continue,
            };

            let tag = start.name().as_ref().to_owned();
            let attributes = read_attributes(&start, reader.buffer_position())?;
            let parent = stack.last();

            let open = if skip_depth.is_some() {
                Open::Other
            } else {
                match parent {
                    // The document element (<root>) or unknown nesting.
                    None | Some(Open::Other) => {
                        if tag == "BehaviorTree" {
                            let tree_id = find(&attributes, "ID").unwrap_or_default().to_owned();
                            let fullpath = find(&attributes, "_fullpath")
                                .unwrap_or_default()
                                .to_owned();
                            instances.push(SubtreeInstance {
                                tree_id,
                                fullpath,
                                root: None,
                                parent_node: None,
                            });
                            Open::Tree(InstanceIndex(instances.len() - 1))
                        } else {
                            if tag == "TreeNodesModel" {
                                skip_depth = Some(stack.len() + 1);
                            }
                            Open::Other
                        }
                    }
                    Some(Open::Tree(instance)) => {
                        let instance = *instance;
                        let index =
                            push_node(&mut nodes, &mut by_uid, tag, attributes, None, instance)?;
                        let slot = &mut instances[instance.0].root;
                        if slot.is_some() {
                            return Err(RuntimeTreeError::MultipleRoots(
                                instances[instance.0].tree_id.clone(),
                            ));
                        }
                        *slot = Some(index);
                        Open::Node(index)
                    }
                    Some(Open::Node(parent_index)) => {
                        let parent_index = *parent_index;
                        let instance = nodes[parent_index.0].instance;
                        let index = push_node(
                            &mut nodes,
                            &mut by_uid,
                            tag,
                            attributes,
                            Some(parent_index),
                            instance,
                        )?;
                        nodes[parent_index.0].children.push(index);
                        Open::Node(index)
                    }
                }
            };
            if !is_empty {
                stack.push(open);
            } else if skip_depth.is_some_and(|depth| stack.len() + 1 == depth) {
                // An empty <TreeNodesModel/>.
                skip_depth = None;
            }
        }

        if instances.is_empty() {
            return Err(RuntimeTreeError::NoTree);
        }

        link_subtrees(&mut nodes, &mut instances);

        Ok(RuntimeTree {
            xml: xml.to_owned(),
            instances,
            nodes,
            by_uid,
        })
    }

    /// The node with this UID.
    pub fn node_by_uid(&self, uid: Uid) -> Option<&RuntimeNode> {
        self.index_of_uid(uid).map(|index| &self.nodes[index.0])
    }

    /// The index of the node with this UID.
    pub fn index_of_uid(&self, uid: Uid) -> Option<NodeIndex> {
        self.by_uid.get(&uid.0).copied()
    }

    /// The node at this index.
    pub fn node(&self, index: NodeIndex) -> &RuntimeNode {
        &self.nodes[index.0]
    }

    /// The root instance (the main tree).
    pub fn root_instance(&self) -> &SubtreeInstance {
        &self.instances[0]
    }

    /// The root node of the main tree.
    pub fn root(&self) -> Option<NodeIndex> {
        self.instances[0].root
    }

    /// The number of nodes in all instances.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True if the tree has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The full path of a node: the instance path, `/`, and the node name.
    ///
    /// This is a display path. It is not always equal to the path of BT.CPP
    /// (`TreeNode::fullPath()`): for a node whose instance name is its type
    /// ID, BT.CPP adds `::<uid>`. Use [`RuntimeTree::full_path`] for the
    /// exact BT.CPP path.
    pub fn node_path(&self, index: NodeIndex) -> String {
        let node = &self.nodes[index.0];
        if node.is_subtree()
            && let Some(fullpath) = node.attribute("_fullpath")
        {
            return fullpath.to_owned();
        }
        let instance = &self.instances[node.instance.0];
        if instance.fullpath.is_empty() {
            node.name.clone()
        } else {
            format!("{}/{}", instance.fullpath, node.name)
        }
    }

    /// The full path of a node exactly as BT.CPP gives it
    /// (`TreeNode::fullPath()`, the value of `NodeConfig::path`). The
    /// `SqliteLogger` writes it in the `Nodes` table, and the substitution
    /// rules of `BehaviorTreeFactory` match it.
    ///
    /// `createNodeFromXML` (`src/xml_parsing.cpp`) makes the path:
    ///
    /// - the prefix of the SubTree instance: empty for the main tree, else
    ///   the instance path (`_fullpath` of the `<BehaviorTree>` element) and
    ///   `/`;
    /// - the instance name: the `name` attribute, else the type ID (for a
    ///   `<SubTree>`: its `ID`, the tree ID);
    /// - if the instance name is equal to the type ID: `::` and the UID.
    ///   This also applies when a `name` attribute has the type ID as value.
    ///
    /// For a `<SubTree>` node, `WriteTreeToXML` writes this path in the
    /// `_fullpath` attribute, so the function returns that attribute.
    ///
    /// Differences from [`RuntimeTree::node_path`]: `node_path` never adds
    /// `::<uid>`, and it uses the type ID for an empty `name`. For example,
    /// for `<CheckFlag/>` with UID 10 in the instance `navigate`,
    /// `full_path` gives `navigate/CheckFlag::10` and `node_path` gives
    /// `navigate/CheckFlag`. A node without a UID (an authoring file, not
    /// runtime XML) gets no `::<uid>` part.
    pub fn full_path(&self, index: NodeIndex) -> String {
        let node = &self.nodes[index.0];
        if node.is_subtree()
            && let Some(fullpath) = node.attribute("_fullpath")
        {
            return fullpath.to_owned();
        }
        let type_id = if node.is_subtree() {
            node.attribute("ID").unwrap_or("SubTree")
        } else {
            node.type_id.as_str()
        };
        let instance_name = node.attribute("name").unwrap_or(type_id);
        let instance = &self.instances[node.instance.0];
        let mut path = String::with_capacity(instance.fullpath.len() + instance_name.len() + 8);
        if !instance.fullpath.is_empty() {
            path.push_str(&instance.fullpath);
            path.push('/');
        }
        path.push_str(instance_name);
        if instance_name == type_id
            && let Some(uid) = node.uid
        {
            path.push_str("::");
            path.push_str(&uid.to_string());
        }
        path
    }

    /// The expanded children of a node: for a `<SubTree>` node, the root of
    /// the instance that it expands into; else the XML children.
    pub fn expanded_children(&self, index: NodeIndex) -> Vec<NodeIndex> {
        let node = &self.nodes[index.0];
        match node
            .expands_to
            .and_then(|instance| self.instances[instance.0].root)
        {
            Some(root) => vec![root],
            None => node.children.clone(),
        }
    }

    /// All nodes of the expanded tree, depth first, starting at the main root.
    /// Each item is the node index and its depth.
    pub fn walk_expanded(&self) -> Vec<(NodeIndex, usize)> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<(NodeIndex, usize)> =
            self.root().map(|root| (root, 0)).into_iter().collect();
        while let Some((index, depth)) = stack.pop() {
            out.push((index, depth));
            for child in self.expanded_children(index).into_iter().rev() {
                stack.push((child, depth + 1));
            }
        }
        out
    }
}

fn find<'a>(attributes: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn read_attributes(
    start: &BytesStart<'_>,
    position: u64,
) -> Result<Vec<(String, String)>, RuntimeTreeError> {
    let mut out = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|err| RuntimeTreeError::Xml {
            position,
            message: err.to_string(),
        })?;
        let key = attribute.key.as_ref().to_owned();
        let raw = &attribute.value;
        // Resolve escapes but keep newlines, as tinyxml2 (used by BT.CPP) does.
        let value = quick_xml::escape::unescape(raw)
            .map_err(|err| RuntimeTreeError::Xml {
                position,
                message: err.to_string(),
            })?
            .replace("\r\n", "\n");
        out.push((key, value));
    }
    Ok(out)
}

fn push_node(
    nodes: &mut Vec<RuntimeNode>,
    by_uid: &mut HashMap<u16, NodeIndex>,
    tag: String,
    attributes: Vec<(String, String)>,
    parent: Option<NodeIndex>,
    instance: InstanceIndex,
) -> Result<NodeIndex, RuntimeTreeError> {
    let index = NodeIndex(nodes.len());
    let uid = match find(&attributes, "_uid") {
        Some(text) => {
            let value: u16 = text
                .trim()
                .parse()
                .map_err(|_| RuntimeTreeError::InvalidUid(text.to_owned()))?;
            if by_uid.insert(value, index).is_some() {
                return Err(RuntimeTreeError::DuplicateUid(value));
            }
            Some(Uid(value))
        }
        None => None,
    };
    let is_explicit = matches!(
        tag.as_str(),
        "Action" | "Condition" | "Control" | "Decorator"
    );
    let type_id = match (is_explicit, find(&attributes, "ID")) {
        (true, Some(id)) => id.to_owned(),
        _ => tag.clone(),
    };
    let name = match find(&attributes, "name") {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ if type_id == "SubTree" => find(&attributes, "_fullpath")
            .and_then(|path| path.rsplit('/').next())
            .filter(|last| !last.is_empty())
            .or_else(|| find(&attributes, "ID"))
            .unwrap_or("SubTree")
            .to_owned(),
        _ => type_id.clone(),
    };
    nodes.push(RuntimeNode {
        uid,
        type_id,
        tag,
        name,
        attributes,
        parent,
        children: Vec::new(),
        instance,
        expands_to: None,
    });
    Ok(index)
}

/// Links each `<SubTree>` node to the instance that it expands into.
///
/// The link uses `_fullpath`. If a SubTree node has no usable `_fullpath`,
/// the link uses the order of the instances: BT.CPP creates the instances
/// depth first, in the order in which it reaches the SubTree nodes of the
/// expanded tree.
fn link_subtrees(nodes: &mut [RuntimeNode], instances: &mut [SubtreeInstance]) {
    let mut by_path: HashMap<String, InstanceIndex> = HashMap::new();
    for (index, instance) in instances.iter().enumerate().skip(1) {
        by_path
            .entry(instance.fullpath.clone())
            .or_insert(InstanceIndex(index));
    }
    let mut used = vec![false; instances.len()];
    used[0] = true;
    for (index, node) in nodes.iter_mut().enumerate() {
        if !node.is_subtree() {
            continue;
        }
        let Some(instance) = node
            .attribute("_fullpath")
            .and_then(|path| by_path.get(path))
            .copied()
        else {
            continue;
        };
        if !used[instance.0] {
            used[instance.0] = true;
            node.expands_to = Some(instance);
            instances[instance.0].parent_node = Some(NodeIndex(index));
        }
    }

    // Fallback by order: walk the expanded tree depth first.
    let mut next_free = 1;
    let mut visited = vec![false; instances.len()];
    visited[0] = true;
    let mut stack: Vec<NodeIndex> = instances[0].root.into_iter().collect();
    while let Some(index) = stack.pop() {
        if nodes[index.0].is_subtree() {
            let target = match nodes[index.0].expands_to {
                Some(instance) => Some(instance),
                None => {
                    while next_free < instances.len() && used[next_free] {
                        next_free += 1;
                    }
                    if next_free < instances.len() {
                        let instance = InstanceIndex(next_free);
                        used[next_free] = true;
                        nodes[index.0].expands_to = Some(instance);
                        instances[instance.0].parent_node = Some(index);
                        Some(instance)
                    } else {
                        None
                    }
                }
            };
            if let Some(instance) = target
                && !visited[instance.0]
            {
                visited[instance.0] = true;
                stack.extend(instances[instance.0].root);
            }
        } else {
            stack.extend(nodes[index.0].children.iter().rev().copied());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runtime XML in the form that `WriteTreeToXML(tree, true, true)` writes.
    const SAMPLE: &str = r#"<root BTCPP_format="4">
    <BehaviorTree ID="MainTree" _fullpath="">
        <Sequence name="root_sequence" _uid="1">
            <SaySomething name="hello" _uid="2" message="hello &amp; welcome"/>
            <SubTree ID="Sub" _fullpath="sub_a" _uid="3" target="{goal}"/>
            <SubTree ID="Sub" _fullpath="Sub::4" _uid="4"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Sub" _fullpath="sub_a">
        <Fallback name="fb" _uid="5" _skipIf="x == 1">
            <AlwaysFailure name="AlwaysFailure" _uid="6"/>
        </Fallback>
    </BehaviorTree>
    <BehaviorTree ID="Sub" _fullpath="Sub::4">
        <Fallback name="fb" _uid="7">
            <AlwaysFailure name="AlwaysFailure" _uid="8"/>
        </Fallback>
    </BehaviorTree>
    <TreeNodesModel>
        <Action ID="SaySomething">
            <input_port name="message" type="std::string"/>
        </Action>
    </TreeNodesModel>
</root>"#;

    #[test]
    fn parses_instances_and_uids() {
        let tree = RuntimeTree::parse(SAMPLE).unwrap();
        assert_eq!(tree.instances.len(), 3);
        assert_eq!(tree.len(), 8);
        assert_eq!(tree.root_instance().tree_id, "MainTree");
        assert_eq!(tree.root_instance().blackboard_name(), "MainTree");
        assert_eq!(tree.instances[1].blackboard_name(), "sub_a");

        let hello = tree.node_by_uid(Uid(2)).unwrap();
        assert_eq!(hello.name, "hello");
        assert_eq!(hello.type_id, "SaySomething");
        assert_eq!(hello.attribute("message"), Some("hello & welcome"));
        assert_eq!(
            hello.port_attributes().collect::<Vec<_>>(),
            vec![("message", "hello & welcome")]
        );

        let sub = tree.node_by_uid(Uid(3)).unwrap();
        assert!(sub.is_subtree());
        assert_eq!(sub.name, "sub_a");
        assert_eq!(sub.expands_to, Some(InstanceIndex(1)));
        let sub2 = tree.node_by_uid(Uid(4)).unwrap();
        assert_eq!(sub2.name, "Sub::4");
        assert_eq!(sub2.expands_to, Some(InstanceIndex(2)));
        assert_eq!(tree.instances[2].parent_node, tree.index_of_uid(Uid(4)));
    }

    #[test]
    fn model_section_is_not_a_tree() {
        let tree = RuntimeTree::parse(SAMPLE).unwrap();
        assert!(tree.nodes.iter().all(|node| node.type_id != "input_port"));
    }

    #[test]
    fn walk_expanded_descends_into_subtrees() {
        let tree = RuntimeTree::parse(SAMPLE).unwrap();
        let uids: Vec<u16> = tree
            .walk_expanded()
            .into_iter()
            .map(|(index, _)| tree.node(index).uid.unwrap().0)
            .collect();
        assert_eq!(uids, vec![1, 2, 3, 5, 6, 4, 7, 8]);
        let depths: Vec<usize> = tree
            .walk_expanded()
            .into_iter()
            .map(|(_, depth)| depth)
            .collect();
        assert_eq!(depths, vec![0, 1, 1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn node_paths() {
        let tree = RuntimeTree::parse(SAMPLE).unwrap();
        assert_eq!(tree.node_path(tree.index_of_uid(Uid(2)).unwrap()), "hello");
        assert_eq!(tree.node_path(tree.index_of_uid(Uid(3)).unwrap()), "sub_a");
        assert_eq!(
            tree.node_path(tree.index_of_uid(Uid(5)).unwrap()),
            "sub_a/fb"
        );
    }

    #[test]
    fn full_paths_follow_btcpp() {
        let tree = RuntimeTree::parse(SAMPLE).unwrap();
        let path = |uid: u16| tree.full_path(tree.index_of_uid(Uid(uid)).unwrap());
        assert_eq!(path(1), "root_sequence");
        assert_eq!(path(2), "hello");
        assert_eq!(path(3), "sub_a");
        assert_eq!(path(4), "Sub::4");
        assert_eq!(path(5), "sub_a/fb");
        // The name is the type ID: BT.CPP adds "::<uid>".
        assert_eq!(path(6), "sub_a/AlwaysFailure::6");
        assert_eq!(path(7), "Sub::4/fb");
        assert_eq!(path(8), "Sub::4/AlwaysFailure::8");
        // node_path differs for these nodes.
        assert_eq!(
            tree.node_path(tree.index_of_uid(Uid(6)).unwrap()),
            "sub_a/AlwaysFailure"
        );
    }

    #[test]
    fn full_paths_without_runtime_attributes() {
        let xml = r#"<root>
            <BehaviorTree ID="T"><Sequence name="">
                <Action ID="Talk" _uid="7"/>
                <Action ID="Talk" name="Talk"/>
                <SubTree ID="Sub" _uid="9"/>
                <SubTree ID="Sub" name="named"/>
            </Sequence></BehaviorTree>
        </root>"#;
        let tree = RuntimeTree::parse(xml).unwrap();
        let paths: Vec<String> = (0..tree.len())
            .map(|i| tree.full_path(NodeIndex(i)))
            .collect();
        // An empty name stays empty (BT.CPP does not use the type ID then).
        assert_eq!(paths, vec!["", "Talk::7", "Talk", "Sub::9", "named"]);
    }

    #[test]
    fn explicit_form_uses_id() {
        let xml = r#"<root><BehaviorTree ID="T"><Action ID="Talk" name="t" _uid="1"/></BehaviorTree></root>"#;
        let tree = RuntimeTree::parse(xml).unwrap();
        assert_eq!(tree.nodes[0].type_id, "Talk");
        assert_eq!(tree.nodes[0].tag, "Action");
    }

    #[test]
    fn fallback_link_by_order_without_fullpath() {
        let xml = r#"<root>
            <BehaviorTree ID="Main"><Sequence _uid="1"><SubTree ID="A" _uid="2"/></Sequence></BehaviorTree>
            <BehaviorTree ID="A"><AlwaysSuccess _uid="3"/></BehaviorTree>
        </root>"#;
        let tree = RuntimeTree::parse(xml).unwrap();
        assert_eq!(
            tree.node_by_uid(Uid(2)).unwrap().expands_to,
            Some(InstanceIndex(1))
        );
    }

    #[test]
    fn fallback_link_follows_creation_order() {
        // Creation order in BT.CPP: Main, A (first SubTree), C (inside A), B.
        let xml = r#"<root>
            <BehaviorTree ID="Main"><Sequence _uid="1"><SubTree ID="A" _uid="2"/><SubTree ID="B" _uid="3"/></Sequence></BehaviorTree>
            <BehaviorTree ID="A"><Sequence _uid="4"><SubTree ID="C" _uid="5"/></Sequence></BehaviorTree>
            <BehaviorTree ID="C"><AlwaysSuccess _uid="6"/></BehaviorTree>
            <BehaviorTree ID="B"><AlwaysSuccess _uid="7"/></BehaviorTree>
        </root>"#;
        let tree = RuntimeTree::parse(xml).unwrap();
        assert_eq!(
            tree.node_by_uid(Uid(2)).unwrap().expands_to,
            Some(InstanceIndex(1))
        );
        assert_eq!(
            tree.node_by_uid(Uid(5)).unwrap().expands_to,
            Some(InstanceIndex(2))
        );
        assert_eq!(
            tree.node_by_uid(Uid(3)).unwrap().expands_to,
            Some(InstanceIndex(3))
        );
    }

    #[test]
    fn errors() {
        assert!(matches!(
            RuntimeTree::parse("<root/>"),
            Err(RuntimeTreeError::NoTree)
        ));
        assert!(matches!(
            RuntimeTree::parse(r#"<root><BehaviorTree ID="T"><A _uid="x"/></BehaviorTree></root>"#),
            Err(RuntimeTreeError::InvalidUid(_))
        ));
        assert!(matches!(
            RuntimeTree::parse(
                r#"<root><BehaviorTree ID="T"><A _uid="1"><B _uid="1"/></A></BehaviorTree></root>"#
            ),
            Err(RuntimeTreeError::DuplicateUid(1))
        ));
        assert!(matches!(
            RuntimeTree::parse(r#"<root><BehaviorTree ID="T"><A/><B/></BehaviorTree></root>"#),
            Err(RuntimeTreeError::MultipleRoots(_))
        ));
        assert!(matches!(
            RuntimeTree::parse("<root><BehaviorTree"),
            Err(RuntimeTreeError::Xml { .. })
        ));
    }
}
