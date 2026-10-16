//! Export of a canvas tree as an SVG image.
//!
//! The image shows the cards and edges as the canvas draws them at zoom 1:
//! the category band and glyph, the label, the type line, the ports, the
//! condition box, the statuses (live and replay) and the chips of collapsed
//! SubTrees and folded nodes, the groups, the canvas notes and the marks of
//! node notes. It does not show the selection, the search matches,
//! breakpoints or problem markers: they are editor state, not part of the
//! tree.
//!
//! The glyphs and badges come from [`super::paint`], as egui shapes; this
//! module writes those shapes as SVG elements. The text uses the font
//! names of the bundled fonts (Roboto and a monospace font), with generic
//! fallbacks.

use std::fmt::Write as _;

use egui::epaint::{ColorMode, PathStroke};
use egui::{Color32, CornerRadius, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};
use fasttick_core::status::NodeStatus;

use super::annotate::{GroupBox, NoteBox, TEXT_LINE, TEXT_PAD, TITLE_LINE};
use super::annotate_paint;
use super::card::{self, CardGeometry, TextKind, TextMeasure};
use super::layout::{LayoutDirection, TreeLayout};
use super::model::{CanvasNodeId, CanvasTree, CardEntry, NodeCategory};
use super::paint::{self, EdgeStyle};
use crate::theme::{CanvasPalette, mix};

/// The free space around the tree.
const MARGIN: f32 = 32.0;

/// The font of labels.
const LABEL_FONT: &str = "Roboto, 'Helvetica Neue', Arial, sans-serif";
/// The font of type lines, port names and chips.
const TEXT_FONT: &str = "Roboto, 'Helvetica Neue', Arial, sans-serif";
/// The font of port values.
const MONO_FONT: &str = "'DejaVu Sans Mono', 'Liberation Mono', Consolas, monospace";

/// The groups and the canvas notes of an SVG image.
#[derive(Debug, Clone, Copy, Default)]
pub struct Annotations<'a> {
    /// The group boxes, the largest first.
    pub groups: &'a [GroupBox],
    /// The canvas notes.
    pub notes: &'a [NoteBox],
}

/// Writes the tree as an SVG document. `measure` must measure text as the
/// canvas did when it measured the cards (the continuation lines of a value
/// start below its first character).
pub fn tree_svg(
    tree: &CanvasTree,
    layout: &TreeLayout,
    edge_style: EdgeStyle,
    palette: &CanvasPalette,
    measure: &mut dyn TextMeasure,
    annotations: Annotations<'_>,
) -> String {
    let bounds = if layout.bounds.is_positive() {
        layout.bounds
    } else {
        Rect::from_min_size(Pos2::ZERO, vec2(200.0, 100.0))
    };
    // Room for the chips below the cards and the stacked SubTree cards.
    let view = Rect::from_min_max(
        bounds.min - Vec2::splat(MARGIN),
        bounds.max + Vec2::splat(MARGIN),
    );
    let mut out = String::new();
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{x} {y} {w} {h}">"#,
        x = num(view.min.x),
        y = num(view.min.y),
        w = num(view.width()),
        h = num(view.height()),
    );
    let _ = writeln!(out, "<title>{}</title>", escape(&tree.title));
    write_shapes(
        &mut out,
        &[Shape::rect_filled(view, 0.0, palette.background)],
    );
    for group in annotations.groups {
        group_svg(&mut out, group, palette);
    }
    let mut shapes = Vec::new();
    edges(tree, layout, edge_style, palette, &mut shapes);
    write_shapes(&mut out, &shapes);
    for index in 0..tree.nodes().len() {
        let Some(rect) = layout.rect(CanvasNodeId(index as u32)) else {
            continue;
        };
        node_svg(
            &mut out,
            tree,
            index,
            rect,
            layout.direction,
            palette,
            measure,
        );
        if tree.nodes()[index].note.is_some() {
            note_mark_svg(&mut out, rect, palette);
        }
    }
    for note in annotations.notes {
        note_svg(&mut out, note, palette);
    }
    out.push_str("</svg>\n");
    out
}

