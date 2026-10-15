//! The geometry of a node card, in world units (zoom 1).
//!
//! ```text
//!  ┌─┬──────────────────────────────────┐
//!  │B│ [G]  label                   (S) │   B: category band
//!  │a│      TypeID                      │   G: category glyph
//!  │n├──────────────────────────────────┤   S: status badge
//!  │d│ port: value                      │   all ports
//!  │ │ code: a := 1; b := 2; c := 3;    │
//!  │ │       d := 4                     │   a long value wraps
//!  │ │╭────────────────────────────────╮│
//!  │ ││ _skipIf: script                ││   pre- and post-conditions,
//!  │ │╰────────────────────────────────╯│   in a box with an outline
//!  └─┴──────────────────────────────────┘
//! ```
//!
//! [`measure_tree`] computes the display text and the size of each card
//! with a [`TextMeasure`]. The painter uses [`CardGeometry`] to place the
//! parts. Both use the same constants, so text always fits its card.

use egui::{Pos2, Rect, Vec2, pos2, vec2};

use super::model::{CanvasNode, CanvasTree, CardEntry, CardText, PortLine};

/// Width of the category band.
pub const BAND: f32 = 7.5;
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
/// The space between the ports (or the header) and the condition box, and
/// below the box.
pub const CONDITION_GAP: f32 = 6.0;
/// The space between the edge of the condition box and its first and last
/// line.
pub const CONDITION_PAD_Y: f32 = 3.0;
/// How far the condition box extends to the left and the right of the
/// text column.
pub const CONDITION_INSET: f32 = 5.0;
/// The smallest card width.
pub const MIN_WIDTH: f32 = 128.0;
/// The largest card width. A longer label is shortened with an ellipsis;
/// a longer port value continues on the next line.
pub const MAX_WIDTH: f32 = 280.0;
/// The most lines of one port value. The last line of a longer value ends
/// with an ellipsis (the inspector and the tooltip show all of it).
pub const MAX_VALUE_LINES: usize = 12;
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
    /// Small chips.
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

/// Breaks `text` into lines that are at most `max_width` wide. A line
/// breaks after a space, `;` or `,` if it can, else between two
/// characters. A new line in the text starts a new line; other control
/// characters show as spaces. The result has at least one line and at most
/// `max_lines`; if text remains, the last line ends with an ellipsis.
/// Returns the lines and the width of the widest line.
pub fn wrap(
    measure: &mut dyn TextMeasure,
    text: &str,
    kind: TextKind,
    max_width: f32,
    max_lines: usize,
) -> (Vec<String>, f32) {
    let max_lines = max_lines.max(1);
    let mut lines: Vec<String> = Vec::new();
    // The text after the last line.
    let mut rest = String::new();
    'paragraphs: for paragraph in text.split('\n') {
        let clean: String = paragraph
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let clean = clean.trim_end();
        if lines.len() == max_lines {
            rest.push(' ');
            rest.push_str(clean);
            continue;
        }
        let mut start = 0;
        let mut width = 0.0;
        let mut last_break: Option<usize> = None;
        for (i, c) in clean.char_indices() {
            if i < start {
                // A space after a break.
                continue;
            }
            let w = measure.char_width(c, kind);
            if width + w > max_width && i > start {
                let cut = match last_break {
                    // A space can stay at the end of the line.
                    _ if c == ' ' => i,
                    Some(b) if b > start => b,
                    _ => i,
                };
                lines.push(clean[start..cut].trim_end().to_owned());
                start = cut + (clean[cut..].len() - clean[cut..].trim_start().len());
                last_break = None;
                if lines.len() == max_lines {
                    rest = clean[start..].to_owned();
                    continue 'paragraphs;
                }
                if i < start {
                    width = 0.0;
                    continue;
                }
                width = measure.width(&clean[start..i], kind);
            }
            width += w;
            if matches!(c, ' ' | ';' | ',') {
                last_break = Some(i + c.len_utf8());
            }
        }
        lines.push(clean[start..].to_owned());
    }
    if !rest.trim().is_empty()
        && let Some(last) = lines.last_mut()
    {
        let joined = format!("{last} {}", rest.trim());
        *last = elide(measure, &joined, kind, max_width).0;
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    let widest = lines
        .iter()
        .map(|line| measure.width(line, kind))
        .fold(0.0, f32::max);
    (lines, widest)
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

    // All ports and all conditions show. A long value continues on the
    // next lines.
    let mut ports_w: f32 = 0.0;
    let ports = measure_entries(&node.ports, measure, &mut ports_w);
    let conditions = measure_entries(&node.conditions, measure, &mut ports_w);
    let ports_needed = BAND + 2.0 * PAD_X + ports_w;

    let width = header_w.max(ports_needed).clamp(MIN_WIDTH, MAX_WIDTH);
    let mut card = CardText {
        label,
        type_line,
        ports,
        conditions,
        size: Vec2::ZERO,
    };
    card.size = vec2(width.round(), CardGeometry::height_of(&card));
    card
}

