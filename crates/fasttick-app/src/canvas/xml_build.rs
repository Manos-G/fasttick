//! Builds a [`CanvasTree`] from one tree of an editable project file.
//!
//! The canvas shows, in this order:
//!
//! 1. the root node of the tree and its descendants;
//! 2. more root elements, if the `<BehaviorTree>` has them (the validator
//!    reports them as an error);
//! 3. the unconnected nodes of the tree (see `docs/DESIGN.md` 2.3).
//!
//! Each node uses [`NodeSource::Xml`], so its identity stays the same over
//! edits. A `<SubTree>` node in the expansion set shows the root of its
//! target tree as its child. Those nodes belong to another tree; they have
//! [`CanvasNode::inline`] set and the editor treats them as read-only.

use std::collections::HashSet;

use fasttick_core::bt::NodeView;
use fasttick_core::models::NodeKind;
use fasttick_core::project::{FileId, Project};
use fasttick_core::validate::{Problem, Severity};
use fasttick_core::xml::NodeId;

use super::categories::CategoryLookup;
use super::model::{
    CanvasNode, CanvasNodeId, CanvasTree, CanvasTreeBuilder, Expansion, NodeCategory, NodeSource,
    PortLine, ProblemMarker, SubtreeInfo, condition_lines,
};

/// The deepest SubTree nesting that the builder expands.
const MAX_INLINE_NESTING: usize = 64;

/// Node categories from the model registry of a project.
#[derive(Debug, Clone, Copy)]
pub struct ProjectCategories<'a> {
    /// The project.
    pub project: &'a Project,
    /// The file whose look-up order applies.
    pub file: FileId,
}

impl CategoryLookup for ProjectCategories<'_> {
    fn category(&self, type_id: &str) -> Option<NodeCategory> {
        self.project
            .lookup_model(self.file, type_id)
            .map(|(model, _)| category_of_kind(model.kind))
    }

    fn description(&self, type_id: &str) -> Option<String> {
        self.project
            .lookup_model(self.file, type_id)
            .and_then(|(model, _)| model.description().map(str::to_owned))
    }
}

/// The canvas category of a model kind.
pub fn category_of_kind(kind: NodeKind) -> NodeCategory {
    match kind {
        NodeKind::Action => NodeCategory::Action,
        NodeKind::Condition => NodeCategory::Condition,
        NodeKind::Control => NodeCategory::Control,
        NodeKind::Decorator => NodeCategory::Decorator,
        NodeKind::SubTree => NodeCategory::SubTree,
    }
}

/// One step of the depth-first walk.
enum Step {
    /// Visit an element. `inline` is true inside an expanded SubTree.
    Visit {
        file: FileId,
        node: NodeId,
        parent: Option<CanvasNodeId>,
        inline: bool,
    },
    /// Leave an expanded SubTree: pop its tree from the ancestry.
    Leave,
}