/// A group box with its header.
fn group_svg(out: &mut String, group: &GroupBox, palette: &CanvasPalette) {
    let colors = annotate_paint::group_colors(group.color.as_deref(), palette);
    let header = CornerRadius {
        nw: 10,
        ne: 10,
        sw: 0,
        se: 0,
    };
    write_shapes(
        out,
        &[
            Shape::rect_filled(group.rect, 10.0, colors.fill),
            Shape::rect_filled(group.header, header, colors.header),
            Shape::rect_stroke(
                group.rect,
                10.0,
                Stroke::new(1.0, colors.border),
                StrokeKind::Inside,
            ),
        ],
    );
    let left = group.rect.min.x + TEXT_PAD;
    let mut middle = group.rect.min.y + TEXT_PAD + TITLE_LINE * 0.5;
    text_line(
        out,
        pos2(left, middle),
        &[Span::new(&group.title, TextKind::Label, palette.text)],
    );
    middle += TITLE_LINE * 0.5 + 2.0 + TEXT_LINE * 0.5;
    for line in &group.lines {
        text_line(
            out,
            pos2(left, middle),
            &[Span::new(line, TextKind::Type, palette.text_weak)],
        );
        middle += TEXT_LINE;
    }
}

/// A canvas note.
fn note_svg(out: &mut String, note: &NoteBox, palette: &CanvasPalette) {
    let (fill, border) = annotate_paint::note_colors(note.color.as_deref(), palette);
    let rect = note.rect;
    let fold = 12.0_f32.min(rect.width() * 0.3).min(rect.height() * 0.3);
    let corner = rect.right_top();
    write_shapes(
        out,
        &[
            Shape::rect_filled(rect, 4.0, fill),
            Shape::convex_polygon(
                vec![
                    corner + vec2(-fold, 0.0),
                    corner + vec2(0.0, fold),
                    corner + vec2(-fold, fold),
                ],
                border,
                Stroke::NONE,
            ),
            Shape::rect_stroke(rect, 4.0, Stroke::new(1.0, border), StrokeKind::Inside),
        ],
    );
    let left = rect.min.x + TEXT_PAD;
    let mut middle = rect.min.y + TEXT_PAD + TEXT_LINE * 0.5;
    for line in &note.lines {
        text_line(
            out,
            pos2(left, middle),
            &[Span::new(line, TextKind::Type, palette.text)],
        );
        middle += TEXT_LINE;
    }
}

/// The mark of a node with a note, on the top side of its card.
fn note_mark_svg(out: &mut String, card: Rect, palette: &CanvasPalette) {
    let (fill, border) = annotate_paint::note_colors(None, palette);
    let mark = Rect::from_center_size(pos2(card.max.x - 20.0, card.min.y + 1.0), vec2(10.0, 11.0));
    let line = Stroke::new(1.0, palette.text_weak);
    let lines = (1..=2).map(|i| {
        let y = mark.min.y + mark.height() * (i as f32) / 3.0;
        Shape::line_segment([pos2(mark.min.x + 2.0, y), pos2(mark.max.x - 2.0, y)], line)
    });
    let mut shapes = vec![
        Shape::rect_filled(mark, 1.0, fill),
        Shape::rect_stroke(mark, 1.0, Stroke::new(1.0, border), StrokeKind::Inside),
    ];
    shapes.extend(lines);
    write_shapes(out, &shapes);
}

