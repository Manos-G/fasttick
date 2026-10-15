//! The search panel. In edit mode it searches all trees of the open file;
//! in live and replay modes it searches the running tree. The matching
//! nodes get a mark on the canvas. Click a result to show the node.

use std::collections::HashSet;
use std::ops::Range;

use egui::text::{LayoutJob, TextFormat};
use egui::{RichText, Ui};
use fasttick_core::project::FileId;
use fasttick_core::runtime::{NodeIndex, Uid};
use fasttick_core::search::{Field, Fields, Query, search_project, search_runtime};
use fasttick_core::xml::NodeId;

use super::PanelContext;
use crate::actions::AppAction;
use crate::canvas::NodeSource;
use crate::documents::{DocId, ViewId};

/// Where a result points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// An element of a file (edit mode).
    Xml(FileId, NodeId),
    /// A node of a running tree.
    Runtime(NodeIndex, Option<Uid>),
}

/// One result row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultRow {
    /// The tree ID.
    pub tree: String,
    /// The instance name of the node.
    pub label: String,
    /// The field that matched.
    pub field: Field,
    /// The attribute name (ports and conditions).
    pub attribute: Option<String>,
    /// The field text.
    pub text: String,
    /// The match in `text`.
    pub range: Range<usize>,
    /// The node.
    pub target: Target,
    /// True if the node is in the open file (edit mode) or has a UID.
    pub can_reveal: bool,
}

/// The state of the search panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchState {
    /// The query.
    pub query: Query,
    /// The document and the version that `results` belong to.
    key: Option<(DocId, u64, Query)>,
    /// The results.
    pub results: Vec<ResultRow>,
    /// The nodes that are marked on the canvas of each view.
    marked: Vec<(ViewId, u64)>,
}

impl Default for SearchState {
    fn default() -> Self {
        SearchState {
            query: Query::new(""),
            key: None,
            results: Vec::new(),
            marked: Vec::new(),
        }
    }
}

/// The name of a field, for the result list.
pub fn field_name(field: Field) -> &'static str {
    match field {
        Field::Name => "name",
        Field::Type => "type",
        Field::PortName => "port",
        Field::PortValue => "value",
        Field::Condition => "condition",
    }
}

/// Draws the search panel.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let state_id = egui::Id::new("fasttick_search_state");
    let mut state: SearchState = ui.data_mut(|d| d.get_temp(state_id).unwrap_or_default());
    let Some(view) = cx.workspace.active() else {
        super::empty(ui, "Open a tree or a log to search its nodes.");
        return;
    };
    let view_id = view.id;
    let Some(doc) = cx.workspace.doc(view.doc) else {
        return;
    };
    if doc.mode() != cx.mode {
        super::empty(ui, "No document in this mode.");
        return;
    }
    let doc_id = doc.id;

    // The query.
    let mut focus = false;
    ui.horizontal(|ui| {
        let label = ui.label("Find").id;
        let response = ui
            .add(
                egui::TextEdit::singleline(&mut state.query.text)
                    .hint_text("node name, type, port or value")
                    .desired_width(ui.available_width()),
            )
            .labelled_by(label);
        if ui.memory(|m| m.focused().is_none())
            && ui
                .data(|d| d.get_temp::<bool>(state_id.with("focus")))
                .unwrap_or(false)
        {
            response.request_focus();
            focus = true;
        }
    });
    if focus {
        ui.data_mut(|d| d.insert_temp(state_id.with("focus"), false));
    }
    ui.horizontal_wrapped(|ui| {
        ui.toggle_value(&mut state.query.case_sensitive, "Aa")
            .on_hover_text("Match case");
        ui.toggle_value(&mut state.query.whole_word, "Word")
            .on_hover_text("Whole words");
        ui.toggle_value(&mut state.query.wildcard, "* ?")
            .on_hover_text("Wildcards: * is any text, ? is one character");
        ui.separator();
        let fields = &mut state.query.fields;
        ui.toggle_value(&mut fields.name, "Names");
        ui.toggle_value(&mut fields.type_id, "Types");
        ui.toggle_value(&mut fields.port_names, "Ports");
        ui.toggle_value(&mut fields.port_values, "Values");
        ui.toggle_value(&mut fields.conditions, "Conditions");
        if *fields == Fields::NONE {
            *fields = Fields::ALL;
        }
    });

    // The results, again when the query or the document changes.
    let version = match &doc.editor {
        Some(editor) => editor.revision(),
        None => std::sync::Arc::as_ptr(&doc.runtime) as u64,
    };
    let key = (doc_id, version, state.query.clone());
    if state.key.as_ref() != Some(&key) {
        state.results = match &doc.editor {
            Some(editor) => {
                let main = editor.file_id();
                let project = editor.project();
                search_project(project, &state.query)
                    .into_iter()
                    .map(|m| {
                        let label = project
                            .file(m.file)
                            .and_then(|f| {
                                fasttick_core::bt::NodeView::new(f.doc(), m.node).map(|n| {
                                    n.instance_name().or(n.type_id()).unwrap_or("").to_owned()
                                })
                            })
                            .unwrap_or_default();
                        ResultRow {
                            tree: m.location.tree_id().to_owned(),
                            label,
                            field: m.field,
                            attribute: m.attribute,
                            can_reveal: m.file == main,
                            text: m.text,
                            range: m.range,
                            target: Target::Xml(m.file, m.node),
                        }
                    })
                    .collect()
            }
            None => search_runtime(&doc.runtime, &state.query)
                .into_iter()
                .map(|m| {
                    let node = doc.runtime.node(m.node);
                    let tree = doc
                        .runtime
                        .instances
                        .get(node.instance.0)
                        .map(|i| i.tree_id.clone())
                        .unwrap_or_default();
                    ResultRow {
                        tree,
                        label: node.name.clone(),
                        field: m.field,
                        attribute: m.attribute,
                        text: m.text,
                        range: m.range,
                        can_reveal: node.uid.is_some(),
                        target: Target::Runtime(m.node, node.uid),
                    }
                })
                .collect(),
        };
        state.key = Some(key);
        state.marked.clear();
    }

    // Marks on the canvas of the active view.
    let canvas_key = (view_id, version ^ hash_query(&state.query));
    if !state.marked.contains(&canvas_key) {
        let sources: HashSet<NodeSource> = state
            .results
            .iter()
            .map(|r| match r.target {
                Target::Xml(file, node) => NodeSource::Xml(file, node),
                Target::Runtime(index, _) => NodeSource::Runtime(index),
            })
            .collect();
        if let Some(view) = cx.workspace.view_mut(view_id) {
            let tree = view.canvas.tree_mut();
            for id in tree.ids().collect::<Vec<_>>() {
                if let Some(node) = tree.get_mut(id) {
                    node.deco.search_match = sources.contains(&node.source);
                }
            }
        }
        state.marked.push(canvas_key);
    }

    ui.separator();
    if state.query.is_empty() {
        super::empty(ui, "Type a text to search.");
    } else if state.results.is_empty() {
        super::empty(ui, "No node matches.");
    } else {
        let nodes: HashSet<Target> = state.results.iter().map(|r| r.target).collect();
        ui.label(
            RichText::new(format!(
                "{} matches in {} nodes",
                state.results.len(),
                nodes.len()
            ))
            .small()
            .weak(),
        );
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, state.results.len(), |ui, rows| {
                for row in &state.results[rows] {
                    result_row(ui, cx, view_id, doc_id, row);
                }
            });
    }
    ui.data_mut(|d| d.insert_temp(state_id, state));
}

