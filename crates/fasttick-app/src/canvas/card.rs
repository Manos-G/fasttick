//! The geometry of a node card, in world units (zoom 1).
//!
//! ```text
//!  ┌─┬──────────────────────────────────┐
//!  │B│ [G]  label                   (S) │   B: category band
//!  │a│      TypeID                      │   G: category glyph
//!  │n├──────────────────────────────────┤   S: status badge
//!  │d│ port: value                      │
//!  │ │ port: value                      │
//!  │ │ +2 more                          │
//!  │ ├──────────────────────────────────┤
//!  │ │ _skipIf: script                  │   pre- and post-conditions
//!  └─┴──────────────────────────────────┘
//! ```
//!
//! [`measure_tree`] computes the display text and the size of each card
//! with a [`TextMeasure`]. The painter uses [`CardGeometry`] to place the
//! parts. Both use the same constants, so text always fits its card.

use egui::{Pos2, Rect, Vec2, pos2, vec2};

use super::model::{CanvasNode, CanvasTree, CardText};

/// Width of the category band.
pub const BAND: f32 = 5.0;
/// Horizontal padding.
pub const PAD_X: f32 = 10.0;
/// Padding above the label.
pub const PAD_TOP: f32 = 8.0;
/// Padding below the last line.
pub const PAD_BOTTOM: f32 = 8.0;
/// Size of the category glyph box.
pub const GLYPH: f32 = 18.0;
/// Gap between the glyph and the text.
pub const GLYPH_GAP: f32 = 8.0;
/// Diameter of the status badge.
pub const BADGE: f32 = 15.0;
/// Gap between the text and the status badge.
pub const BADGE_GAP: f32 = 8.0;
/// Font size of the label.
pub const LABEL_SIZE: f32 = 13.0;
/// Font size of the type line.
pub const TYPE_SIZE: f32 = 11.0;
/// Font size of the port lines.
pub const PORT_SIZE: f32 = 11.0;
/// Height of the label line.
pub const LABEL_LINE: f32 = 17.0;
/// Height of the type line.
pub const TYPE_LINE: f32 = 15.0;
/// Height of a port line.
pub const PORT_LINE: f32 = 15.0;
/// Space between the header and the first port line (the separator is in
/// the middle).
pub const PORT_GAP: f32 = 9.0;
/// The smallest card width.
pub const MIN_WIDTH: f32 = 128.0;
/// The largest card width. Longer text is shortened with an ellipsis.
pub const MAX_WIDTH: f32 = 280.0;
/// The most port lines that a card shows.
pub const MAX_PORT_LINES: usize = 3;
/// The separator between a port name and its value.
pub const PORT_SEPARATOR: &str = ": ";
/// The ellipsis character (Roboto has it).
pub const ELLIPSIS: &str = "\u{2026}";

/// The kinds of text on a card. Each kind has its own font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextKind {
    /// The label (medium weight).
    Label,
    /// The type line.
    Type,
    /// A port name.
    PortName,
    /// A port value (monospace).
    PortValue,
    /// The "+N more" line and small chips.
    Note,
}

impl TextKind {
    /// The font size at zoom 1.
    pub fn size(self) -> f32 {
        match self {
            TextKind::Label => LABEL_SIZE,
            TextKind::Type => TYPE_SIZE,
            TextKind::PortName | TextKind::PortValue => PORT_SIZE,
            TextKind::Note => 10.5,
        }
    }
}

/// Measures text at zoom 1.
pub trait TextMeasure {
    /// The width of `text` in the font of `kind`.
    fn width(&mut self, text: &str, kind: TextKind) -> f32;

    /// The width of one character. The default measures a one-character
    /// string.
    fn char_width(&mut self, c: char, kind: TextKind) -> f32 {
        let mut buffer = [0u8; 4];
        self.width(c.encode_utf8(&mut buffer), kind)
    }
}

/// A fixed-width measure: each character is `width` wide. For tests.
#[derive(Debug, Clone, Copy)]
pub struct FixedMeasure(pub f32);

impl TextMeasure for FixedMeasure {
    fn width(&mut self, text: &str, _kind: TextKind) -> f32 {
        text.chars().count() as f32 * self.0
    }
}