/// The edges, as the canvas draws them at zoom 1.
fn edges(
    tree: &CanvasTree,
    layout: &TreeLayout,
    edge_style: EdgeStyle,
    palette: &CanvasPalette,
    shapes: &mut Vec<Shape>,
) {
    let vertical = layout.direction == LayoutDirection::TopDown;
    let mut active = Vec::new();
    let mut points = Vec::new();
    for (index, parent) in tree.nodes().iter().enumerate() {
        let Some(parent_rect) = layout.rect(CanvasNodeId(index as u32)) else {
            continue;
        };
        let start = layout.edge_start(parent_rect);
        let bus = layout.bus_of(CanvasNodeId(index as u32), parent.depth);
        for child in &parent.children {
            let Some(child_rect) = layout.rect(*child) else {
                continue;
            };
            let end = layout.edge_end(child_rect);
            let (color, rank) = match tree.nodes()[child.index()].deco.status {
                NodeStatus::Running => (palette.running, 3),
                NodeStatus::Failure => (palette.failure, 2),
                NodeStatus::Success => (palette.success, 1),
                NodeStatus::Idle | NodeStatus::Skipped => (palette.edge, 0),
            };
            let stroke = Stroke::new(if rank > 0 { 2.2 } else { 1.6 }, color);
            let shape = match edge_style {
                EdgeStyle::Curved => paint::curved_edge(start, end, vertical, stroke),
                EdgeStyle::Orthogonal => {
                    let bus = bus.unwrap_or(if vertical {
                        (start.y + end.y) * 0.5
                    } else {
                        (start.x + end.x) * 0.5
                    });
                    paint::orthogonal_edge_points(start, end, bus, vertical, 8.0, &mut points);
                    Shape::line(points.clone(), stroke)
                }
            };
            if rank > 0 {
                active.push((shape, rank));
            } else {
                shapes.push(shape);
            }
        }
    }
    active.sort_by_key(|(_, rank)| *rank);
    shapes.extend(active.into_iter().map(|(shape, _)| shape));
}