impl CanvasTree {
    /// Builds the canvas tree of tree `tree_index` of `file`. See the
    /// [module documentation](self). An index that does not exist gives an
    /// empty tree.
    pub fn from_project(
        project: &Project,
        file: FileId,
        tree_index: usize,
        expansion: &Expansion,
    ) -> CanvasTree {
        let Some(bt_file) = project.file(file) else {
            return CanvasTree::default();
        };
        let Some(entry) = bt_file.index().trees.get(tree_index) else {
            return CanvasTree::default();
        };
        let title = entry.id.clone();
        let mut roots: Vec<NodeId> = entry.root.iter().copied().collect();
        roots.extend(entry.extra_roots.iter().copied());
        roots.extend(project.unconnected(file, &title).iter().map(|u| u.node));

        let mut builder = CanvasTreeBuilder::new();
        // The trees of the expanded SubTrees on the current path, to stop
        // cycles: (file, tree ID).
        let mut ancestry: Vec<(FileId, String)> = vec![(file, title.clone())];
        let mut stack: Vec<Step> = roots
            .iter()
            .rev()
            .map(|&node| Step::Visit {
                file,
                node,
                parent: None,
                inline: false,
            })
            .collect();
        while let Some(step) = stack.pop() {
            let (node_file, node_id, parent, inline) = match step {
                Step::Visit {
                    file,
                    node,
                    parent,
                    inline,
                } => (file, node, parent, inline),
                Step::Leave => {
                    ancestry.pop();
                    continue;
                }
            };
            let Some(doc) = project.file(node_file).map(|f| f.doc()) else {
                continue;
            };
            let Some(view) = NodeView::new(doc, node_id) else {
                continue;
            };
            let source = NodeSource::Xml(node_file, node_id);
            let type_id = view.type_id().unwrap_or_default();
            let label = view.instance_name().unwrap_or(type_id);
            let mut canvas_node = CanvasNode::new(source, label, type_id);
            canvas_node.inline = inline;
            canvas_node.ports = view
                .ports()
                .map(|(name, value)| PortLine {
                    name: name.to_owned(),
                    value: value.to_owned(),
                })
                .collect();
            canvas_node.conditions = condition_lines(
                view.pre_conditions()
                    .map(|(c, script)| (c.attribute_name(), script))
                    .chain(
                        view.post_conditions()
                            .map(|(c, script)| (c.attribute_name(), script)),
                    ),
            );
            let children: Vec<NodeId> = view.children().map(|child| child.id()).collect();
            let (category, inferred) = categorize(project, node_file, &view, children.len());
            canvas_node.category = category;
            canvas_node.category_inferred = inferred;
            let lookup = ProjectCategories {
                project,
                file: node_file,
            };
            canvas_node.description = match view.subtree_id() {
                // The model of the tree, else the text of SubTree.
                Some(tree_id) => lookup
                    .description(tree_id)
                    .or_else(|| lookup.description("SubTree")),
                None => view.type_id().and_then(|id| lookup.description(id)),
            };

            if view.is_subtree() {
                let tree_id = view.subtree_id().unwrap_or_default().to_owned();
                let target = project
                    .find_tree_from(node_file, &tree_id)
                    .and_then(|tree_ref| {
                        let entry = project.tree(tree_ref)?;
                        Some((tree_ref.file, entry.root?))
                    });
                let cycle = target.is_some_and(|(target_file, _)| {
                    ancestry
                        .iter()
                        .any(|(f, id)| *f == target_file && *id == tree_id)
                }) || ancestry.len() > MAX_INLINE_NESTING;
                let can_expand = target.is_some() && !cycle;
                let expanded = can_expand && expansion.is_expanded(source);
                let inner_count = target
                    .and_then(|(target_file, root)| {
                        let doc = project.file(target_file)?.doc();
                        Some(doc.descendants(root).filter(|n| doc.is_element(*n)).count())
                    })
                    .unwrap_or(0);
                canvas_node.subtree = Some(SubtreeInfo {
                    tree_id: tree_id.clone(),
                    expanded,
                    can_expand,
                    inner_count,
                });
                let id = builder.push(parent, canvas_node);
                if expanded && let Some((target_file, root)) = target {
                    ancestry.push((target_file, tree_id));
                    stack.push(Step::Leave);
                    stack.push(Step::Visit {
                        file: target_file,
                        node: root,
                        parent: Some(id),
                        inline: true,
                    });
                }
                // A SubTree element has no children in valid XML; show any
                // that it has, so that nothing is hidden.
                for child in children.iter().rev() {
                    stack.push(Step::Visit {
                        file: node_file,
                        node: *child,
                        parent: Some(id),
                        inline,
                    });
                }
            } else {
                let id = builder.push(parent, canvas_node);
                for child in children.iter().rev() {
                    stack.push(Step::Visit {
                        file: node_file,
                        node: *child,
                        parent: Some(id),
                        inline,
                    });
                }
            }
        }
        builder.finish(title)
    }

