//! One BT.CPP XML file: the document, its path and its index.

use std::path::{Path, PathBuf};

use crate::fsio::write_atomic;
use crate::xml::{Change, Document, EditError, NodeId, XmlError};

use super::index::{BtIndex, TreeEntry};
use super::node::NodeView;
use super::path::{self, StructureHash};

/// An error from loading, parsing or saving a [`BtFile`].
#[derive(Debug, thiserror::Error)]
pub enum BtError {
    /// The file cannot be read.
    #[error("cannot read {}: {source}", .path.display())]
    Read {
        /// The file path.
        path: PathBuf,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The file cannot be written.
    #[error("cannot write {}: {source}", .path.display())]
    Write {
        /// The file path.
        path: PathBuf,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The text is not XML that BT.CPP can read.
    #[error("{}{error}", path_prefix(.path))]
    Parse {
        /// The file path, if the text came from a file.
        path: Option<PathBuf>,
        /// The parse error, with the line and the column.
        #[source]
        error: XmlError,
    },
    /// The file has no path yet.
    #[error("the file has no path; save it with a file name first")]
    NoPath,
}

fn path_prefix(path: &Option<PathBuf>) -> String {
    match path {
        Some(p) => format!("{}: ", p.display()),
        None => String::new(),
    }
}

/// The text of a new, empty file.
pub const NEW_FILE_TEXT: &str =
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<root BTCPP_format=\"4\">\n</root>\n";

/// One BT.CPP XML file: the lossless document, the file path and the
/// index.
///
/// After each change of the document, call [`BtFile::reindex`] (or change
/// the document through [`BtFile::apply`] or [`BtFile::edit`], which do
/// it).
///
/// The file has a *generation* for the dirty state. The edit module sets a
/// new generation after each change ([`BtFile::bump_generation`]) and an
/// old one after an undo ([`BtFile::set_generation`]). The file is dirty
/// when the generation is not the generation of the last save.
#[derive(Debug, Clone)]
pub struct BtFile {
    path: Option<PathBuf>,
    doc: Document,
    index: BtIndex,
    generation: u64,
    saved_generation: u64,
    next_generation: u64,
}

impl BtFile {
    /// Makes a file from a document.
    pub fn from_document(doc: Document, path: Option<PathBuf>) -> BtFile {
        let index = BtIndex::build(&doc);
        BtFile {
            path,
            doc,
            index,
            generation: 0,
            saved_generation: 0,
            next_generation: 1,
        }
    }

    /// Makes a new file with an empty `<root BTCPP_format="4">` and no path
    /// ([`NEW_FILE_TEXT`]). It is not dirty.
    pub fn new_empty() -> BtFile {
        // The text is fixed and valid.
        let doc = Document::parse(NEW_FILE_TEXT).unwrap_or_default();
        BtFile::from_document(doc, None)
    }

    /// Parses a file from text. `path` is only stored (and used in error
    /// messages); nothing is read.
    pub fn parse(text: &str, path: Option<PathBuf>) -> Result<BtFile, BtError> {
        match Document::parse(text) {
            Ok(doc) => Ok(BtFile::from_document(doc, path)),
            Err(error) => Err(BtError::Parse { path, error }),
        }
    }

    /// Reads and parses a file. The file must be UTF-8; a byte order mark is
    /// accepted and kept.
    pub fn load(path: impl AsRef<Path>) -> Result<BtFile, BtError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| BtError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        match Document::parse_bytes(&bytes) {
            Ok(doc) => Ok(BtFile::from_document(doc, Some(path.to_path_buf()))),
            Err(error) => Err(BtError::Parse {
                path: Some(path.to_path_buf()),
                error,
            }),
        }
    }

    /// The file path, or `None` for a new file.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Sets the file path. It does not read or write anything.
    pub fn set_path(&mut self, path: Option<PathBuf>) {
        self.path = path;
    }

    /// A short name for the user: the file name, or "untitled".
    pub fn display_name(&self) -> String {
        self.path
            .as_deref()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_owned())
    }

    /// The document.
    pub fn doc(&self) -> &Document {
        &self.doc
    }

    /// The document, for changes. Call [`BtFile::reindex`] after the
    /// changes.
    pub fn doc_mut(&mut self) -> &mut Document {
        &mut self.doc
    }