/// One card with its text and chips.
fn node_svg(
    out: &mut String,
    tree: &CanvasTree,
    index: usize,
    rect: Rect,
    direction: LayoutDirection,
    palette: &CanvasPalette,
    measure: &mut dyn TextMeasure,
) {
    let node = &tree.nodes()[index];
    let deco = node.deco;
    let geometry = CardGeometry::of(&node.card);
    let category_color = palette.category(node.category);
    let skipped = deco.status == NodeStatus::Skipped;
    let alpha = if skipped { 0.55 } else { 1.0 };
    let r = CornerRadius::same(6);
    let mut shapes = Vec::new();

    if node.is_collapsed_subtree() {
        for step in [2.0, 1.0] {
            let behind = rect.translate(vec2(4.0, 4.0) * step);
            shapes.push(Shape::rect_filled(
                behind,
                r,
                palette.card.gamma_multiply(alpha),
            ));
            shapes.push(Shape::rect_stroke(
                behind,
                r,
                Stroke::new(1.0, palette.card_border.gamma_multiply(alpha)),
                StrokeKind::Inside,
            ));
        }
    }
    let fill = if deco.status == NodeStatus::Running {
        mix(
            palette.card,
            palette.running,
            if palette.dark { 0.12 } else { 0.08 },
        )
    } else {
        palette.card
    };
    shapes.push(Shape::rect_filled(
        rect.translate(vec2(0.0, 1.5)).expand(0.5),
        r,
        palette.shadow.gamma_multiply(alpha),
    ));
    shapes.push(Shape::rect_filled(rect, r, fill.gamma_multiply(alpha)));
    shapes.push(Shape::rect_filled(
        Rect::from_min_max(rect.min, pos2(rect.min.x + card::BAND, rect.max.y)),
        CornerRadius {
            nw: r.nw,
            sw: r.sw,
            ne: 0,
            se: 0,
        },
        category_color.gamma_multiply(alpha),
    ));
    let border = match (palette.status(deco.status), deco.status) {
        (Some(color), NodeStatus::Running) => Stroke::new(2.0, color),
        (Some(color), NodeStatus::Skipped) => Stroke::new(1.2, color),
        (Some(color), _) => Stroke::new(1.6, color),
        (None, _) => Stroke::new(1.0, palette.card_border),
    };
    if skipped {
        paint::dashed_rect(&mut shapes, rect, border, 4.0, 3.0);
    } else {
        shapes.push(Shape::rect_stroke(rect, r, border, StrokeKind::Inside));
    }

    let local = |p: Pos2| rect.min + p.to_vec2();
    let glyph_center = local(geometry.glyph_center());
    shapes.push(Shape::rect_filled(
        Rect::from_center_size(glyph_center, Vec2::splat(card::GLYPH)),
        CornerRadius::same(4),
        mix(
            palette.card,
            category_color,
            if palette.dark { 0.22 } else { 0.14 },
        )
        .gamma_multiply(alpha),
    ));
    paint::category_glyph(
        &mut shapes,
        glyph_center,
        card::GLYPH,
        node.category,
        category_color.gamma_multiply(alpha),
    );
    if geometry.port_lines > 0 {
        shapes.push(Shape::hline(
            (rect.min.x + card::BAND)..=(rect.max.x - 1.0),
            local(pos2(0.0, geometry.separator_y())).y,
            Stroke::new(1.0, palette.separator.gamma_multiply(alpha)),
        ));
    }
    if geometry.condition_lines > 0 {
        let boxed = geometry.condition_box(node.card.size.x);
        let boxed = Rect::from_min_max(local(boxed.min), local(boxed.max));
        let fill = mix(
            palette.card,
            palette.condition,
            if palette.dark { 0.17 } else { 0.10 },
        );
        shapes.push(Shape::rect_filled(
            boxed,
            CornerRadius::same(4),
            fill.gamma_multiply(alpha),
        ));
        shapes.push(Shape::rect_stroke(
            boxed,
            CornerRadius::same(4),
            Stroke::new(1.4, palette.condition.gamma_multiply(alpha)),
            StrokeKind::Inside,
        ));
    }
    let badge = local(geometry.badge_center(node.card.size.x));
    match (deco.status, deco.previous_status) {
        (NodeStatus::Idle, Some(previous)) => {
            paint::status_badge(&mut shapes, badge, card::BADGE, previous, true, palette);
        }
        (status, _) => {
            paint::status_badge(&mut shapes, badge, card::BADGE, status, false, palette);
        }
    }
    write_shapes(out, &shapes);

    // Text.
    let text = palette.text.gamma_multiply(alpha);
    let weak = palette.text_weak.gamma_multiply(alpha);
    let has_type = node.card.type_line.is_some();
    let label_top = local(geometry.label_pos(has_type));
    text_line(
        out,
        pos2(label_top.x, label_top.y + card::LABEL_LINE * 0.5),
        &[Span::new(&node.card.label, TextKind::Label, text)],
    );
    if let Some(type_line) = &node.card.type_line {
        let top = local(geometry.type_pos());
        text_line(
            out,
            pos2(top.x, top.y + card::TYPE_LINE * 0.5),
            &[Span::new(type_line, TextKind::Type, weak)],
        );
    }
    let colors = EntryColors {
        name: weak,
        value: palette.text_value.gamma_multiply(alpha),
        reference: palette.text_reference.gamma_multiply(alpha),
    };
    entries(out, &node.card.ports, colors, measure, |line| {
        local(geometry.port_pos(line))
    });
    let colors = EntryColors {
        name: palette.condition.gamma_multiply(alpha),
        reference: colors.value,
        ..colors
    };
    entries(out, &node.card.conditions, colors, measure, |line| {
        local(geometry.condition_pos(line))
    });

    // Chips: the size of a collapsed SubTree, the hidden nodes of a folded
    // node.
    if let Some(info) = node
        .subtree
        .as_ref()
        .filter(|_| node.is_collapsed_subtree())
    {
        let center = match direction {
            LayoutDirection::TopDown => pos2(rect.center().x, rect.max.y + 4.0),
            LayoutDirection::LeftRight => pos2(rect.max.x + 6.0, rect.center().y),
        };
        chip(
            out,
            center,
            &format!("+{}", info.inner_count),
            palette.category(NodeCategory::SubTree),
            palette.on_color,
            measure,
        );
    }
    if let Some(hidden) = node.folded {
        let center = match direction {
            LayoutDirection::TopDown => pos2(rect.center().x, rect.max.y + 17.0),
            LayoutDirection::LeftRight => pos2(rect.max.x + 22.0, rect.center().y),
        };
        chip(
            out,
            center,
            &format!("+{hidden}"),
            category_color.gamma_multiply(alpha),
            palette.on_color,
            measure,
        );
    }
}

/// The colours of a port or condition line.
#[derive(Clone, Copy)]
struct EntryColors {
    name: Color32,
    value: Color32,
    reference: Color32,
}

