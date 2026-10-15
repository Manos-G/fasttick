//! Child-index paths, the structure hash and the nesting depth of trees.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::xml::{Document, NodeId};

use super::file::BtFile;

/// The deepest node level below a `<BehaviorTree>` element that BT.CPP
/// accepts: `kMaxNestingDepth` in `VerifyXML` (`src/xml_parsing.cpp`).
///
/// `VerifyXML` gives the `<BehaviorTree>` element depth 0 and the root
/// node depth 1, and fails for a depth above 256.
/// (`recursivelyCreateSubtree` has the same constant but starts at the root
/// node with depth 0; `VerifyXML` runs first, so its limit applies.)
pub const MAX_NESTING_DEPTH: usize = 256;

/// The tree index and the child-index path of a node.
///
/// The path has one item for each level below the `<BehaviorTree>`
/// element: the index of the element among the element children of its
/// parent (text and comments do not count). The root node of a tree has the
/// path `[0]`; the second child of the root has `[0, 1]`. Extra root
/// elements (an error in BT.CPP) have `[1]`, `[2]`, ...
///
/// Returns `None` if the node is not inside a tree of the file (for
/// example a detached node, or the `<BehaviorTree>` element itself).
pub fn node_index_path(file: &BtFile, node: NodeId) -> Option<(usize, Vec<usize>)> {
    let doc = file.doc();
    let index = file.index();
    let mut path = Vec::new();
    let mut current = node;
    loop {
        let parent = doc.parent(current)?;
        let position = doc.element_children(parent).position(|c| c == current)?;
        path.push(position);
        if let Some(tree) = index.tree_index_of_element(parent) {
            path.reverse();
            return Some((tree, path));
        }
        current = parent;
    }
}

/// The node at a child-index path of a tree (see [`node_index_path`]).
/// Returns `None` for an empty path or a path that does not exist.
pub fn node_at_index_path(file: &BtFile, tree: usize, index_path: &[usize]) -> Option<NodeId> {
    if index_path.is_empty() {
        return None;
    }
    let doc = file.doc();
    let mut current = file.index().trees.get(tree)?.element;
    for &i in index_path {
        current = doc.element_children(current).nth(i)?;
    }
    Some(current)
}

/// Writes a child-index path as text: the indices joined with `/`, for
/// example `0/1`. This is the key of a node position in the project file.
pub fn format_index_path(index_path: &[usize]) -> String {
    let mut out = String::new();
    for (i, n) in index_path.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(&n.to_string());
    }
    out
}

/// Reads a child-index path from text (see [`format_index_path`]).
/// Returns `None` for an empty text or a part that is not a decimal
/// number.
pub fn parse_index_path(text: &str) -> Option<Vec<usize>> {
    if text.is_empty() {
        return None;
    }
    text.split('/')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                None
            } else {
                part.parse().ok()
            }
        })
        .collect()
}

/// A hash of the structure of a tree. The project file stores it with the
/// node positions; if the hash of the loaded tree is different, the file
/// changed outside FastTick and the positions are not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructureHash(pub u64);

impl fmt::Display for StructureHash {
    /// Writes 16 lowercase hexadecimal digits.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl FromStr for StructureHash {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        u64::from_str_radix(s, 16).map(StructureHash)
    }
}

impl Serialize for StructureHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for StructureHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const ENTER: u8 = 0x01;
const LEAVE: u8 = 0x02;

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *hash ^= u64::from(b);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

