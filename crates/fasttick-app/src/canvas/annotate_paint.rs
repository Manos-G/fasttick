//! Painting of group boxes and canvas notes (see [`super::annotate`]).

use egui::epaint::CornerRadiusF32;
use egui::{Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, vec2};

use super::annotate::{GroupBox, NoteBox, TEXT_LINE, TEXT_PAD, TITLE_LINE};
use crate::theme::{CanvasPalette, mix};

/// The base color of an annotation color name (see
/// `fasttick_core::bt::annotations::ANNOTATION_COLORS`).
pub fn base_color(name: Option<&str>) -> Option<Color32> {
    Some(match name? {
        "yellow" => Color32::from_rgb(0xE2, 0xB3, 0x2E),
        "blue" => Color32::from_rgb(0x4C, 0x8D, 0xD6),
        "green" => Color32::from_rgb(0x48, 0xA8, 0x68),
        "orange" => Color32::from_rgb(0xE8, 0x87, 0x3A),
        "red" => Color32::from_rgb(0xD9, 0x53, 0x4F),
        "purple" => Color32::from_rgb(0x94, 0x68, 0xD0),
        "gray" => Color32::from_rgb(0x8A, 0x90, 0x99),
        _ => return None,
    })
}

/// A color with an alpha value.
fn tint(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// The colors of a group box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupColors {
    /// The fill of the box.
    pub fill: Color32,
    /// The fill of the header.
    pub header: Color32,
    /// The border.
    pub border: Color32,
}

/// The colors of a group with a color name (`None`: the accent color).
pub fn group_colors(name: Option<&str>, palette: &CanvasPalette) -> GroupColors {
    let base = base_color(name).unwrap_or(palette.accent);
    let (fill, header) = if palette.dark { (26, 52) } else { (18, 40) };
    GroupColors {
        fill: tint(base, fill),
        header: tint(base, header),
        border: tint(base, 170),
    }
}

/// The fill and the border of a canvas note (`None`: yellow).
pub fn note_colors(name: Option<&str>, palette: &CanvasPalette) -> (Color32, Color32) {
    let base = base_color(name.or(Some("yellow"))).unwrap_or(palette.accent);
    let amount = if palette.dark { 0.26 } else { 0.32 };
    (
        mix(palette.card, base, amount),
        mix(palette.card, base, 0.75),
    )
}

/// The fonts and the scale of the annotation text of one frame.
#[derive(Debug, Clone)]
pub struct AnnotationFonts {
    /// The title font at the current zoom.
    pub title: FontId,
    /// The description and note font at the current zoom.
    pub text: FontId,
    /// The zoom.
    pub zoom: f32,
}

impl AnnotationFonts {
    /// True if the text is large enough to read.
    pub fn readable(&self) -> bool {
        self.text.size >= 5.5
    }
}

/// Small squares on the corners and the sides of a selected box.
fn handles(shapes: &mut Vec<Shape>, rect: Rect, palette: &CanvasPalette) {
    let size = vec2(7.0, 7.0);
    let points = [
        rect.left_top(),
        rect.center_top(),
        rect.right_top(),
        rect.left_center(),
        rect.right_center(),
        rect.left_bottom(),
        rect.center_bottom(),
        rect.right_bottom(),
    ];
    for p in points {
        let handle = Rect::from_center_size(p, size);
        shapes.push(Shape::rect_filled(
            handle,
            CornerRadius::same(1),
            palette.card,
        ));
        shapes.push(Shape::rect_stroke(
            handle,
            CornerRadius::same(1),
            Stroke::new(1.2, palette.accent),
            StrokeKind::Inside,
        ));
    }
}

/// Paints a group box. `screen` is the box on the screen; `selected` adds
/// the resize handles.
#[allow(clippy::too_many_arguments)]
pub fn paint_group(
    shapes: &mut Vec<Shape>,
    painter: &Painter,
    group: &GroupBox,
    screen: Rect,
    fonts: &AnnotationFonts,
    palette: &CanvasPalette,
    selected: bool,
    hovered: bool,
) {
    let zoom = fonts.zoom;
    let colors = group_colors(group.color.as_deref(), palette);
    let r = (10.0 * zoom).clamp(2.0, 14.0);
    shapes.push(Shape::rect_filled(
        screen,
        CornerRadius::from(r),
        colors.fill,
    ));
    let header_height = group.header.height() * zoom;
    let header = Rect::from_min_size(screen.min, vec2(screen.width(), header_height));
    let header_radius = CornerRadiusF32 {
        nw: r,
        ne: r,
        sw: 0.0,
        se: 0.0,
    };
    shapes.push(Shape::rect_filled(header, header_radius, colors.header));
    let border = if selected {
        Stroke::new(2.0, palette.accent)
    } else if hovered {
        Stroke::new(1.6, colors.border)
    } else {
        Stroke::new(1.0, colors.border)
    };
    shapes.push(Shape::rect_stroke(
        screen,
        CornerRadius::from(r),
        border,
        StrokeKind::Inside,
    ));
    if fonts.readable() {
        let left = screen.min.x + TEXT_PAD * zoom;
        let mut y = screen.min.y + TEXT_PAD * zoom;
        let title = painter.layout_no_wrap(group.title.clone(), fonts.title.clone(), palette.text);
        let title_y = y + (TITLE_LINE * zoom - title.size().y) * 0.5;
        shapes.push(Shape::galley(Pos2::new(left, title_y), title, palette.text));
        y += TITLE_LINE * zoom + 2.0 * zoom;
        for line in &group.lines {
            let galley =
                painter.layout_no_wrap(line.clone(), fonts.text.clone(), palette.text_weak);
            shapes.push(Shape::galley(Pos2::new(left, y), galley, palette.text_weak));
            y += TEXT_LINE * zoom;
        }
    }
    if selected {
        handles(shapes, screen, palette);
    }
}