/// The lines of ports or conditions. `top` gives the top-left corner of a
/// line. The next lines of a value start below its first character.
fn entries(
    out: &mut String,
    entries: &[CardEntry],
    colors: EntryColors,
    measure: &mut dyn TextMeasure,
    top: impl Fn(usize) -> Pos2,
) {
    let mut line = 0;
    for entry in entries {
        let name = format!("{}{}", entry.name, card::PORT_SEPARATOR);
        let indent = measure.width(&name, TextKind::PortName);
        for part in 0..entry.line_count() {
            let value = entry.value.get(part).map_or("", String::as_str);
            let reference =
                entry.value.len() == 1 && value.starts_with('{') && value.ends_with('}');
            let value_color = if reference {
                colors.reference
            } else {
                colors.value
            };
            let corner = top(line);
            let middle = corner.y + card::PORT_LINE * 0.5;
            if part == 0 {
                text_line(
                    out,
                    pos2(corner.x, middle),
                    &[
                        Span::new(&name, TextKind::PortName, colors.name),
                        Span::new(value, TextKind::PortValue, value_color),
                    ],
                );
            } else {
                text_line(
                    out,
                    pos2(corner.x + indent, middle),
                    &[Span::new(value, TextKind::PortValue, value_color)],
                );
            }
            line += 1;
        }
    }
}

/// A small pill with a text.
fn chip(
    out: &mut String,
    center: Pos2,
    text: &str,
    fill: Color32,
    color: Color32,
    measure: &mut dyn TextMeasure,
) {
    let size = vec2(
        measure.width(text, TextKind::Note) + 10.0,
        TextKind::Note.size() + 4.0,
    );
    let rect = Rect::from_center_size(center, size);
    write_shapes(
        out,
        &[Shape::rect_filled(
            rect,
            CornerRadius::same((size.y * 0.5) as u8),
            fill,
        )],
    );
    let span = Span::new(text, TextKind::Note, color);
    let _ = writeln!(
        out,
        r#"<text x="{}" y="{}" text-anchor="middle" {}>{}</text>"#,
        num(center.x),
        num(baseline(center.y, TextKind::Note)),
        span.style(),
        escape(text),
    );
}

/// A part of a text line in one font and colour.
struct Span<'a> {
    text: &'a str,
    kind: TextKind,
    color: Color32,
}

impl<'a> Span<'a> {
    fn new(text: &'a str, kind: TextKind, color: Color32) -> Self {
        Span { text, kind, color }
    }

    /// The font and fill attributes.
    fn style(&self) -> String {
        let (family, weight) = match self.kind {
            TextKind::Label => (LABEL_FONT, "500"),
            TextKind::PortValue => (MONO_FONT, "400"),
            TextKind::Type | TextKind::PortName | TextKind::Note => (TEXT_FONT, "400"),
        };
        format!(
            r#"font-family="{family}" font-size="{}" font-weight="{weight}" {}"#,
            num(self.kind.size()),
            paint_attr("fill", self.color),
        )
    }
}

/// The baseline of a line of `kind` whose middle is at `middle`.
fn baseline(middle: f32, kind: TextKind) -> f32 {
    middle + kind.size() * 0.36
}