/// Asks the search panel to take the keyboard focus on the next frame.
pub fn request_focus(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("fasttick_search_state").with("focus"), true));
}

fn hash_query(query: &Query) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    query.hash(&mut h);
    h.finish()
}

fn result_row(ui: &mut Ui, cx: &mut PanelContext<'_>, view: ViewId, doc: DocId, row: &ResultRow) {
    let palette = cx.palette;
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = LayoutJob::default();
    let plain = TextFormat {
        font_id: font.clone(),
        color: ui.visuals().text_color(),
        ..TextFormat::default()
    };
    let weak = TextFormat {
        font_id: font.clone(),
        color: ui.visuals().weak_text_color(),
        ..TextFormat::default()
    };
    job.append(&row.label, 0.0, plain.clone());
    let field = match &row.attribute {
        Some(attribute) => format!("  {} {attribute}: ", field_name(row.field)),
        None => format!("  {}: ", field_name(row.field)),
    };
    job.append(&field, 0.0, weak.clone());
    let text = &row.text;
    let range = row.range.start.min(text.len())..row.range.end.min(text.len());
    // Only a part of a long text, around the match.
    let from = text[..range.start]
        .char_indices()
        .rev()
        .nth(24)
        .map_or(0, |(i, _)| i);
    if from > 0 {
        job.append("…", 0.0, weak.clone());
    }
    job.append(&text[from..range.start], 0.0, plain.clone());
    job.append(
        &text[range.clone()],
        0.0,
        TextFormat {
            background: palette.search.gamma_multiply(0.45),
            ..plain.clone()
        },
    );
    let rest = &text[range.end..];
    let cut = rest.char_indices().nth(40).map_or(rest.len(), |(i, _)| i);
    job.append(&rest[..cut], 0.0, plain);
    if cut < rest.len() {
        job.append("…", 0.0, weak.clone());
    }
    job.append(&format!("  {}", row.tree), 0.0, weak);
    let response = ui
        .add_enabled(
            row.can_reveal,
            egui::Button::new(job).frame(false).truncate(),
        )
        .on_disabled_hover_text("This node is in another file.");
    if response.clicked() {
        match row.target {
            Target::Xml(file, node) => cx.actions.push(AppAction::RevealXml { view, file, node }),
            Target::Runtime(_, Some(uid)) => cx.actions.push(AppAction::RevealNode(doc, uid)),
            Target::Runtime(_, None) => {}
        }
    }
}