/// Shortens the names and wraps the values of ports or conditions.
/// `widest` grows to the width of the widest line.
fn measure_entries(
    lines: &[PortLine],
    measure: &mut dyn TextMeasure,
    widest: &mut f32,
) -> Vec<CardEntry> {
    let max_port = MAX_WIDTH - BAND - 2.0 * PAD_X;
    let separator_w = measure.width(PORT_SEPARATOR, TextKind::PortName);
    lines
        .iter()
        .map(|line| {
            let (name, name_w) = elide(measure, &line.name, TextKind::PortName, max_port * 0.45);
            let (value, value_w) = wrap(
                measure,
                &line.value,
                TextKind::PortValue,
                max_port - name_w - separator_w,
                MAX_VALUE_LINES,
            );
            *widest = widest.max(name_w + separator_w + value_w);
            CardEntry { name, value }
        })
        .collect()
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
    /// The number of text lines of the ports.
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
            port_lines: card.port_line_count(),
            condition_lines: card.condition_line_count(),
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
            self.condition_box_bottom() + CONDITION_GAP
        } else if self.port_lines == 0 {
            self.header_height
        } else {
            self.ports_bottom() + PAD_BOTTOM - 2.0
        }
    }

    /// The y position below the last port line.
    fn ports_bottom(&self) -> f32 {
        self.header_height + PORT_GAP + self.port_lines as f32 * PORT_LINE - 1.0
    }

    /// The top of the condition box.
    fn condition_box_top(&self) -> f32 {
        if self.port_lines == 0 {
            self.header_height - 2.0
        } else {
            self.ports_bottom() + CONDITION_GAP
        }
    }

    /// The bottom of the condition box.
    fn condition_box_bottom(&self) -> f32 {
        self.condition_top() + self.condition_lines as f32 * PORT_LINE + CONDITION_PAD_Y
    }

    /// The y position of the first condition line.
    fn condition_top(&self) -> f32 {
        self.condition_box_top() + CONDITION_PAD_Y
    }

    /// The box around the conditions, for a card of `width`.
    pub fn condition_box(&self, width: f32) -> Rect {
        Rect::from_min_max(
            pos2(BAND + PAD_X - CONDITION_INSET, self.condition_box_top()),
            pos2(width - PAD_X + CONDITION_INSET, self.condition_box_bottom()),
        )
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
    fn conditions_are_in_a_box_below_the_ports() {
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
        assert!(card.size.y >= plain.size.y + 2.0 * PORT_LINE);
        let geometry = CardGeometry::of(&card);
        let boxed = geometry.condition_box(card.size.x);
        // The box is below the ports and inside the card.
        assert!(boxed.min.y > geometry.port_pos(0).y + PORT_LINE - 0.5);
        assert!(boxed.min.x > BAND && boxed.max.x < card.size.x);
        assert!(boxed.max.y < card.size.y);
        // The lines are inside the box, and the text column is inside it.
        for line in 0..2 {
            let top = geometry.condition_pos(line);
            assert!(boxed.contains(top) && boxed.contains(top + vec2(0.0, PORT_LINE)));
        }
        assert!(geometry.condition_pos(0).x - boxed.min.x >= CONDITION_INSET);
        assert!(boxed.max.x - (card.size.x - PAD_X) >= CONDITION_INSET);
        // Without ports, the box comes after the header.
        let mut only = node("n", "T", 0);
        only.conditions = with.conditions.clone();
        let card = measure_node(&only, &mut m);
        let geometry = CardGeometry::of(&card);
        let boxed = geometry.condition_box(card.size.x);
        assert!(boxed.min.y >= geometry.header_height - 2.0);
        assert!(boxed.max.y < card.size.y);
    }

    #[test]
    fn a_long_script_wraps_at_the_statements() {
        let mut m = FixedMeasure(7.0);
        let mut n = node("Script", "Script", 1);
        n.ports[0].name = "code".into();
        n.ports[0].value =
            "crate1:=0;crate2:=0;crate3:=0;crate4:=0;@magazine:=0;@dynamic:=0".into();
        let card = measure_node(&n, &mut m);
        let lines = &card.ports[0].value;
        assert!(lines.len() > 1, "{lines:?}");
        // No text is lost, and each line ends at a statement.
        assert_eq!(lines.concat(), n.ports[0].value);
        for line in &lines[..lines.len() - 1] {
            assert!(line.ends_with(';'), "{lines:?}");
        }
        assert_eq!(
            card.size.y - measure_node(&node("Script", "Script", 1), &mut m).size.y,
            (lines.len() - 1) as f32 * PORT_LINE
        );
    }

    #[test]
    fn wrap_breaks_at_spaces_and_new_lines() {
        let mut m = FixedMeasure(10.0);
        let (lines, widest) = wrap(&mut m, "aaa bbb ccc", TextKind::PortValue, 75.0, 10);
        assert_eq!(lines, ["aaa bbb", "ccc"]);
        assert_eq!(widest, 70.0);
        // Without a break place, a line breaks between two characters.
        let (lines, _) = wrap(&mut m, "abcdefghij", TextKind::PortValue, 40.0, 10);
        assert_eq!(lines, ["abcd", "efgh", "ij"]);
        // A new line starts a new line; other control characters are spaces.
        let (lines, _) = wrap(&mut m, "a := 1\nb\t:= 2", TextKind::PortValue, 200.0, 10);
        assert_eq!(lines, ["a := 1", "b := 2"]);
        // Spaces at a break do not start the next line.
        let (lines, _) = wrap(&mut m, "abc    def", TextKind::PortValue, 40.0, 10);
        assert_eq!(lines, ["abc", "def"]);
        // An empty value has one empty line.
        assert_eq!(wrap(&mut m, "", TextKind::PortValue, 40.0, 10).0, [""]);
        // The line limit: the last line ends with an ellipsis.
        let (lines, _) = wrap(&mut m, "aaaa bbbb cccc dddd", TextKind::PortValue, 40.0, 2);
        assert_eq!(lines, ["aaaa".to_owned(), format!("bbb{ELLIPSIS}")]);
        let (lines, _) = wrap(&mut m, "a\nb\nc", TextKind::PortValue, 100.0, 2);
        assert_eq!(lines, ["a", "b c"]);
        // Multi-byte characters.
        let (lines, _) = wrap(&mut m, "ééééé", TextKind::PortValue, 30.0, 10);
        assert_eq!(lines, ["ééé", "éé"]);
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
        let geometry = CardGeometry::of(&with_ports);
        assert_eq!(geometry.port_lines, 2);
        assert!(with_ports.size.y > named.size.y);

        // All ports show.
        let many = measure_node(&node("n", "T", 7), &mut m);
        assert_eq!(many.ports.len(), 7);
        assert_eq!(many.ports[6].name, "port6");
        assert_eq!(many.ports[6].value, ["{value_6}"]);
        assert_eq!(CardGeometry::of(&many).port_lines, 7);
        assert_eq!(many.size.y - with_ports.size.y, 5.0 * PORT_LINE);

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
        let entry = &card.ports[0];
        // The value wraps to the line limit and ends with an ellipsis.
        assert_eq!(entry.value.len(), MAX_VALUE_LINES);
        assert!(entry.value.last().unwrap().ends_with(ELLIPSIS));
        assert_eq!(geometry.port_lines, MAX_VALUE_LINES);
        let value_x = geometry.port_pos(0).x
            + m.width(&entry.name, TextKind::PortName)
            + m.width(PORT_SEPARATOR, TextKind::PortName);
        for line in &entry.value {
            let end = value_x + m.width(line, TextKind::PortValue);
            assert!(end <= card.size.x - PAD_X + 0.5, "{end} > {}", card.size.x);
        }
        assert!(geometry.port_pos(0).y > geometry.separator_y());
        assert!(geometry.port_pos(MAX_VALUE_LINES - 1).y + PORT_LINE <= card.size.y);
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