/// One line of text whose middle is at `left_middle.y`.
fn text_line(out: &mut String, left_middle: Pos2, spans: &[Span<'_>]) {
    let kind = spans.first().map_or(TextKind::Type, |s| s.kind);
    let _ = write!(
        out,
        r#"<text x="{}" y="{}" xml:space="preserve">"#,
        num(left_middle.x),
        num(baseline(left_middle.y, kind)),
    );
    for span in spans {
        let _ = write!(out, "<tspan {}>{}</tspan>", span.style(), escape(span.text));
    }
    out.push_str("</text>\n");
}

/// Writes egui shapes as SVG elements. Shapes that the canvas does not use
/// (meshes, text, callbacks) are left out.
fn write_shapes(out: &mut String, shapes: &[Shape]) {
    for shape in shapes {
        write_shape(out, shape);
    }
}

fn write_shape(out: &mut String, shape: &Shape) {
    match shape {
        Shape::Vec(shapes) => write_shapes(out, shapes),
        Shape::Circle(circle) => {
            let _ = writeln!(
                out,
                r#"<circle cx="{}" cy="{}" r="{}" {} {}/>"#,
                num(circle.center.x),
                num(circle.center.y),
                num(circle.radius),
                paint_attr("fill", circle.fill),
                stroke_attr(circle.stroke),
            );
        }
        Shape::LineSegment { points, stroke } => {
            let _ = writeln!(
                out,
                r#"<line x1="{}" y1="{}" x2="{}" y2="{}" {} stroke-linecap="round"/>"#,
                num(points[0].x),
                num(points[0].y),
                num(points[1].x),
                num(points[1].y),
                stroke_attr(*stroke),
            );
        }
        Shape::Path(path) => {
            let element = if path.closed { "polygon" } else { "polyline" };
            let points: Vec<String> = path
                .points
                .iter()
                .map(|p| format!("{},{}", num(p.x), num(p.y)))
                .collect();
            let fill = if path.closed {
                path.fill
            } else {
                Color32::TRANSPARENT
            };
            let _ = writeln!(
                out,
                r#"<{element} points="{}" {} {} stroke-linejoin="round" stroke-linecap="round"/>"#,
                points.join(" "),
                paint_attr("fill", fill),
                path_stroke_attr(&path.stroke),
            );
        }
        Shape::CubicBezier(bezier) => {
            let [a, b, c, d] = bezier.points;
            let _ = writeln!(
                out,
                r#"<path d="M{},{} C{},{} {},{} {},{}" fill="none" {}/>"#,
                num(a.x),
                num(a.y),
                num(b.x),
                num(b.y),
                num(c.x),
                num(c.y),
                num(d.x),
                num(d.y),
                path_stroke_attr(&bezier.stroke),
            );
        }
        Shape::Rect(rect) => {
            let width = rect.stroke.width;
            let outline = match rect.stroke_kind {
                StrokeKind::Inside => rect.rect.shrink(width * 0.5),
                StrokeKind::Middle => rect.rect,
                StrokeKind::Outside => rect.rect.expand(width * 0.5),
            };
            if rect.fill.a() > 0 {
                rounded_rect(
                    out,
                    rect.rect,
                    rect.corner_radius,
                    &paint_attr("fill", rect.fill),
                );
            }
            if width > 0.0 && rect.stroke.color.a() > 0 {
                let attrs = format!(r#"fill="none" {}"#, stroke_attr(rect.stroke));
                rounded_rect(out, outline, rect.corner_radius, &attrs);
            }
        }
        _ => {}
    }
}

/// A rectangle with rounded corners: `<rect>` if all corners are the same,
/// else a path.
fn rounded_rect(out: &mut String, rect: Rect, radius: CornerRadius, attrs: &str) {
    let max = rect.width().min(rect.height()) * 0.5;
    let [nw, ne, sw, se] =
        [radius.nw, radius.ne, radius.sw, radius.se].map(|r| f32::from(r).min(max));
    if nw == ne && nw == sw && nw == se {
        let corner = if nw > 0.0 {
            format!(r#" rx="{}""#, num(nw))
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            r#"<rect x="{}" y="{}" width="{}" height="{}"{corner} {attrs}/>"#,
            num(rect.min.x),
            num(rect.min.y),
            num(rect.width()),
            num(rect.height()),
        );
        return;
    }
    let (l, t, r, b) = (rect.min.x, rect.min.y, rect.max.x, rect.max.y);
    let _ = writeln!(
        out,
        r#"<path d="M{},{} H{} Q{},{} {},{} V{} Q{},{} {},{} H{} Q{},{} {},{} V{} Q{},{} {},{} Z" {attrs}/>"#,
        num(l + nw),
        num(t),
        num(r - ne),
        num(r),
        num(t),
        num(r),
        num(t + ne),
        num(b - se),
        num(r),
        num(b),
        num(r - se),
        num(b),
        num(l + sw),
        num(l),
        num(b),
        num(l),
        num(b - sw),
        num(t + nw),
        num(l),
        num(t),
        num(l + nw),
        num(t),
    );
}

/// `name="#rrggbb"` and an opacity attribute if the colour is not opaque;
/// `name="none"` for a transparent colour.
fn paint_attr(name: &str, color: Color32) -> String {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    if a == 0 {
        return format!(r#"{name}="none""#);
    }
    let mut attr = format!(r##"{name}="#{r:02x}{g:02x}{b:02x}""##);
    if a < 255 {
        let _ = write!(attr, r#" {name}-opacity="{}""#, num(f32::from(a) / 255.0));
    }
    attr
}

fn stroke_attr(stroke: Stroke) -> String {
    if stroke.width <= 0.0 || stroke.color.a() == 0 {
        return r#"stroke="none""#.to_owned();
    }
    format!(
        r#"{} stroke-width="{}""#,
        paint_attr("stroke", stroke.color),
        num(stroke.width)
    )
}

fn path_stroke_attr(stroke: &PathStroke) -> String {
    match &stroke.color {
        ColorMode::Solid(color) => stroke_attr(Stroke::new(stroke.width, *color)),
        ColorMode::UV(_) => r#"stroke="none""#.to_owned(),
    }
}

/// A number with at most two decimals, without trailing zeros.
fn num(value: f32) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// Escapes text for XML.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::card::FixedMeasure;
    use crate::canvas::model::{CanvasNode, CanvasTreeBuilder, NodeSource, PortLine};
    use fasttick_core::runtime::NodeIndex;

    fn node(index: usize, label: &str, type_id: &str, category: NodeCategory) -> CanvasNode {
        let mut n = CanvasNode::new(NodeSource::Runtime(NodeIndex(index)), label, type_id);
        n.category = category;
        n
    }

    fn sample() -> (CanvasTree, TreeLayout) {
        let mut b = CanvasTreeBuilder::new();
        let root = b.push(None, node(0, "main", "Sequence", NodeCategory::Control));
        let mut say = node(1, "say <hello>", "SayMessage", NodeCategory::Action);
        say.ports.push(PortLine {
            name: "message".into(),
            value: "{greeting}".into(),
        });
        say.conditions.push(PortLine {
            name: "_skipIf".into(),
            value: "done && count > 2".into(),
        });
        b.push(Some(root), say);
        b.push(Some(root), node(2, "wait", "Sleep", NodeCategory::Action));
        let mut tree = b.finish("Main & more");
        card::measure_tree(&mut tree, &mut FixedMeasure(7.0));
        tree.get_mut(CanvasNodeId(1)).unwrap().deco.status = NodeStatus::Success;
        let layout = crate::canvas::layout::layout(&tree, LayoutDirection::TopDown);
        (tree, layout)
    }

    #[test]
    fn the_svg_has_the_cards_edges_and_escaped_text() {
        let (tree, layout) = sample();
        let svg = tree_svg(
            &tree,
            &layout,
            EdgeStyle::Orthogonal,
            &CanvasPalette::light(),
            &mut FixedMeasure(7.0),
            Annotations::default(),
        );
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains("<title>Main &amp; more</title>"));
        // The text of the cards, escaped.
        assert!(svg.contains(">say &lt;hello&gt;</tspan>"), "{svg}");
        assert!(svg.contains(">SayMessage</tspan>"));
        assert!(svg.contains(">message: </tspan>"));
        assert!(svg.contains(">{greeting}</tspan>"));
        assert!(svg.contains(">done &amp;&amp; count &gt; 2</tspan>"));
        // Two edges (polylines) and the success colour on the edge to the
        // node with SUCCESS.
        let palette = CanvasPalette::light();
        let [r, g, b, _] = palette.success.to_srgba_unmultiplied();
        let success = format!("stroke=\"#{r:02x}{g:02x}{b:02x}\"");
        assert!(
            svg.lines()
                .any(|l| l.starts_with("<polyline") && l.contains(&success)),
            "{svg}"
        );
        // The document is well formed.
        fasttick_core::xml::Document::parse(&svg).expect("valid XML");
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(1.5), "1.5");
        assert_eq!(num(-0.001), "0");
        assert_eq!(num(2.345), "2.35");
    }
}