/// Computes the structure hash of the elements below a `<BehaviorTree>`
/// element.
///
/// The algorithm is 64-bit FNV-1a (offset basis `0xcbf29ce484222325`, prime
/// `0x100000001b3`) over this byte sequence: a walk of the element children
/// of `tree`, in document order, depth first; for each element the byte
/// `0x01`, the UTF-8 bytes of the element name, then the same for its child
/// elements, then the byte `0x02`. XML names cannot contain the bytes
/// `0x01` and `0x02`, so the sequence describes the shape exactly.
///
/// Only the element names and the parent/child structure count. Attributes,
/// text, comments, white space and the `<BehaviorTree>` element itself (and
/// its ID) do not change the hash. The hash is the same on all platforms
/// and in all versions of FastTick.
pub fn structure_hash(doc: &Document, tree: NodeId) -> StructureHash {
    enum Step {
        Enter(NodeId),
        Leave,
    }
    let mut hash = FNV_OFFSET_BASIS;
    let mut stack: Vec<Step> = doc.element_children(tree).map(Step::Enter).collect();
    stack.reverse();
    while let Some(step) = stack.pop() {
        match step {
            Step::Enter(node) => {
                fnv1a(&mut hash, &[ENTER]);
                fnv1a(&mut hash, doc.name(node).unwrap_or_default().as_bytes());
                stack.push(Step::Leave);
                let first = stack.len();
                stack.extend(doc.element_children(node).map(Step::Enter));
                stack[first..].reverse();
            }
            Step::Leave => fnv1a(&mut hash, &[LEAVE]),
        }
    }
    StructureHash(hash)
}