    /// The index.
    pub fn index(&self) -> &BtIndex {
        &self.index
    }

    /// Builds the index again. Call it after each change of the document.
    /// It reads only the child elements of the root element and the model
    /// sections.
    pub fn reindex(&mut self) {
        self.index = BtIndex::build(&self.doc);
    }

    /// Applies changes to the document (see [`Document::apply_all`]) and
    /// builds the index again. Returns the changes that undo them.
    pub fn apply(
        &mut self,
        changes: impl IntoIterator<Item = Change>,
    ) -> Result<Vec<Change>, EditError> {
        let result = self.doc.apply_all(changes);
        self.reindex();
        result
    }

    /// Runs a function that changes the document, then builds the index
    /// again.
    pub fn edit<R>(&mut self, f: impl FnOnce(&mut Document) -> R) -> R {
        let result = f(&mut self.doc);
        self.reindex();
        result
    }

    /// The text of the document.
    pub fn to_xml_string(&self) -> String {
        self.doc.to_string()
    }

    /// Writes the document to `path` and makes it the file path. The
    /// function writes a temporary file in the same directory and renames
    /// it, so an error does not damage an existing file. After a successful
    /// save, the file is not dirty.
    pub fn save(&mut self, path: impl AsRef<Path>) -> Result<(), BtError> {
        let path = path.as_ref();
        write_atomic(path, self.to_xml_string().as_bytes()).map_err(|source| BtError::Write {
            path: path.to_path_buf(),
            source,
        })?;
        self.path = Some(path.to_path_buf());
        self.mark_saved();
        Ok(())
    }

    /// Writes the document to its path. See [`BtFile::save`].
    pub fn save_in_place(&mut self) -> Result<(), BtError> {
        let path = self.path.clone().ok_or(BtError::NoPath)?;
        self.save(path)
    }

    /// The current generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Gives the file a new generation that no earlier state had, and
    /// returns it. Call it after a change.
    pub fn bump_generation(&mut self) -> u64 {
        self.generation = self.next_generation;
        self.next_generation += 1;
        self.generation
    }

    /// Sets the generation, for example back to the generation of an
    /// earlier state after an undo.
    pub fn set_generation(&mut self, generation: u64) {
        self.generation = generation;
        self.next_generation = self.next_generation.max(generation.saturating_add(1));
    }

    /// The generation of the last save (or of the load).
    pub fn saved_generation(&self) -> u64 {
        self.saved_generation
    }

    /// Records the current generation as saved.
    pub fn mark_saved(&mut self) {
        self.saved_generation = self.generation;
    }

    /// True if the document changed after the last save.
    pub fn is_dirty(&self) -> bool {
        self.generation != self.saved_generation
    }

