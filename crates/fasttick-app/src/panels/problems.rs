//! The problems list: the results of the validation of the open file.
//! Click a row to show the node on the canvas.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use fasttick_core::validate::{Problem, Severity};

use super::PanelContext;
use super::node_editor::{severity_badge, severity_color};
use crate::actions::AppAction;

/// The filter of the problems list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemFilter {
    /// Show errors.
    pub errors: bool,
    /// Show warnings.
    pub warnings: bool,
    /// Show notes.
    pub infos: bool,
    /// Show only problems whose message, tree, code or file name contains
    /// this text (case-insensitive).
    pub text: String,
}

impl Default for ProblemFilter {
    fn default() -> Self {
        ProblemFilter {
            errors: true,
            warnings: true,
            infos: true,
            text: String::new(),
        }
    }
}

impl ProblemFilter {
    /// True if a problem of the file `file_name` passes the filter.
    pub fn accepts(&self, problem: &Problem, file_name: &str) -> bool {
        let severity = match problem.severity {
            Severity::Error => self.errors,
            Severity::Warning => self.warnings,
            Severity::Info => self.infos,
        };
        if !severity {
            return false;
        }
        let text = self.text.trim().to_lowercase();
        text.is_empty()
            || problem.message.to_lowercase().contains(&text)
            || problem
                .tree
                .as_deref()
                .is_some_and(|t| t.to_lowercase().contains(&text))
            || problem.code.to_string().to_lowercase().contains(&text)
            || file_name.to_lowercase().contains(&text)
    }
}

/// Draws the problems list.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let Some((view, editor, _)) = super::active_edit(cx.workspace, cx.mode) else {
        super::empty(ui, "The problems of a tree file show here in edit mode.");
        return;
    };
    let filter_id = ui.id().with("problem_filter");
    let mut filter: ProblemFilter = ui.data_mut(|d| d.get_temp(filter_id).unwrap_or_default());
    let counts = editor.problem_counts();
    ui.horizontal(|ui| {
        let error = severity_color(ui, Severity::Error);
        let warn = severity_color(ui, Severity::Warning);
        ui.toggle_value(
            &mut filter.errors,
            RichText::new(format!("{} errors", counts.errors)).color(error),
        );
        ui.toggle_value(
            &mut filter.warnings,
            RichText::new(format!("{} warnings", counts.warnings)).color(warn),
        );
        ui.toggle_value(&mut filter.infos, format!("{} notes", counts.infos));
        ui.add(
            egui::TextEdit::singleline(&mut filter.text)
                .hint_text("Filter problems")
                .desired_width(180.0),
        );
    });
    ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
    let project = editor.project();
    let rows: Vec<&Problem> = editor
        .problems()
        .iter()
        .filter(|p| filter.accepts(p, &editor.file_name(p.file)))
        .collect();
    if rows.is_empty() {
        super::empty(
            ui,
            if counts.total() == 0 {
                "No problems."
            } else {
                "No problem matches the filter."
            },
        );
        return;
    }
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    // A project with more than one file: a column with the file name.
    let show_files = editor.project().files().nth(1).is_some();
    // A click on a row reveals the node, so the labels do not take clicks
    // for text selection.
    ui.style_mut().interaction.selectable_labels = false;
    let mut table = TableBuilder::new(ui)
        .id_salt("problems_table")
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .column(Column::exact(22.0))
        .column(Column::remainder().at_least(240.0).clip(true))
        .column(Column::initial(120.0).at_least(60.0).clip(true));
    if show_files {
        table = table.column(Column::initial(140.0).at_least(60.0).clip(true));
    }
    table
        .column(Column::initial(70.0).at_least(50.0))
        .header(row_height, |mut header| {
            header.col(|_| {});
            header.col(|ui| {
                ui.strong("Problem");
            });
            header.col(|ui| {
                ui.strong("Tree");
            });
            if show_files {
                header.col(|ui| {
                    ui.strong("File");
                });
            }
            header.col(|ui| {
                ui.strong("Line");
            });
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let problem = rows[row.index()];
                row.col(|ui| {
                    let color = severity_color(ui, problem.severity);
                    severity_badge(ui, problem.severity, color);
                });
                row.col(|ui| {
                    ui.label(&problem.message)
                        .on_hover_text(format!("{}: {}", problem.code, problem.message));
                });
                row.col(|ui| {
                    ui.label(problem.tree.as_deref().unwrap_or(""));
                });
                if show_files {
                    row.col(|ui| {
                        ui.label(editor.file_name(problem.file));
                    });
                }
                row.col(|ui| {
                    let text = match problem.line_col(project) {
                        Some((line, col)) => format!("{line}:{col}"),
                        None => String::new(),
                    };
                    ui.label(RichText::new(text).weak());
                });
                if row.response().clicked()
                    && editor.is_tree_file(problem.file)
                    && let Some(node) = problem.node
                {
                    cx.actions.push(AppAction::RevealXml {
                        view,
                        file: problem.file,
                        node,
                    });
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use fasttick_core::bt::BtFile;
    use fasttick_core::project::Project;
    use fasttick_core::validate::validate_project;

    #[test]
    fn filter_by_severity_and_text() {
        let xml =
            r#"<root BTCPP_format="4"><BehaviorTree ID="Main"><Sequence/></BehaviorTree></root>"#;
        let (project, _) = Project::with_file("t", BtFile::parse(xml, None).unwrap());
        let problems = validate_project(&project);
        let problem = problems.first().expect("an empty Sequence is a problem");
        let mut filter = ProblemFilter::default();
        assert!(filter.accepts(problem, "t.xml"));
        filter.text = "main".into();
        assert!(filter.accepts(problem, "t.xml"), "the tree ID matches");
        filter.text = "no such text".into();
        assert!(!filter.accepts(problem, "t.xml"));
        filter.text = "B1_TEST".into();
        assert!(
            filter.accepts(problem, "b1_test.xml"),
            "the file name matches"
        );
        assert!(!filter.accepts(problem, "t.xml"));
        filter.text.clear();
        match problem.severity {
            Severity::Error => filter.errors = false,
            Severity::Warning => filter.warnings = false,
            Severity::Info => filter.infos = false,
        }
        assert!(!filter.accepts(problem, "t.xml"));
    }
}
