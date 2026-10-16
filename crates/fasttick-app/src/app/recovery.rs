//! Copies of unsaved changes and their restore after a crash (see
//! [`crate::recovery`]).

use super::*;
use crate::editor::EditorDoc;
use crate::recovery::{Orphan, Record, Recovery};

impl FastTickApp {
    /// Keeps copies of unsaved changes in `root` (a folder in the FastTick
    /// data folder for the real application) and offers the copies of a
    /// FastTick that stopped without a normal close.
    pub fn use_recovery_dir(&mut self, root: &Path) {
        if let Some(recovery) = &mut self.recovery {
            recovery.finish();
        }
        match Recovery::start(root) {
            Ok(recovery) => {
                self.recovery_offer = recovery.orphans();
                self.recovery = Some(recovery);
            }
            Err(err) => {
                self.recovery = None;
                self.recovery_offer.clear();
                eprintln!(
                    "FastTick cannot keep copies of unsaved changes in {}: {err}",
                    root.display()
                );
            }
        }
    }

    /// The folder of the copies of this FastTick, if it keeps copies.
    pub fn recovery_dir(&self) -> Option<&Path> {
        self.recovery.as_ref().map(Recovery::dir)
    }

    /// The number of documents that the restore question offers.
    pub fn recovery_offered(&self) -> usize {
        self.recovery_offer.iter().map(|o| o.records.len()).sum()
    }

    /// Writes the copies of the documents with unsaved changes (at most
    /// once in [`crate::recovery::INTERVAL`]) and deletes the copies that
    /// are not necessary any more. Call it once per frame.
    pub(super) fn update_recovery(&mut self, ctx: &egui::Context) {
        let Some(recovery) = self.recovery.as_mut() else {
            return;
        };
        let docs = self.workspace.docs();
        for id in recovery.recorded() {
            if !docs.iter().any(|d| d.id == id && dirty(d).is_some()) {
                recovery.remove(id);
            }
        }
        let waiting = docs
            .iter()
            .filter_map(dirty)
            .any(|(doc, editor)| recovery.needs_write(doc.id, editor.revision()));
        if !waiting {
            return;
        }
        if !recovery.due() {
            ctx.request_repaint_after(crate::recovery::INTERVAL);
            return;
        }
        recovery.checked();
        let mut failure = None;
        for (doc, editor) in docs.iter().filter_map(dirty) {
            if !recovery.needs_write(doc.id, editor.revision()) {
                continue;
            }
            let texts = editor.unsaved_texts();
            if texts.is_empty() {
                // Only the project file has changes: no XML text to keep.
                recovery.remove(doc.id);
                continue;
            }
            let record = Record::new(
                doc.title.clone(),
                editor.path(),
                editor.project_path().map(Path::to_path_buf),
                texts,
            );
            if let Err(err) = recovery.write(doc.id, editor.revision(), &record) {
                failure = Some(err);
            }
        }
        if let Some(err) = failure
            && !self.recovery_failed
        {
            self.recovery_failed = true;
            self.notify(
                ctx,
                NoticeLevel::Warning,
                format!("FastTick cannot keep a copy of the unsaved changes: {err}"),
            );
        }
    }

    /// Writes the copies of the documents with unsaved changes now, also
    /// if the last copy is new (for tests).
    pub fn write_recovery_now(&mut self, ctx: &egui::Context) {
        if let Some(recovery) = &mut self.recovery {
            recovery.reset_due();
        }
        self.update_recovery(ctx);
    }

    /// Deletes the copies of this FastTick, at a normal close.
    pub(super) fn finish_recovery(&mut self) {
        if let Some(recovery) = &mut self.recovery {
            recovery.finish();
        }
    }