    /// Marks the nodes that have problems. A node keeps its most severe
    /// problem. Problems of other files and problems without a node are
    /// ignored.
    pub fn apply_problems(&mut self, problems: &[Problem]) {
        for node in self.nodes_mut() {
            node.deco.problem = None;
        }
        for problem in problems {
            let Some(node) = problem.node else {
                continue;
            };
            let Some(id) = self.find_source(NodeSource::Xml(problem.file, node)) else {
                continue;
            };
            let marker = match problem.severity {
                Severity::Error => ProblemMarker::Error,
                Severity::Warning => ProblemMarker::Warning,
                _ => continue,
            };
            if let Some(canvas_node) = self.get_mut(id) {
                let current = canvas_node.deco.problem;
                canvas_node.deco.problem = Some(current.map_or(marker, |c| c.max(marker)));
            }
        }
    }

    /// All SubTree nodes of a project, as expansion sources (for "expand
    /// all").
    pub fn all_project_subtrees(project: &Project) -> Expansion {
        let mut expansion = Expansion::none();
        let mut seen = HashSet::new();
        for (file_id, _, file) in project.files() {
            let doc = file.doc();
            for entry in &file.index().trees {
                let Some(root) = entry.root else { continue };
                for node in doc.descendants(root) {
                    if NodeView::new(doc, node).is_some_and(|v| v.is_subtree())
                        && seen.insert((file_id, node))
                    {
                        expansion.set(NodeSource::Xml(file_id, node), true);
                    }
                }
            }
        }
        expansion
    }
}

