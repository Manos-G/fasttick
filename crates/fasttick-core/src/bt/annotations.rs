//! Notes and groups of a tree, stored as XML attributes.
//!
//! FastTick keeps three kinds of annotations in the XML file, so that they
//! go with the file (version control, a robot over SSH, other people):
//!
//! - A **node note**: the attribute `_note` of a tree node.
//! - A **group**: a box around some nodes of a tree, with a title, a
//!   description and a color. Each member node has the attribute
//!   `_group="<id>"`. The `<BehaviorTree>` element defines the group:
//!   `_group_<id>` (the title), `_group_<id>_description` and
//!   `_group_<id>_color`.
//! - A **canvas note**: a text box at a free place on the canvas of a
//!   tree. The `<BehaviorTree>` element holds `_note_<id>` (the text),
//!   `_note_<id>_box` (`"x y width height"` in canvas units) and
//!   `_note_<id>_color`.
//!
//! An `<id>` is a positive decimal number without leading zeros.
//!
//! BT.CPP 4.x ignores all of these attributes (see `src/xml_parsing.cpp`):
//! a node attribute that starts with `_` and is not reserved goes into
//! `NodeConfig::other_attributes` (a `<SubTree>` skips it), and BT.CPP
//! reads only `ID` from a `<BehaviorTree>` element.

use crate::xml::{Document, NodeId};

/// The attribute of a node note.
pub const NOTE_ATTRIBUTE: &str = "_note";

/// The attribute that makes a node a member of a group.
pub const GROUP_ATTRIBUTE: &str = "_group";

/// The colors that groups and canvas notes can have. The first name is the
/// default of a canvas note.
pub const ANNOTATION_COLORS: [&str; 7] =
    ["yellow", "blue", "green", "orange", "red", "purple", "gray"];

/// The default size of a new canvas note (width, height; canvas units).
pub const DEFAULT_NOTE_SIZE: [f32; 2] = [220.0, 90.0];

/// The smallest size of a canvas note (width, height; canvas units).
pub const MIN_NOTE_SIZE: [f32; 2] = [60.0, 32.0];

/// A group of a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfo {
    /// The group ID (unique in the tree).
    pub id: u32,
    /// The title.
    pub title: String,
    /// The description (can be empty).
    pub description: String,
    /// The color name (see [`ANNOTATION_COLORS`]), if set.
    pub color: Option<String>,
}

/// A canvas note of a tree.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasNoteInfo {
    /// The note ID (unique in the tree).
    pub id: u32,
    /// The text.
    pub text: String,
    /// The box: left, top, width, height (canvas units).
    pub rect: [f32; 4],
    /// The color name (see [`ANNOTATION_COLORS`]), if set.
    pub color: Option<String>,
}

/// One annotation attribute of a `<BehaviorTree>` element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeAnnotation {
    /// `_group_<id>`: the title of a group.
    GroupTitle(u32),
    /// `_group_<id>_description`.
    GroupDescription(u32),
    /// `_group_<id>_color`.
    GroupColor(u32),
    /// `_note_<id>`: the text of a canvas note.
    NoteText(u32),
    /// `_note_<id>_box`.
    NoteBox(u32),
    /// `_note_<id>_color`.
    NoteColor(u32),
}

impl TreeAnnotation {
    /// Reads an attribute name. `None` for other names.
    pub fn parse(name: &str) -> Option<TreeAnnotation> {
        if let Some(rest) = name.strip_prefix("_group_") {
            let (id, suffix) = split_id(rest)?;
            return match suffix {
                "" => Some(TreeAnnotation::GroupTitle(id)),
                "_description" => Some(TreeAnnotation::GroupDescription(id)),
                "_color" => Some(TreeAnnotation::GroupColor(id)),
                _ => None,
            };
        }
        let rest = name.strip_prefix("_note_")?;
        let (id, suffix) = split_id(rest)?;
        match suffix {
            "" => Some(TreeAnnotation::NoteText(id)),
            "_box" => Some(TreeAnnotation::NoteBox(id)),
            "_color" => Some(TreeAnnotation::NoteColor(id)),
            _ => None,
        }
    }

    /// The attribute name.
    pub fn name(self) -> String {
        match self {
            TreeAnnotation::GroupTitle(id) => format!("_group_{id}"),
            TreeAnnotation::GroupDescription(id) => format!("_group_{id}_description"),
            TreeAnnotation::GroupColor(id) => format!("_group_{id}_color"),
            TreeAnnotation::NoteText(id) => format!("_note_{id}"),
            TreeAnnotation::NoteBox(id) => format!("_note_{id}_box"),
            TreeAnnotation::NoteColor(id) => format!("_note_{id}_color"),
        }
    }

    /// The group ID, for the group attributes.
    pub fn group(self) -> Option<u32> {
        match self {
            TreeAnnotation::GroupTitle(id)
            | TreeAnnotation::GroupDescription(id)
            | TreeAnnotation::GroupColor(id) => Some(id),
            _ => None,
        }
    }