/// Paints the outline of a group box that the user drags (`area`) and the
/// box that it snaps to (`snapped`, if it has members).
pub fn paint_group_drag(
    shapes: &mut Vec<Shape>,
    area: Rect,
    snapped: Option<Rect>,
    zoom: f32,
    palette: &CanvasPalette,
) {
    let r = (10.0 * zoom).clamp(2.0, 14.0);
    if let Some(snapped) = snapped {
        shapes.push(Shape::rect_filled(
            snapped,
            CornerRadius::from(r),
            tint(palette.accent, 28),
        ));
        shapes.push(Shape::rect_stroke(
            snapped,
            CornerRadius::from(r),
            Stroke::new(1.5, tint(palette.accent, 200)),
            StrokeKind::Inside,
        ));
    }
    super::paint::dashed_rect(shapes, area, Stroke::new(1.5, palette.accent), 6.0, 4.0);
}

/// Paints a canvas note. `placeholder` is the text of an empty note.
#[allow(clippy::too_many_arguments)]
pub fn paint_note(
    shapes: &mut Vec<Shape>,
    painter: &Painter,
    note: &NoteBox,
    screen: Rect,
    fonts: &AnnotationFonts,
    palette: &CanvasPalette,
    selected: bool,
    editing: bool,
) {
    let zoom = fonts.zoom;
    let (fill, border) = note_colors(note.color.as_deref(), palette);
    let r = (4.0 * zoom).clamp(1.0, 6.0);
    // Shadow, fill, folded corner, border.
    shapes.push(Shape::rect_filled(
        screen.translate(vec2(0.0, 2.0 * zoom.min(1.5))),
        CornerRadius::from(r),
        palette.shadow,
    ));
    shapes.push(Shape::rect_filled(screen, CornerRadius::from(r), fill));
    let fold = (12.0 * zoom)
        .min(screen.width() * 0.3)
        .min(screen.height() * 0.3);
    if fold >= 3.0 {
        let corner = screen.right_top();
        shapes.push(Shape::convex_polygon(
            vec![
                corner + vec2(-fold, 0.0),
                corner + vec2(0.0, fold),
                corner + vec2(-fold, fold),
            ],
            border,
            Stroke::NONE,
        ));
    }
    let stroke = if selected {
        Stroke::new(2.0, palette.accent)
    } else {
        Stroke::new(1.0, border)
    };
    shapes.push(Shape::rect_stroke(
        screen,
        CornerRadius::from(r),
        stroke,
        StrokeKind::Inside,
    ));
    if fonts.readable() && !editing {
        let left = screen.min.x + TEXT_PAD * zoom;
        let mut y = screen.min.y + TEXT_PAD * zoom;
        let empty = note.lines.iter().all(String::is_empty);
        if empty {
            let galley = painter.layout_no_wrap(
                "Double-click to write".to_owned(),
                fonts.text.clone(),
                palette.text_weak,
            );
            shapes.push(Shape::galley(Pos2::new(left, y), galley, palette.text_weak));
        } else {
            for line in &note.lines {
                if y + TEXT_LINE * zoom > screen.max.y + 1.0 {
                    break;
                }
                let galley = painter.layout_no_wrap(line.clone(), fonts.text.clone(), palette.text);
                shapes.push(Shape::galley(Pos2::new(left, y), galley, palette.text));
                y += TEXT_LINE * zoom;
            }
        }
    }
    if selected {
        handles(shapes, screen, palette);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        for name in fasttick_core::bt::annotations::ANNOTATION_COLORS {
            assert!(base_color(Some(name)).is_some(), "{name}");
        }
        assert_eq!(base_color(Some("pink")), None);
        assert_eq!(base_color(None), None);
        let palette = crate::theme::CanvasPalette::light();
        let colors = group_colors(None, &palette);
        assert_eq!(colors.border.a(), 170);
        let (fill, border) = note_colors(None, &palette);
        assert_ne!(fill, border);
        assert_eq!(fill.a(), 255);
    }
}