/// The category of an element and true if it was inferred.
fn categorize(
    project: &Project,
    file: FileId,
    view: &NodeView<'_>,
    child_count: usize,
) -> (NodeCategory, bool) {
    if view.is_subtree() {
        return (NodeCategory::SubTree, false);
    }
    if view.is_explicit_form()
        && let Some(kind) = view.category_tag()
    {
        return (category_of_kind(kind), false);
    }
    let lookup = ProjectCategories { project, file };
    if let Some(category) = view.type_id().and_then(|id| lookup.category(id)) {
        return (category, false);
    }
    let guess = match child_count {
        0 => NodeCategory::Action,
        1 => NodeCategory::Decorator,
        _ => NodeCategory::Control,
    };
    (guess, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fasttick_core::bt::BtFile;

    const XML: &str = r#"<root BTCPP_format="4" main_tree_to_execute="Main">
    <BehaviorTree ID="Main">
        <Sequence name="seq">
            <MyAction name="act" goal="{g}"/>
            <SubTree ID="Sub" goal="{g}"/>
            <SubTree ID="Main"/>
        </Sequence>
    </BehaviorTree>
    <BehaviorTree ID="Sub">
        <Inverter><AlwaysFailure/></Inverter>
    </BehaviorTree>
    <TreeNodesModel>
        <Condition ID="MyAction">
            <input_port name="goal"/>
            <MetadataFields><Metadata description="Checks the goal"/></MetadataFields>
        </Condition>
    </TreeNodesModel>
</root>"#;

    fn project() -> (Project, FileId) {
        let file = BtFile::parse(XML, None).unwrap();
        Project::with_file("t", file)
    }

    #[test]
    fn builds_the_tree_with_models_and_subtrees() {
        let (project, file) = project();
        let tree = CanvasTree::from_project(&project, file, 0, &Expansion::none());
        assert_eq!(tree.title, "Main");
        assert_eq!(tree.len(), 4);
        let nodes = tree.nodes();
        assert_eq!(nodes[0].label, "seq");
        assert_eq!(nodes[0].category, NodeCategory::Control);
        assert!(!nodes[0].category_inferred);
        assert_eq!(nodes[1].label, "act");
        // The local model says MyAction is a condition.
        assert_eq!(nodes[1].category, NodeCategory::Condition);
        assert_eq!(nodes[1].ports[0].name, "goal");
        let sub = nodes[2].subtree.as_ref().unwrap();
        assert_eq!(sub.tree_id, "Sub");
        assert!(sub.can_expand && !sub.expanded);
        assert_eq!(sub.inner_count, 2);
        // A SubTree to the tree itself is a cycle: it cannot expand.
        assert!(!nodes[3].subtree.as_ref().unwrap().can_expand);
        assert!(matches!(nodes[0].source, NodeSource::Xml(f, _) if f == file));
        // Descriptions: the built-in text, the metadata of a model, and the
        // text of SubTree for a tree without a model.
        assert!(
            nodes[0]
                .description
                .as_deref()
                .unwrap()
                .starts_with("Ticks its children")
        );
        assert_eq!(nodes[1].description.as_deref(), Some("Checks the goal"));
        assert!(
            nodes[2]
                .description
                .as_deref()
                .unwrap()
                .starts_with("Runs another tree")
        );
    }

    #[test]
    fn expanded_subtree_nodes_are_inline() {
        let (project, file) = project();
        let base = CanvasTree::from_project(&project, file, 0, &Expansion::none());
        let mut expansion = Expansion::none();
        expansion.set(base.nodes()[2].source, true);
        let tree = CanvasTree::from_project(&project, file, 0, &expansion);
        assert_eq!(tree.len(), 6);
        let inline: Vec<bool> = tree.nodes().iter().map(|n| n.inline).collect();
        assert_eq!(inline, vec![false, false, false, true, true, false]);
        assert_eq!(tree.nodes()[3].type_id, "Inverter");
        // "Expand all" finds both SubTree nodes.
        assert_eq!(CanvasTree::all_project_subtrees(&project).len(), 2);
    }

    #[test]
    fn unconnected_nodes_are_extra_roots() {
        let (mut project, file) = project();
        let node = {
            let bt = project.file_mut(file).unwrap();
            let nodes = bt.doc_mut().parse_fragment("<Sleep msec=\"1\"/>").unwrap();
            nodes[0]
        };
        project.add_unconnected(file, "Main", node, [0.0, 0.0]);
        let tree = CanvasTree::from_project(&project, file, 0, &Expansion::none());
        assert_eq!(tree.len(), 5);
        let last = tree.nodes().last().unwrap();
        assert_eq!(last.type_id, "Sleep");
        assert!(last.parent.is_none());
    }

    #[test]
    fn conditions_are_in_bt_cpp_order() {
        let xml = r#"<root BTCPP_format="4"><BehaviorTree ID="Main">
            <Sleep msec="1" _post="a:=1" _skipIf="x == 1" _onSuccess="b:=2" _failureIf="y"/>
        </BehaviorTree></root>"#;
        let (project, file) = Project::with_file("t", BtFile::parse(xml, None).unwrap());
        let tree = CanvasTree::from_project(&project, file, 0, &Expansion::none());
        let names: Vec<&str> = tree.nodes()[0]
            .conditions
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["_failureIf", "_skipIf", "_onSuccess", "_post"]);
        assert_eq!(tree.nodes()[0].conditions[1].value, "x == 1");
        // Conditions are not ports.
        assert_eq!(tree.nodes()[0].ports.len(), 1);
    }

    #[test]
    fn bad_indices_give_an_empty_tree() {
        let (project, file) = project();
        assert!(CanvasTree::from_project(&project, file, 9, &Expansion::none()).is_empty());
        assert!(CanvasTree::from_project(&project, FileId(7), 0, &Expansion::none()).is_empty());
    }

    #[test]
    fn problems_mark_nodes() {
        let xml = r#"<root BTCPP_format="4"><BehaviorTree ID="T"><Sequence><Unknown1/></Sequence></BehaviorTree></root>"#;
        let (project, file) = Project::with_file("t", BtFile::parse(xml, None).unwrap());
        let problems = fasttick_core::validate::validate_project(&project);
        let mut tree = CanvasTree::from_project(&project, file, 0, &Expansion::none());
        tree.apply_problems(&problems);
        assert_eq!(tree.nodes()[1].deco.problem, Some(ProblemMarker::Error));
    }
}