/// Shortens `text` with an ellipsis so that it is at most `max_width` wide.
/// Returns the text and its width.
pub fn elide(
    measure: &mut dyn TextMeasure,
    text: &str,
    kind: TextKind,
    max_width: f32,
) -> (String, f32) {
    // Control characters (for example new lines in scripts) would break the
    // single-line layout; show them as spaces.
    let clean: std::borrow::Cow<'_, str> = if text.chars().any(char::is_control) {
        text.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect::<String>()
            .into()
    } else {
        text.into()
    };
    let full = measure.width(&clean, kind);
    if full <= max_width {
        return (clean.into_owned(), full);
    }
    let ellipsis = measure.width(ELLIPSIS, kind);
    let budget = max_width - ellipsis;
    let mut width = 0.0;
    let mut end = 0;
    for (index, c) in clean.char_indices() {
        let w = measure.char_width(c, kind);
        if width + w > budget {
            break;
        }
        width += w;
        end = index + c.len_utf8();
    }
    let mut out = clean[..end].trim_end().to_owned();
    out.push_str(ELLIPSIS);
    let out_width = measure.width(&out, kind);
    (out, out_width)
}

/// The x position of the label and the type line.
pub const TEXT_LEFT: f32 = BAND + PAD_X + GLYPH + GLYPH_GAP;

/// The space that the header needs beside the text.
const HEADER_EXTRA: f32 = TEXT_LEFT + BADGE_GAP + BADGE + PAD_X;

/// Computes the display text and the size of one card.
pub fn measure_node(node: &CanvasNode, measure: &mut dyn TextMeasure) -> CardText {
    let max_text = MAX_WIDTH - HEADER_EXTRA;
    let (label, label_w) = elide(measure, &node.label, TextKind::Label, max_text);
    let (type_line, type_w) = if node.label_is_type() {
        (None, 0.0)
    } else {
        let (text, width) = elide(measure, &node.type_id, TextKind::Type, max_text);
        (Some(text), width)
    };
    let header_w = HEADER_EXTRA + label_w.max(type_w);

    let max_port = MAX_WIDTH - BAND - 2.0 * PAD_X;
    let separator_w = measure.width(PORT_SEPARATOR, TextKind::PortName);
    let shown = if node.ports.len() > MAX_PORT_LINES {
        MAX_PORT_LINES - 1
    } else {
        node.ports.len()
    };
    let mut ports = Vec::with_capacity(shown);
    let mut ports_w: f32 = 0.0;
    for port in &node.ports[..shown] {
        let (name, name_w) = elide(measure, &port.name, TextKind::PortName, max_port * 0.45);
        let (value, value_w) = elide(
            measure,
            &port.value,
            TextKind::PortValue,
            max_port - name_w - separator_w,
        );
        ports_w = ports_w.max(name_w + separator_w + value_w);
        ports.push((name, value));
    }
    let more = (node.ports.len() > shown).then(|| {
        let text = format!("+{} more", node.ports.len() - shown);
        ports_w = ports_w.max(measure.width(&text, TextKind::Note));
        text
    });
    // All conditions show: a node has 8 at most, and they change how the
    // node runs.
    let mut conditions = Vec::with_capacity(node.conditions.len());
    for line in &node.conditions {
        let (name, name_w) = elide(measure, &line.name, TextKind::PortName, max_port * 0.45);
        let (value, value_w) = elide(
            measure,
            &line.value,
            TextKind::PortValue,
            max_port - name_w - separator_w,
        );
        ports_w = ports_w.max(name_w + separator_w + value_w);
        conditions.push((name, value));
    }
    let ports_needed = BAND + 2.0 * PAD_X + ports_w;

    let width = header_w.max(ports_needed).clamp(MIN_WIDTH, MAX_WIDTH);
    let mut card = CardText {
        label,
        type_line,
        ports,
        more,
        conditions,
        size: Vec2::ZERO,
    };
    card.size = vec2(width.round(), CardGeometry::height_of(&card));
    card
}

/// Computes the card text and size of all nodes.
pub fn measure_tree(tree: &mut CanvasTree, measure: &mut dyn TextMeasure) {
    for node in tree.nodes_mut() {
        node.card = measure_node(node, measure);
    }
}

/// Computes the card text and size of the nodes that have no size yet
/// (see [`CanvasTree::copy_measurements_from`]).
pub fn measure_missing(tree: &mut CanvasTree, measure: &mut dyn TextMeasure) {
    for node in tree.nodes_mut() {
        if node.card.size == Vec2::ZERO {
            node.card = measure_node(node, measure);
        }
    }
}

/// The places of the parts of a card, relative to the top-left corner of
/// the card, in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardGeometry {
    /// The height of the header (label, type line, padding).
    pub header_height: f32,
    /// The number of lines in the port section (ports and "+N more").
    pub port_lines: usize,
    /// The number of condition lines.
    pub condition_lines: usize,
}

impl CardGeometry {
    /// The geometry of a card.
    pub fn of(card: &CardText) -> Self {
        let lines = if card.type_line.is_some() { 1.0 } else { 0.0 };
        CardGeometry {
            header_height: PAD_TOP + LABEL_LINE + lines * TYPE_LINE + PAD_BOTTOM,
            port_lines: card.ports.len() + usize::from(card.more.is_some()),
            condition_lines: card.conditions.len(),
        }
    }