/// Finds the first element (in document order) below a `<BehaviorTree>`
/// element that is deeper than [`MAX_NESTING_DEPTH`]. BT.CPP rejects such a
/// tree ("Maximum XML nesting depth exceeded").
pub fn find_too_deep(doc: &Document, tree: NodeId) -> Option<NodeId> {
    let mut stack: Vec<(NodeId, usize)> = doc.element_children(tree).map(|c| (c, 1)).collect();
    stack.reverse();
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_NESTING_DEPTH {
            return Some(node);
        }
        let first = stack.len();
        stack.extend(doc.element_children(node).map(|c| (c, depth + 1)));
        stack[first..].reverse();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"<root>
  <BehaviorTree ID="A">
    <Sequence>
      <X/>
      <!-- comment -->
      <Fallback><Y/><Z/></Fallback>
    </Sequence>
  </BehaviorTree>
  <BehaviorTree ID="B"><R/><Extra/></BehaviorTree>
</root>"#;

    #[test]
    fn index_paths() {
        let file = BtFile::parse(TEXT, None).unwrap();
        let doc = file.doc();
        let find = |name: &str| {
            doc.descendants(doc.root())
                .find(|&n| doc.name(n) == Some(name))
                .unwrap()
        };
        assert_eq!(file.node_index_path(find("Sequence")), Some((0, vec![0])));
        assert_eq!(file.node_index_path(find("X")), Some((0, vec![0, 0])));
        assert_eq!(file.node_index_path(find("Z")), Some((0, vec![0, 1, 1])));
        assert_eq!(file.node_index_path(find("Extra")), Some((1, vec![1])));
        assert_eq!(file.node_index_path(find("BehaviorTree")), None);
        assert_eq!(file.node_index_path(doc.root()), None);
        for name in ["Sequence", "X", "Y", "Z", "R", "Extra"] {
            let node = find(name);
            let (tree, path) = file.node_index_path(node).unwrap();
            assert_eq!(file.node_at_index_path(tree, &path), Some(node));
        }
        assert_eq!(file.node_at_index_path(0, &[]), None);
        assert_eq!(file.node_at_index_path(0, &[1]), None);
        assert_eq!(file.node_at_index_path(0, &[0, 5]), None);
        assert_eq!(file.node_at_index_path(9, &[0]), None);
    }

    #[test]
    fn detached_node_has_no_path() {
        let mut file = BtFile::parse(TEXT, None).unwrap();
        let nodes = file.doc_mut().parse_fragment("<Q><W/></Q>").unwrap();
        let w = file.doc().element_children(nodes[0]).next().unwrap();
        assert_eq!(file.node_index_path(w), None);
    }

    #[test]
    fn path_text() {
        assert_eq!(format_index_path(&[0, 1, 12]), "0/1/12");
        assert_eq!(format_index_path(&[0]), "0");
        assert_eq!(format_index_path(&[]), "");
        assert_eq!(parse_index_path("0/1/12"), Some(vec![0, 1, 12]));
        assert_eq!(parse_index_path("0"), Some(vec![0]));
        for bad in ["", "/", "0/", "/0", "a", "0//1", "-1", "+1", " 1", "1.0"] {
            assert_eq!(parse_index_path(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn hash_is_stable_and_structural() {
        let file = BtFile::parse(TEXT, None).unwrap();
        let a = file.structure_hash(0).unwrap();
        // Fixed value: the algorithm must not change between versions.
        let doc = Document::parse("<BehaviorTree><A/></BehaviorTree>").unwrap();
        let single = structure_hash(&doc, doc.document_element().unwrap());
        let mut expected = FNV_OFFSET_BASIS;
        fnv1a(&mut expected, b"\x01A\x02");
        assert_eq!(single, StructureHash(expected));
        assert_eq!(single.to_string(), format!("{expected:016x}"));
        let empty = Document::parse("<BehaviorTree/>").unwrap();
        assert_eq!(
            structure_hash(&empty, empty.document_element().unwrap()).to_string(),
            "cbf29ce484222325"
        );

        // Attributes, comments, white space and the tree ID do not count.
        let same = BtFile::parse(
            r#"<root><BehaviorTree ID="Other" x="1"><Sequence name="s"><X a="b"/><Fallback>
            <!-- c --><Y/><Z/></Fallback></Sequence></BehaviorTree></root>"#,
            None,
        )
        .unwrap();
        assert_eq!(same.structure_hash(0), Some(a));

        // Shape changes do count.
        for changed in [
            "<root><BehaviorTree><Sequence><X/><Fallback><Y/></Fallback></Sequence></BehaviorTree></root>",
            "<root><BehaviorTree><Sequence><X/><Fallback><Z/><Y/></Fallback></Sequence></BehaviorTree></root>",
            "<root><BehaviorTree><Sequence><X/><Fallback><Y><Z/></Y></Fallback></Sequence></BehaviorTree></root>",
            "<root><BehaviorTree><Sequence><X/><Fallback><Y/></Fallback><Z/></Sequence></BehaviorTree></root>",
            "<root><BehaviorTree><Sequence><X/><Fallbac><Y/><Z/></Fallbac></Sequence></BehaviorTree></root>",
        ] {
            let f = BtFile::parse(changed, None).unwrap();
            assert_ne!(f.structure_hash(0), Some(a), "{changed}");
        }
        assert_eq!(file.structure_hash(5), None);
    }

    #[test]
    fn hash_text_and_serde() {
        let h = StructureHash(0x0123_4567_89ab_cdef);
        assert_eq!(h.to_string(), "0123456789abcdef");
        assert_eq!("0123456789abcdef".parse::<StructureHash>().unwrap(), h);
        assert!("xyz".parse::<StructureHash>().is_err());
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, "\"0123456789abcdef\"");
        assert_eq!(serde_json::from_str::<StructureHash>(&json).unwrap(), h);
        assert!(serde_json::from_str::<StructureHash>("\"zz\"").is_err());
        assert!(serde_json::from_str::<StructureHash>("12").is_err());
    }

    fn nested(depth: usize) -> String {
        let mut s = String::from("<root><BehaviorTree ID=\"T\">");
        for _ in 0..depth {
            s.push_str("<D>");
        }
        for _ in 0..depth {
            s.push_str("</D>");
        }
        s.push_str("</BehaviorTree></root>");
        s
    }

    #[test]
    fn nesting_depth() {
        let ok = BtFile::parse(&nested(MAX_NESTING_DEPTH), None).unwrap();
        assert_eq!(find_too_deep(ok.doc(), ok.trees()[0].element), None);
        let bad = BtFile::parse(&nested(MAX_NESTING_DEPTH + 1), None).unwrap();
        let node = find_too_deep(bad.doc(), bad.trees()[0].element).unwrap();
        let (_, path) = bad.node_index_path(node).unwrap();
        assert_eq!(path.len(), MAX_NESTING_DEPTH + 1);
        // The hash works on deep trees without recursion.
        let deep = BtFile::parse(&nested(450), None).unwrap();
        assert!(deep.structure_hash(0).is_some());
    }
}
