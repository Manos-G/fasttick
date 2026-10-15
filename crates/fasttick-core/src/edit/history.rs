//! Undo and redo.

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use crate::project::{FileId, Project};
use crate::xml::NodeId;

use super::editor::{Applied, Editor, StructurePolicy, run};
use super::error::CommandError;
use super::transaction::{FileGeneration, Transaction};

/// The default maximum number of undo steps.
pub const DEFAULT_MAX_DEPTH: usize = 500;

/// A key that identifies a kind of small, repeated edit, for example "set
/// the attribute `msec` of node 12 in file 0". Two edits with the same key
/// that come quickly one after the other become one undo step (see
/// [`History::run_coalesced`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CoalesceKey(String);

impl CoalesceKey {
    /// Makes a key from any text.
    pub fn new(key: impl Into<String>) -> CoalesceKey {
        CoalesceKey(key.into())
    }

    /// The key for changes of one attribute of one element.
    pub fn attribute(file: FileId, node: NodeId, name: &str) -> CoalesceKey {
        CoalesceKey(format!("attribute:{}:{}:{name}", file.0, node.index()))
    }

    /// The key for moves of one unconnected node on the canvas.
    pub fn position(file: FileId, node: NodeId) -> CoalesceKey {
        CoalesceKey(format!("position:{}:{}", file.0, node.index()))
    }

    /// The key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CoalesceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The coalescing data of one edit: the key, the time of the edit and the
/// window.
///
/// The edit merges into the previous undo step if the previous step has the
/// same key, the previous edit of that step was not more than `window`
/// before `now`, and nothing else happened between them (no other edit, no
/// undo or redo, no save of a changed file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coalesce {
    /// The key.
    pub key: CoalesceKey,
    /// The time of the edit. The caller gives it, so tests can control it.
    pub now: Instant,
    /// The largest time between two edits that merge.
    pub window: Duration,
}

/// One step of the undo or the redo stack.
#[derive(Debug, Clone)]
struct Step {
    /// The transaction that goes back (on the undo stack) or forward (on
    /// the redo stack).
    transaction: Transaction,
    /// The generations of the changed files before and after the step.
    generations: Vec<FileGeneration>,
    /// The coalescing key and the time of the last merged edit.
    coalesce: Option<(CoalesceKey, Instant)>,
}

/// An error from [`History::undo`] or [`History::redo`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("cannot {action} \"{label}\": {error}")]
pub struct HistoryError {
    /// "undo" or "redo".
    pub action: &'static str,
    /// The label of the step.
    pub label: String,
    /// The reason.
    pub error: CommandError,
}

/// The undo and redo stacks of a project.
///
/// - [`History::run`] runs an editor session and records it as one undo
///   step. A new step clears the redo stack.
/// - [`History::undo`] and [`History::redo`] apply the steps. They also set
///   the generation of each changed file back or forward, so the dirty
///   state of a file follows the undo and the redo: a file is dirty when its
///   generation is not the generation of its last save (see
///   [`crate::bt::BtFile::is_dirty`]). Saving a file
///   ([`Project::save_file`]) sets its saved mark.
/// - The undo stack holds at most [`History::max_depth`] steps (default
///   [`DEFAULT_MAX_DEPTH`]); the oldest steps are dropped.
/// - [`History::run_coalesced`] merges quick, repeated edits (for example
///   typing in an attribute field) into one step.
///
/// The history refers to files and nodes by their IDs. Use one history for
/// one project, and clear it ([`History::clear`]) when files are reloaded
/// from the disk.
#[derive(Debug, Clone)]
pub struct History {
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    max_depth: usize,
    policy: StructurePolicy,
    /// True if the top undo step can take a coalesced edit: the last
    /// action was a coalesced edit.
    open: bool,
}

impl Default for History {
    fn default() -> Self {
        History::new()
    }
}

impl History {
    /// Makes an empty history with the maximum depth
    /// [`DEFAULT_MAX_DEPTH`].
    pub fn new() -> History {
        History::with_max_depth(DEFAULT_MAX_DEPTH)
    }