    /// The height of a card.
    pub fn height_of(card: &CardText) -> f32 {
        let geometry = CardGeometry::of(card);
        geometry.height()
    }

    /// The height of the card.
    pub fn height(&self) -> f32 {
        if self.condition_lines > 0 {
            self.condition_top() + self.condition_lines as f32 * PORT_LINE + PAD_BOTTOM - 2.0
        } else if self.port_lines == 0 {
            self.header_height
        } else {
            self.header_height + PORT_GAP + self.port_lines as f32 * PORT_LINE + PAD_BOTTOM - 2.0
        }
    }

    /// The y position of the first condition line.
    fn condition_top(&self) -> f32 {
        let ports = if self.port_lines == 0 {
            0.0
        } else {
            // The same space as between the header and the first port.
            self.port_lines as f32 * PORT_LINE + PORT_GAP - 1.0
        };
        self.header_height + ports + PORT_GAP - 1.0
    }

    /// The y position of the separator above the conditions.
    pub fn condition_separator_y(&self) -> f32 {
        self.condition_top() - PORT_GAP * 0.65 + 1.0
    }

    /// The top-left corner of condition line `index`.
    pub fn condition_pos(&self, index: usize) -> Pos2 {
        pos2(
            BAND + PAD_X,
            self.condition_top() + index as f32 * PORT_LINE,
        )
    }

    /// The centre of the glyph box.
    pub fn glyph_center(&self) -> Pos2 {
        pos2(BAND + PAD_X + GLYPH * 0.5, self.header_height * 0.5)
    }

    /// The top-left corner of the label.
    pub fn label_pos(&self, has_type: bool) -> Pos2 {
        if has_type {
            pos2(TEXT_LEFT, PAD_TOP)
        } else {
            pos2(TEXT_LEFT, (self.header_height - LABEL_LINE) * 0.5)
        }
    }

    /// The top-left corner of the type line.
    pub fn type_pos(&self) -> Pos2 {
        pos2(TEXT_LEFT, PAD_TOP + LABEL_LINE)
    }

    /// The centre of the status badge, for a card of `width`.
    pub fn badge_center(&self, width: f32) -> Pos2 {
        pos2(width - PAD_X - BADGE * 0.5, self.header_height * 0.5)
    }

    /// The y position of the separator line.
    pub fn separator_y(&self) -> f32 {
        self.header_height + PORT_GAP * 0.35
    }

    /// The top-left corner of port line `index`.
    pub fn port_pos(&self, index: usize) -> Pos2 {
        pos2(
            BAND + PAD_X,
            self.header_height + PORT_GAP + index as f32 * PORT_LINE - 1.0,
        )
    }
}