    /// The note ID, for the canvas note attributes.
    pub fn note(self) -> Option<u32> {
        match self {
            TreeAnnotation::NoteText(id)
            | TreeAnnotation::NoteBox(id)
            | TreeAnnotation::NoteColor(id) => Some(id),
            _ => None,
        }
    }
}

/// Splits `"<id><suffix>"`. The ID is a positive decimal number without
/// leading zeros; the suffix is empty or starts with `_`.
fn split_id(text: &str) -> Option<(u32, &str)> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let (number, suffix) = text.split_at(digits);
    if number.is_empty() || number.starts_with('0') {
        return None;
    }
    if !suffix.is_empty() && !suffix.starts_with('_') {
        return None;
    }
    let id: u32 = number.parse().ok()?;
    Some((id, suffix))
}

/// Reads a group ID (the value of `_group`).
pub fn parse_group_id(value: &str) -> Option<u32> {
    let value = value.trim();
    match split_id(value) {
        Some((id, "")) => Some(id),
        _ => None,
    }
}

/// Reads a note box (`"x y width height"`). The size must be positive and
/// all numbers finite.
pub fn parse_box(value: &str) -> Option<[f32; 4]> {
    let mut numbers = value.split_ascii_whitespace().map(str::parse::<f32>);
    let mut out = [0.0_f32; 4];
    for slot in &mut out {
        *slot = numbers.next()?.ok()?;
    }
    if numbers.next().is_some() || out.iter().any(|v| !v.is_finite()) {
        return None;
    }
    if out[2] <= 0.0 || out[3] <= 0.0 {
        return None;
    }
    Some(out)
}

/// Writes a note box. The numbers are rounded to whole canvas units.
pub fn format_box(rect: [f32; 4]) -> String {
    // `+ 0.0` turns -0 into 0.
    let [x, y, w, h] = rect.map(|v| v.round() + 0.0);
    format!("{x} {y} {w} {h}")
}

/// Reads a color name. Unknown names give `None`.
pub fn parse_color(value: &str) -> Option<String> {
    let value = value.trim();
    ANNOTATION_COLORS
        .iter()
        .find(|c| c.eq_ignore_ascii_case(value))
        .map(|c| (*c).to_owned())
}

/// The group of a node (the value of `_group`), if it is a valid ID.
pub fn node_group(doc: &Document, node: NodeId) -> Option<u32> {
    parse_group_id(doc.attribute(node, GROUP_ATTRIBUTE)?)
}

/// The note of a node (the value of `_note`), if it is not blank.
pub fn node_note(doc: &Document, node: NodeId) -> Option<&str> {
    doc.attribute(node, NOTE_ATTRIBUTE)
        .filter(|text| !text.trim().is_empty())
}

/// The groups that a `<BehaviorTree>` element defines, by ID.
pub fn tree_groups(doc: &Document, tree: NodeId) -> Vec<GroupInfo> {
    let mut groups: Vec<GroupInfo> = Vec::new();
    for attribute in doc.attributes(tree) {
        if let Some(TreeAnnotation::GroupTitle(id)) = TreeAnnotation::parse(attribute.name()) {
            groups.push(GroupInfo {
                id,
                title: attribute.value().to_owned(),
                description: String::new(),
                color: None,
            });
        }
    }
    for group in &mut groups {
        if let Some(text) = doc.attribute(tree, &TreeAnnotation::GroupDescription(group.id).name())
        {
            group.description = text.to_owned();
        }
        group.color = doc
            .attribute(tree, &TreeAnnotation::GroupColor(group.id).name())
            .and_then(parse_color);
    }
    groups.sort_by_key(|g| g.id);
    groups.dedup_by_key(|g| g.id);
    groups
}

/// The canvas notes that a `<BehaviorTree>` element defines, by ID. A note
/// without a valid box is not in the list.
pub fn tree_notes(doc: &Document, tree: NodeId) -> Vec<CanvasNoteInfo> {
    let mut notes: Vec<CanvasNoteInfo> = Vec::new();
    for attribute in doc.attributes(tree) {
        if let Some(TreeAnnotation::NoteText(id)) = TreeAnnotation::parse(attribute.name()) {
            let Some(rect) = doc
                .attribute(tree, &TreeAnnotation::NoteBox(id).name())
                .and_then(parse_box)
            else {
                continue;
            };
            notes.push(CanvasNoteInfo {
                id,
                text: attribute.value().to_owned(),
                rect,
                color: doc
                    .attribute(tree, &TreeAnnotation::NoteColor(id).name())
                    .and_then(parse_color),
            });
        }
    }
    notes.sort_by_key(|n| n.id);
    notes.dedup_by_key(|n| n.id);
    notes
}

/// The smallest group ID that is not used in a `<BehaviorTree>` element:
/// one more than the largest group ID of the definitions. `members` are
/// the group IDs of the nodes, so that a new group does not take the ID of
/// old member attributes.
pub fn next_group_id(doc: &Document, tree: NodeId, members: impl IntoIterator<Item = u32>) -> u32 {
    let defined = doc
        .attributes(tree)
        .iter()
        .filter_map(|a| TreeAnnotation::parse(a.name()).and_then(TreeAnnotation::group));
    defined
        .chain(members)
        .max()
        .map_or(1, |max| max.saturating_add(1))
}