    /// Makes an empty history with a maximum depth (at least 1).
    pub fn with_max_depth(max_depth: usize) -> History {
        History {
            undo: VecDeque::new(),
            redo: Vec::new(),
            max_depth: max_depth.max(1),
            policy: StructurePolicy::default(),
            open: false,
        }
    }

    /// The maximum number of undo steps.
    pub fn max_depth(&self) -> usize {
        self.max_depth
    }

    /// Sets the maximum number of undo steps (at least 1). Old steps are
    /// dropped if necessary.
    pub fn set_max_depth(&mut self, max_depth: usize) {
        self.max_depth = max_depth.max(1);
        self.trim();
    }

    /// The structure policy of new sessions.
    pub fn structure_policy(&self) -> StructurePolicy {
        self.policy
    }

    /// Sets the structure policy of new sessions (see [`StructurePolicy`]).
    pub fn set_structure_policy(&mut self, policy: StructurePolicy) {
        self.policy = policy;
    }

    /// Runs an editor session (see [`run`](super::run)) and records it as
    /// one undo step. A session that changes nothing adds no step.
    ///
    /// On error, the project does not change and the history does not
    /// change.
    pub fn run<T>(
        &mut self,
        project: &mut Project,
        f: impl FnOnce(&mut Editor<'_>) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let (value, applied) = run(project, self.policy, f)?;
        self.push(applied);
        Ok(value)
    }

    /// Runs an editor session like [`History::run`], but merges it into
    /// the previous undo step if the coalescing rules allow it (see
    /// [`Coalesce`]).
    pub fn run_coalesced<T>(
        &mut self,
        project: &mut Project,
        coalesce: Coalesce,
        f: impl FnOnce(&mut Editor<'_>) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let (value, applied) = run(project, self.policy, f)?;
        self.push_coalesced(project, applied, coalesce);
        Ok(value)
    }

    /// Applies a transaction to the project and records it as one undo
    /// step.
    pub fn apply(
        &mut self,
        project: &mut Project,
        transaction: Transaction,
    ) -> Result<(), CommandError> {
        let (undo, generations) = transaction.apply_tracked(project)?;
        self.push(Applied { undo, generations });
        Ok(())
    }

    /// Records the result of a session that ran outside the history (see
    /// [`run`](super::run)). Nothing happens if it is empty.
    pub fn push(&mut self, applied: Applied) {
        if applied.is_empty() {
            return;
        }
        self.redo.clear();
        self.undo.push_back(Step {
            transaction: applied.undo,
            generations: applied.generations,
            coalesce: None,
        });
        self.open = false;
        self.trim();
    }

    /// Records the result of a session with coalescing (see
    /// [`History::run_coalesced`]).
    pub fn push_coalesced(&mut self, project: &Project, applied: Applied, coalesce: Coalesce) {
        if applied.is_empty() {
            return;
        }
        if self.open
            && let Some(top) = self.undo.back_mut()
            && let Some((key, last)) = &top.coalesce
            && *key == coalesce.key
            && coalesce
                .now
                .checked_duration_since(*last)
                .is_some_and(|d| d <= coalesce.window)
            && !saved_since(project, &top.generations)
        {
            // Undo the new edit first, then the old step.
            let mut edits = applied.undo.edits;
            edits.append(&mut top.transaction.edits);
            top.transaction.edits = edits;
            for g in applied.generations {
                match top.generations.iter_mut().find(|t| t.file == g.file) {
                    Some(t) => t.after = g.after,
                    None => top.generations.push(g),
                }
            }
            top.coalesce = Some((coalesce.key, coalesce.now));
            self.redo.clear();
            return;
        }
        self.push(applied);
        if let Some(top) = self.undo.back_mut() {
            top.coalesce = Some((coalesce.key, coalesce.now));
        }
        self.open = true;
    }

    fn trim(&mut self) {
        while self.undo.len() > self.max_depth {
            self.undo.pop_front();
        }
    }

    /// True if there is a step to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// True if there is a step to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The label of the step that [`History::undo`] undoes.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.back().map(|s| s.transaction.label.as_str())
    }

    /// The label of the step that [`History::redo`] applies again.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.transaction.label.as_str())
    }