    /// The question about the copies of a FastTick that stopped without a
    /// normal close.
    pub(super) fn recovery_dialog(&mut self, ctx: &egui::Context) {
        if self.recovery_offer.is_empty() {
            return;
        }
        #[derive(Clone, Copy)]
        enum Choice {
            Restore,
            Delete,
            Later,
        }
        let mut choice = None;
        let records: Vec<&Record> = self
            .recovery_offer
            .iter()
            .flat_map(|o| o.records.iter())
            .collect();
        egui::Window::new("Restore unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -60.0])
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.label(
                    "FastTick stopped before you saved the changes of these documents. \
                     It kept a copy of them:",
                );
                ui.add_space(6.0);
                for record in &records {
                    let place = record
                        .project
                        .as_deref()
                        .or(record.main.as_deref())
                        .map(|p| {
                            self.remote
                                .title(p)
                                .unwrap_or_else(|| p.display().to_string())
                        })
                        .unwrap_or_else(|| "a new file".to_owned());
                    // A long path is shortened at its start: the dialog
                    // keeps its width.
                    ui.label(RichText::new(&record.title).strong());
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "{}, {}",
                                record.age_text(),
                                crate::remote_ui::short_path(&place, 60)
                            ))
                            .weak(),
                        )
                        .truncate(),
                    )
                    .on_hover_text(place);
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Restore opens the documents with the changes. Then save them. \
                         Delete removes the copies. Not now asks again at the next start.",
                    )
                    .weak(),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Restore").strong()).clicked() {
                        choice = Some(Choice::Restore);
                    }
                    if ui.button("Delete").clicked() {
                        choice = Some(Choice::Delete);
                    }
                    if ui.button("Not now").clicked() {
                        choice = Some(Choice::Later);
                    }
                });
            });
        let Some(choice) = choice else {
            return;
        };
        let orphans = std::mem::take(&mut self.recovery_offer);
        match choice {
            Choice::Restore => self.restore_orphans(ctx, orphans),
            Choice::Delete => orphans.into_iter().for_each(Orphan::delete),
            // Dropping the orphans releases their locks.
            Choice::Later => {}
        }
    }

    fn restore_orphans(&mut self, ctx: &egui::Context, orphans: Vec<Orphan>) {
        let mut restored = 0;
        let mut problems = Vec::new();
        for orphan in orphans {
            for record in &orphan.records {
                match self.restore_record(record) {
                    Ok(()) => restored += 1,
                    Err(message) => problems.push(format!("{}: {message}", record.title)),
                }
            }
            orphan.delete();
        }
        // Keep a copy of the restored changes at once.
        if let Some(recovery) = &mut self.recovery {
            recovery.reset_due();
        }
        self.set_mode(Mode::Edit);
        if problems.is_empty() {
            self.notify(
                ctx,
                NoticeLevel::Info,
                format!(
                    "Restored the unsaved changes of {}. Save them to keep them.",
                    counted(restored, "document", "documents")
                ),
            );
        } else {
            self.notify(
                ctx,
                NoticeLevel::Warning,
                format!(
                    "Restored {}. FastTick could not restore some changes: {}",
                    counted(restored, "document", "documents"),
                    problems.join("; ")
                ),
            );
        }
    }

    /// Opens the document of a record with the unsaved texts in it.
    fn restore_record(&mut self, record: &Record) -> Result<(), String> {
        let texts = record.texts();
        let open_path = record.project.as_deref().or(record.main.as_deref());
        // The document is open already (for example from the command line).
        if let Some(view) = open_path.and_then(|p| self.find_open_view(p, None)) {
            let doc = self
                .workspace
                .doc_of_view(view)
                .map(|d| d.id)
                .ok_or("the document closed")?;
            let editor = self.workspace.editor_mut(doc).ok_or("not a tree file")?;
            let errors = editor.restore_files(&texts);
            self.workspace.refresh_editor_views();
            self.focus_view(view);
            return result_of(errors);
        }
        let from_disk = [record.project.as_deref(), record.main.as_deref()]
            .into_iter()
            .flatten()
            .filter(|p| p.exists())
            .find_map(|p| Some((p.to_path_buf(), crate::jobs::read_tree(p).ok()?.editor)));
        let (path, mut editor) = match from_disk {
            Some(found) => found,
            None => {
                // The file is gone (or it is a new file): the text of the
                // main file makes the document.
                let main = record
                    .files
                    .iter()
                    .find(|f| f.path == record.main)
                    .or(record.files.first())
                    .ok_or("the copy has no files")?;
                let editor = EditorDoc::from_text(&main.text, record.main.clone())?;
                (record.main.clone().unwrap_or_default(), Box::new(editor))
            }
        };
        // Absolute includes of a file of a robot are in the cache folder.
        if let Some(mirror) = self.remote.mirror_root(&path) {
            editor.set_mirror_root(Some(mirror));
        }
        let errors = editor.restore_files(&texts);
        if path.as_os_str().is_empty() {
            self.add_untitled_document(*editor, &record.title);
        } else {
            let loaded = crate::jobs::LoadedTree {
                editor,
                missing: MissingFiles::default(),
            };
            self.add_tree_document(&path, loaded)
                .ok_or("cannot show the document")?;
        }
        result_of(errors)
    }
}

/// A document with unsaved changes, with its editor.
fn dirty(doc: &crate::documents::Document) -> Option<(&crate::documents::Document, &EditorDoc)> {
    let editor = doc.editor.as_deref().filter(|e| e.is_dirty())?;
    Some((doc, editor))
}

fn result_of(errors: Vec<String>) -> Result<(), String> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}
