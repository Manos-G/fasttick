//! The tree list: the trees of the open file (edit mode), or the SubTree
//! instances of a running tree (live and replay modes).

use egui::{RichText, Ui};
use fasttick_core::runtime::{InstanceIndex, RuntimeTree};

use super::PanelContext;
use crate::actions::AppAction;
use crate::canvas::SubtreeLinking;

/// One row of the tree list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    /// The instance.
    pub instance: InstanceIndex,
    /// The tree ID.
    pub tree_id: String,
    /// The instance path (empty for the root and in authoring files).
    pub path: String,
    /// The indent level (SubTree depth of the instance).
    pub depth: usize,
}

/// The rows for a document. Authoring files list each `<BehaviorTree>`;
/// runtime trees list the instances in depth-first order of the expanded
/// tree.
pub fn rows(tree: &RuntimeTree, linking: SubtreeLinking) -> Vec<TreeRow> {
    match linking {
        SubtreeLinking::ByTreeId => tree
            .instances
            .iter()
            .enumerate()
            .map(|(index, instance)| TreeRow {
                instance: InstanceIndex(index),
                tree_id: instance.tree_id.clone(),
                path: String::new(),
                depth: 0,
            })
            .collect(),
        SubtreeLinking::ByInstance => {
            let mut out = Vec::with_capacity(tree.instances.len());
            // The child instances of each instance: instances whose SubTree
            // node is in it, in document order.
            let mut children: Vec<Vec<InstanceIndex>> = vec![Vec::new(); tree.instances.len()];
            for (index, instance) in tree.instances.iter().enumerate() {
                if let Some(node) = instance.parent_node {
                    let parent = tree.node(node).instance.0;
                    if let Some(list) = children.get_mut(parent) {
                        list.push(InstanceIndex(index));
                    }
                }
            }
            let mut stack = vec![(InstanceIndex(0), 0usize)];
            let mut seen = vec![false; tree.instances.len()];
            while let Some((instance, depth)) = stack.pop() {
                let Some(data) = tree.instances.get(instance.0) else {
                    continue;
                };
                if std::mem::replace(&mut seen[instance.0], true) {
                    continue;
                }
                out.push(TreeRow {
                    instance,
                    tree_id: data.tree_id.clone(),
                    path: data.fullpath.clone(),
                    depth,
                });
                for child in children[instance.0].iter().rev() {
                    stack.push((*child, depth + 1));
                }
            }
            // Instances that the walk did not reach (broken links).
            for (index, done) in seen.iter().enumerate() {
                if !done {
                    let data = &tree.instances[index];
                    out.push(TreeRow {
                        instance: InstanceIndex(index),
                        tree_id: data.tree_id.clone(),
                        path: data.fullpath.clone(),
                        depth: 0,
                    });
                }
            }
            out
        }
    }
}

/// Draws the tree list.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let Some(view_id) = cx.workspace.active_view else {
        ui.add_space(8.0);
        ui.label(RichText::new("Open a tree or a log to see its trees.").weak());
        return;
    };
    let Some(view) = cx.workspace.view(view_id) else {
        return;
    };
    let Some(doc) = cx.workspace.doc(view.doc) else {
        return;
    };
    if doc.mode() != cx.mode {
        ui.add_space(8.0);
        ui.label(RichText::new("No document in this mode.").weak());
        return;
    }
    let current = view.root_instance;
    let doc_id = doc.id;
    let linking = doc.linking;
    let list = rows(&doc.runtime, linking);
    ui.label(RichText::new(&doc.title).strong());
    ui.label(
        RichText::new(match linking {
            SubtreeLinking::ByTreeId => format!("{} trees", list.len()),
            SubtreeLinking::ByInstance => format!("{} SubTree instances", list.len().saturating_sub(1)),
        })
        .small()
        .weak(),
    );
    ui.separator();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for row in list {
            ui.horizontal(|ui| {
                ui.add_space(row.depth as f32 * 14.0);
                let selected = linking == SubtreeLinking::ByTreeId && row.instance == current;
                let mut text = RichText::new(&row.tree_id);
                if row.instance == InstanceIndex(0) {
                    text = text.strong();
                }
                let response = ui.selectable_label(selected, text);
                let response = if row.path.is_empty() {
                    response
                } else {
                    response.on_hover_text(&row.path)
                };
                if !row.path.is_empty() {
                    ui.label(RichText::new(&row.path).small().weak());
                }
                if response.clicked() {
                    match linking {
                        SubtreeLinking::ByTreeId => {
                            cx.actions.push(AppAction::SetViewRoot(view_id, row.instance));
                        }
                        SubtreeLinking::ByInstance => {
                            cx.actions.push(AppAction::RevealInstance(view_id, row.instance));
                        }
                    }
                }
                response.context_menu(|ui| {
                    if ui.button("Open in a new tab").clicked() {
                        cx.actions.push(AppAction::OpenTreeInNewTab(doc_id, row.instance));
                    }
                });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_rows_follow_the_expanded_tree() {
        let xml = r#"<root>
            <BehaviorTree ID="Main" _fullpath=""><Sequence _uid="1"><SubTree ID="A" _fullpath="a" _uid="2"/><SubTree ID="B" _fullpath="b" _uid="3"/></Sequence></BehaviorTree>
            <BehaviorTree ID="A" _fullpath="a"><SubTree ID="C" _fullpath="a/c" _uid="4"/></BehaviorTree>
            <BehaviorTree ID="B" _fullpath="b"><AlwaysSuccess _uid="5"/></BehaviorTree>
            <BehaviorTree ID="C" _fullpath="a/c"><AlwaysSuccess _uid="6"/></BehaviorTree>
        </root>"#;
        let tree = RuntimeTree::parse(xml).unwrap();
        let list = rows(&tree, SubtreeLinking::ByInstance);
        let summary: Vec<(String, usize)> =
            list.iter().map(|r| (r.tree_id.clone(), r.depth)).collect();
        assert_eq!(
            summary,
            vec![
                ("Main".into(), 0),
                ("A".into(), 1),
                ("C".into(), 2),
                ("B".into(), 1)
            ]
        );
        assert_eq!(list[2].path, "a/c");
    }

    #[test]
    fn authoring_rows_list_all_trees() {
        let xml = r#"<root><BehaviorTree ID="X"><A/></BehaviorTree><BehaviorTree ID="Y"><B/></BehaviorTree></root>"#;
        let tree = RuntimeTree::parse(xml).unwrap();
        let list = rows(&tree, SubtreeLinking::ByTreeId);
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].tree_id, "Y");
        assert_eq!(list[1].instance, InstanceIndex(1));
    }
}