/// Maps a point of the card geometry (world units, relative to the card)
/// to the screen.
pub fn to_screen(card_screen: Rect, zoom: f32, local: Pos2) -> Pos2 {
    card_screen.min + local.to_vec2() * zoom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::model::{NodeSource, PortLine};
    use fasttick_core::runtime::NodeIndex;

    fn node(label: &str, type_id: &str, ports: usize) -> CanvasNode {
        let mut node = CanvasNode::new(NodeSource::Runtime(NodeIndex(0)), label, type_id);
        node.ports = (0..ports)
            .map(|i| PortLine {
                name: format!("port{i}"),
                value: format!("{{value_{i}}}"),
            })
            .collect();
        node
    }

    #[test]
    fn conditions_make_the_card_taller() {
        let mut m = FixedMeasure(7.0);
        let plain = measure_node(&node("n", "T", 1), &mut m);
        let mut with = node("n", "T", 1);
        with.conditions = vec![
            PortLine {
                name: "_skipIf".into(),
                value: "x == 1".into(),
            },
            PortLine {
                name: "_post".into(),
                value: "a := a + 1".into(),
            },
        ];
        let card = measure_node(&with, &mut m);
        assert_eq!(card.conditions.len(), 2);
        assert_eq!(card.size.y - plain.size.y, 2.0 * PORT_LINE + PORT_GAP - 2.0);
        let geometry = CardGeometry::of(&card);
        // The conditions are below the ports and inside the card.
        assert!(geometry.condition_pos(0).y > geometry.port_pos(0).y + PORT_LINE - 0.5);
        assert!(geometry.condition_separator_y() > geometry.port_pos(0).y + PORT_LINE - 0.5);
        assert!(geometry.condition_pos(1).y + PORT_LINE <= card.size.y);
        // Without ports, the conditions take the place of the ports.
        let mut only = node("n", "T", 0);
        only.conditions = with.conditions.clone();
        let card = measure_node(&only, &mut m);
        let geometry = CardGeometry::of(&card);
        assert_eq!(geometry.condition_pos(0), geometry.port_pos(0));
        assert_eq!(geometry.condition_separator_y(), geometry.separator_y());
    }

    #[test]
    fn elide_keeps_short_text() {
        let mut m = FixedMeasure(7.0);
        assert_eq!(
            elide(&mut m, "hello", TextKind::Label, 100.0),
            ("hello".into(), 35.0)
        );
        let (text, width) = elide(&mut m, "a long label text", TextKind::Label, 50.0);
        assert!(text.ends_with(ELLIPSIS));
        assert!(width <= 50.0, "{width}");
        assert_eq!(text, format!("a long{ELLIPSIS}"));
        // Control characters become spaces.
        assert_eq!(elide(&mut m, "a\nb", TextKind::Label, 100.0).0, "a b");
        // Multi-byte characters are cut at a character boundary.
        let (text, _) = elide(&mut m, "ééééééééé", TextKind::Label, 30.0);
        assert_eq!(text, format!("ééé{ELLIPSIS}"));
        // No room at all.
        let (text, _) = elide(&mut m, "abc", TextKind::Label, 1.0);
        assert_eq!(text, ELLIPSIS);
    }

    #[test]
    fn card_size_follows_content() {
        let mut m = FixedMeasure(7.0);
        let plain = measure_node(&node("Seq", "Seq", 0), &mut m);
        assert_eq!(plain.type_line, None);
        assert_eq!(plain.size.x, MIN_WIDTH);
        assert_eq!(plain.size.y, PAD_TOP + LABEL_LINE + PAD_BOTTOM);

        let named = measure_node(&node("main_sequence", "Sequence", 0), &mut m);
        assert_eq!(named.type_line.as_deref(), Some("Sequence"));
        assert!(named.size.y > plain.size.y);

        let with_ports = measure_node(&node("n", "T", 2), &mut m);
        assert_eq!(with_ports.ports.len(), 2);
        assert_eq!(with_ports.more, None);
        let geometry = CardGeometry::of(&with_ports);
        assert_eq!(geometry.port_lines, 2);
        assert!(with_ports.size.y > named.size.y);

        let many = measure_node(&node("n", "T", 7), &mut m);
        assert_eq!(many.ports.len(), MAX_PORT_LINES - 1);
        assert_eq!(many.more.as_deref(), Some("+5 more"));
        assert_eq!(CardGeometry::of(&many).port_lines, MAX_PORT_LINES);

        let long = measure_node(&node(&"x".repeat(200), "T", 0), &mut m);
        assert!(long.size.x <= MAX_WIDTH && long.size.x > MAX_WIDTH - 10.0);
        assert!(long.label.ends_with(ELLIPSIS));
    }

    #[test]
    fn text_fits_inside_the_card() {
        let mut m = FixedMeasure(6.5);
        let mut n = node(&"label".repeat(30), &"Type".repeat(30), 1);
        n.ports[0].value = "v".repeat(500);
        let card = measure_node(&n, &mut m);
        let geometry = CardGeometry::of(&card);
        let label_end = geometry.label_pos(true).x + m.width(&card.label, TextKind::Label);
        assert!(label_end <= geometry.badge_center(card.size.x).x - BADGE * 0.5);
        let (name, value) = &card.ports[0];
        let port_end = geometry.port_pos(0).x
            + m.width(name, TextKind::PortName)
            + m.width(PORT_SEPARATOR, TextKind::PortName)
            + m.width(value, TextKind::PortValue);
        assert!(
            port_end <= card.size.x - PAD_X + 0.5,
            "{port_end} > {}",
            card.size.x
        );
        assert!(geometry.port_pos(0).y > geometry.separator_y());
        assert!(geometry.port_pos(0).y + PORT_LINE <= card.size.y);
    }

    #[test]
    fn measure_tree_sets_all_cards() {
        let mut builder = crate::canvas::model::CanvasTreeBuilder::new();
        let root = builder.push(None, node("a", "Sequence", 0));
        builder.push(Some(root), node("b", "Action", 1));
        let mut tree = builder.finish("T");
        measure_tree(&mut tree, &mut FixedMeasure(7.0));
        assert!(tree.nodes().iter().all(|n| n.card.size.x >= MIN_WIDTH));
        let screen = Rect::from_min_size(pos2(100.0, 100.0), vec2(256.0, 80.0));
        assert_eq!(to_screen(screen, 2.0, pos2(10.0, 5.0)), pos2(120.0, 110.0));
    }
}