    /// A view of an element.
    pub fn node(&self, id: NodeId) -> Option<NodeView<'_>> {
        NodeView::new(&self.doc, id)
    }

    /// The tree with this ID (the last one, as in BT.CPP).
    pub fn tree(&self, id: &str) -> Option<&TreeEntry> {
        self.index.tree(id)
    }

    /// The trees, in document order.
    pub fn trees(&self) -> &[TreeEntry] {
        &self.index.trees
    }

    /// The tree index and the child-index path of a node (see
    /// [`path::node_index_path`]).
    pub fn node_index_path(&self, node: NodeId) -> Option<(usize, Vec<usize>)> {
        path::node_index_path(self, node)
    }

    /// The node at a child-index path (see [`path::node_at_index_path`]).
    pub fn node_at_index_path(&self, tree: usize, index_path: &[usize]) -> Option<NodeId> {
        path::node_at_index_path(self, tree, index_path)
    }

    /// The structure hash of a tree (see [`path::structure_hash`]).
    pub fn structure_hash(&self, tree: usize) -> Option<StructureHash> {
        self.index
            .trees
            .get(tree)
            .map(|t| path::structure_hash(&self.doc, t.element))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_index() {
        let file = BtFile::parse(
            "<root BTCPP_format=\"4\"><BehaviorTree ID=\"T\"><A/></BehaviorTree></root>",
            Some(PathBuf::from("/x/t.xml")),
        )
        .unwrap();
        assert_eq!(file.trees().len(), 1);
        assert_eq!(file.tree("T").unwrap().id, "T");
        assert_eq!(file.display_name(), "t.xml");
        assert!(!file.is_dirty());
        let root = file.tree("T").unwrap().root.unwrap();
        assert_eq!(file.node(root).unwrap().type_id(), Some("A"));
    }

    #[test]
    fn parse_error_has_position() {
        let err = BtFile::parse("<root>\n<A>\n</root>", Some(PathBuf::from("f.xml"))).unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("f.xml: line "), "{text}");
        let err = BtFile::parse("<root", None).unwrap_err();
        assert!(err.to_string().starts_with("line 1"), "{err}");
    }

    #[test]
    fn new_empty_file() {
        let file = BtFile::new_empty();
        assert_eq!(file.to_xml_string(), NEW_FILE_TEXT);
        assert_eq!(file.index().format.as_deref(), Some("4"));
        assert_eq!(file.display_name(), "untitled");
        assert!(matches!(file.clone().save_in_place(), Err(BtError::NoPath)));
    }

    #[test]
    fn edit_reindexes() {
        let mut file = BtFile::new_empty();
        let root = file.index().root.unwrap();
        let undo = file
            .edit(|doc| {
                let tree =
                    doc.parse_fragment("<BehaviorTree ID=\"New\"><AlwaysSuccess/></BehaviorTree>")?;
                Ok::<_, Box<dyn std::error::Error>>(doc.insert_element_pretty(root, 0, tree[0])?)
            })
            .unwrap();
        assert_eq!(file.trees().len(), 1);
        assert_eq!(
            file.to_xml_string(),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<root BTCPP_format=\"4\">\n    <BehaviorTree ID=\"New\"><AlwaysSuccess/></BehaviorTree>\n</root>\n"
        );
        let redo = file.apply(undo).unwrap();
        assert!(file.trees().is_empty());
        assert_eq!(file.to_xml_string(), NEW_FILE_TEXT);
        file.apply(redo).unwrap();
        assert_eq!(file.trees().len(), 1);
    }

    #[test]
    fn generations() {
        let mut file = BtFile::new_empty();
        assert_eq!(file.generation(), 0);
        let g1 = file.bump_generation();
        assert!(file.is_dirty());
        let g2 = file.bump_generation();
        assert_ne!(g1, g2);
        file.set_generation(0);
        assert!(!file.is_dirty());
        // A new change after an undo never reuses an old generation.
        let g3 = file.bump_generation();
        assert!(g3 > g2);
        file.mark_saved();
        assert!(!file.is_dirty());
        assert_eq!(file.saved_generation(), g3);
        file.set_generation(100);
        assert_eq!(file.bump_generation(), 101);
    }

    #[test]
    fn load_and_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.xml");
        let text =
            "\u{feff}<root>\r\n  <BehaviorTree ID=\"T\"><A x=\"1\"/></BehaviorTree>\r\n</root>\r\n";
        std::fs::write(&path, text).unwrap();
        let mut file = BtFile::load(&path).unwrap();
        assert_eq!(file.path(), Some(path.as_path()));
        assert_eq!(file.to_xml_string(), text);
        let a = file.tree("T").unwrap().root.unwrap();
        file.edit(|doc| doc.set_attribute(a, "x", "2").map(|_| ()))
            .unwrap();
        file.bump_generation();
        assert!(file.is_dirty());
        file.save_in_place().unwrap();
        assert!(!file.is_dirty());
        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(saved, text.replace("x=\"1\"", "x=\"2\""));

        let other = dir.path().join("copy.xml");
        file.save(&other).unwrap();
        assert_eq!(file.path(), Some(other.as_path()));
        assert_eq!(std::fs::read_to_string(&other).unwrap(), saved);

        // Errors.
        let missing = dir.path().join("missing.xml");
        let err = BtFile::load(&missing).unwrap_err();
        assert!(matches!(err, BtError::Read { .. }));
        assert!(err.to_string().contains("missing.xml"));
        std::fs::write(&missing, b"<root>\xff</root>").unwrap();
        let err = BtFile::load(&missing).unwrap_err();
        assert!(err.to_string().contains("UTF-8"), "{err}");
        let err = file.save(dir.path().join("no/such/dir.xml")).unwrap_err();
        assert!(matches!(err, BtError::Write { .. }));
        assert_eq!(file.path(), Some(other.as_path()));
    }
}