/// The smallest note ID that is not used in a `<BehaviorTree>` element.
pub fn next_note_id(doc: &Document, tree: NodeId) -> u32 {
    doc.attributes(tree)
        .iter()
        .filter_map(|a| TreeAnnotation::parse(a.name()).and_then(TreeAnnotation::note))
        .max()
        .map_or(1, |max| max.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> (Document, NodeId) {
        let doc = Document::parse(text).unwrap();
        let tree = doc.document_element().unwrap();
        (doc, tree)
    }

    #[test]
    fn attribute_names() {
        use TreeAnnotation::*;
        assert_eq!(TreeAnnotation::parse("_group_1"), Some(GroupTitle(1)));
        assert_eq!(
            TreeAnnotation::parse("_group_12_description"),
            Some(GroupDescription(12))
        );
        assert_eq!(TreeAnnotation::parse("_group_3_color"), Some(GroupColor(3)));
        assert_eq!(TreeAnnotation::parse("_note_7"), Some(NoteText(7)));
        assert_eq!(TreeAnnotation::parse("_note_7_box"), Some(NoteBox(7)));
        assert_eq!(TreeAnnotation::parse("_note_7_color"), Some(NoteColor(7)));
        for bad in [
            "_group",
            "_group_",
            "_group_0",
            "_group_01",
            "_group_1x",
            "_group_1_x",
            "_note",
            "_note_a",
            "_note_1_size",
            "_groups_1",
            "group_1",
            "_note_-1",
        ] {
            assert_eq!(TreeAnnotation::parse(bad), None, "{bad}");
        }
        for a in [
            GroupTitle(5),
            GroupDescription(5),
            GroupColor(5),
            NoteText(9),
            NoteBox(9),
            NoteColor(9),
        ] {
            assert_eq!(TreeAnnotation::parse(&a.name()), Some(a));
        }
    }

    #[test]
    fn values() {
        assert_eq!(parse_group_id("3"), Some(3));
        assert_eq!(parse_group_id(" 3 "), Some(3));
        assert_eq!(parse_group_id("0"), None);
        assert_eq!(parse_group_id("x"), None);
        assert_eq!(parse_box("10 -20 200 80"), Some([10.0, -20.0, 200.0, 80.0]));
        assert_eq!(parse_box("1.5 2 3 4"), Some([1.5, 2.0, 3.0, 4.0]));
        assert_eq!(parse_box("1 2 3"), None);
        assert_eq!(parse_box("1 2 3 4 5"), None);
        assert_eq!(parse_box("1 2 0 4"), None);
        assert_eq!(parse_box("1 2 inf 4"), None);
        assert_eq!(parse_box("1 2 NaN 4"), None);
        assert_eq!(format_box([10.4, -20.6, 200.0, 80.0]), "10 -21 200 80");
        assert_eq!(format_box([-0.2, 0.0, 1.0, 1.0]), "0 0 1 1");
        assert_eq!(parse_color("Blue"), Some("blue".to_owned()));
        assert_eq!(parse_color("pink"), None);
    }

    #[test]
    fn reads_the_annotations_of_a_tree() {
        let (doc, tree) = doc(concat!(
            r#"<BehaviorTree ID="Main" _group_2="Pick" _group_2_description="Pick the box" "#,
            r#"_group_1="Move" _group_1_color="green" _group_3_color="red" "#,
            r#"_note_4="Check&#10;this" _note_4_box="10 20 200 80" _note_4_color="blue" "#,
            r#"_note_5="no box" _note_6="bad box" _note_6_box="1 2"><Sequence _group="1"/></BehaviorTree>"#
        ));
        assert_eq!(
            tree_groups(&doc, tree),
            vec![
                GroupInfo {
                    id: 1,
                    title: "Move".into(),
                    description: String::new(),
                    color: Some("green".into()),
                },
                GroupInfo {
                    id: 2,
                    title: "Pick".into(),
                    description: "Pick the box".into(),
                    color: None,
                },
            ]
        );
        assert_eq!(
            tree_notes(&doc, tree),
            vec![CanvasNoteInfo {
                id: 4,
                text: "Check\nthis".into(),
                rect: [10.0, 20.0, 200.0, 80.0],
                color: Some("blue".into()),
            }]
        );
        assert_eq!(next_group_id(&doc, tree, [7]), 8);
        assert_eq!(next_group_id(&doc, tree, []), 4);
        assert_eq!(next_note_id(&doc, tree), 7);
        let sequence = doc.element_children(tree).next().unwrap();
        assert_eq!(node_group(&doc, sequence), Some(1));
        assert_eq!(node_note(&doc, sequence), None);
    }

    #[test]
    fn empty_tree() {
        let (doc, tree) = doc(r#"<BehaviorTree ID="Main" _note=" "/>"#);
        assert!(tree_groups(&doc, tree).is_empty());
        assert!(tree_notes(&doc, tree).is_empty());
        assert_eq!(next_group_id(&doc, tree, []), 1);
        assert_eq!(next_note_id(&doc, tree), 1);
        assert_eq!(node_note(&doc, tree), None);
    }
}