    /// The number of undo steps.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// The number of redo steps.
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Removes all steps.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open = false;
    }

    /// Undoes the last step and returns its label, or `None` if there is
    /// nothing to undo.
    ///
    /// The step must fit the files: each file that it changes must have
    /// the generation that the step left. If a file changed outside the
    /// history (for example [`Project::reload_file`], or a transaction that
    /// was not recorded), the function clears the history and returns
    /// [`CommandError::FileChanged`]. If the step cannot be applied for
    /// another reason (for example because a file of the step was removed
    /// from the project: [`CommandError::UnknownFile`]), the project does
    /// not change, the undo stack is cleared (the older steps depend on this
    /// one) and the function returns the error.
    pub fn undo(&mut self, project: &mut Project) -> Result<Option<String>, HistoryError> {
        let Some(step) = self.undo.pop_back() else {
            return Ok(None);
        };
        self.open = false;
        let label = step.transaction.label.clone();
        if let Some(error) = check_files(project, &step.generations, |g| g.after) {
            if matches!(error, CommandError::FileChanged(_)) {
                self.clear();
            } else {
                self.undo.clear();
            }
            return Err(HistoryError {
                action: "undo",
                label,
                error,
            });
        }
        match step.transaction.apply_tracked(project) {
            Ok((redo, _)) => {
                set_generations(project, &step.generations, |g| g.before);
                self.redo.push(Step {
                    transaction: redo,
                    generations: step.generations,
                    coalesce: None,
                });
                Ok(Some(label))
            }
            Err(error) => {
                self.undo.clear();
                Err(HistoryError {
                    action: "undo",
                    label,
                    error,
                })
            }
        }
    }

    /// Applies the last undone step again and returns its label, or `None`
    /// if there is nothing to redo.
    ///
    /// The checks are the same as for [`History::undo`]. If the step
    /// cannot be applied, the project does not change, the redo stack is
    /// cleared and the function returns the error.
    pub fn redo(&mut self, project: &mut Project) -> Result<Option<String>, HistoryError> {
        let Some(step) = self.redo.pop() else {
            return Ok(None);
        };
        self.open = false;
        let label = step.transaction.label.clone();
        if let Some(error) = check_files(project, &step.generations, |g| g.before) {
            if matches!(error, CommandError::FileChanged(_)) {
                self.clear();
            } else {
                self.redo.clear();
            }
            return Err(HistoryError {
                action: "redo",
                label,
                error,
            });
        }
        match step.transaction.apply_tracked(project) {
            Ok((undo, _)) => {
                set_generations(project, &step.generations, |g| g.after);
                self.undo.push_back(Step {
                    transaction: undo,
                    generations: step.generations,
                    coalesce: None,
                });
                self.trim();
                Ok(Some(label))
            }
            Err(error) => {
                self.redo.clear();
                Err(HistoryError {
                    action: "redo",
                    label,
                    error,
                })
            }
        }
    }
}

/// Checks that the files of a step are in the project and have the
/// expected generations. A file with another generation changed outside
/// the history ([`CommandError::FileChanged`]); the step must not be
/// applied to it.
fn check_files(
    project: &Project,
    generations: &[FileGeneration],
    expected: impl Fn(&FileGeneration) -> u64,
) -> Option<CommandError> {
    generations.iter().find_map(|g| match project.file(g.file) {
        None => Some(CommandError::UnknownFile(g.file)),
        Some(f) if f.generation() != expected(g) => Some(CommandError::FileChanged(g.file)),
        Some(_) => None,
    })
}

/// True if a file of the step was saved after the step: then a new edit
/// must not merge into the step, so that undo can go back to the saved
/// state.
fn saved_since(project: &Project, generations: &[FileGeneration]) -> bool {
    generations.iter().any(|g| {
        project
            .file(g.file)
            .is_some_and(|f| f.saved_generation() == g.after)
    })
}

fn set_generations(
    project: &mut Project,
    generations: &[FileGeneration],
    pick: impl Fn(&FileGeneration) -> u64,
) {
    for g in generations {
        if let Some(file) = project.file_mut(g.file) {
            file.set_generation(pick(g));
        }
    }
}
