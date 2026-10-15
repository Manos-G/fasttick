//! Small widgets of the shell: the segmented mode switcher, painter-drawn
//! icon buttons, the status dot and the logo. All icons are shapes; the
//! bundled font has few symbols.

use egui::{
    Color32, CornerRadius, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
    WidgetInfo, WidgetType, pos2, vec2,
};

/// An icon drawn with shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// A plus sign (zoom in).
    Plus,
    /// A minus sign (zoom out).
    Minus,
    /// Four corners (fit to view).
    Fit,
    /// A tree from top to bottom.
    TopDown,
    /// A tree from left to right.
    LeftRight,
    /// A folder (open).
    Open,
    /// A plug (connect).
    Connect,
    /// Back to the start.
    First,
    /// One step back.
    Previous,
    /// One step forward.
    Next,
    /// To the end.
    Last,
}

/// Draws an icon in a square.
pub fn paint_icon(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    let painter = ui.painter();
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.5;
    let stroke = Stroke::new(1.5, color);
    match icon {
        Icon::Plus => {
            painter.line_segment([c - vec2(s * 0.6, 0.0), c + vec2(s * 0.6, 0.0)], stroke);
            painter.line_segment([c - vec2(0.0, s * 0.6), c + vec2(0.0, s * 0.6)], stroke);
        }
        Icon::Minus => {
            painter.line_segment([c - vec2(s * 0.6, 0.0), c + vec2(s * 0.6, 0.0)], stroke);
        }
        Icon::Fit => {
            let r = s * 0.65;
            let l = s * 0.35;
            for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let corner = c + vec2(dx * r, dy * r);
                painter.line_segment([corner, corner - vec2(dx * l, 0.0)], stroke);
                painter.line_segment([corner, corner - vec2(0.0, dy * l)], stroke);
            }
        }
        Icon::TopDown | Icon::LeftRight => {
            let map = |x: f32, y: f32| -> Pos2 {
                if icon == Icon::TopDown {
                    c + vec2(x, y) * s
                } else {
                    c + vec2(y, x) * s
                }
            };
            let root = map(0.0, -0.6);
            let a = map(-0.55, 0.55);
            let b = map(0.55, 0.55);
            painter.line_segment([root, map(0.0, 0.0)], stroke);
            painter.line_segment([map(-0.55, 0.0), map(0.55, 0.0)], stroke);
            painter.line_segment([map(-0.55, 0.0), a], stroke);
            painter.line_segment([map(0.55, 0.0), b], stroke);
            for p in [root, a, b] {
                painter.circle_filled(p, s * 0.2, color);
            }
        }
        Icon::Open => {
            let body = Rect::from_center_size(c + vec2(0.0, s * 0.1), vec2(s * 1.5, s * 1.0));
            painter.rect_stroke(body, CornerRadius::same(2), stroke, StrokeKind::Middle);
            painter.line_segment(
                [body.left_top() + vec2(0.0, 0.0), body.left_top() + vec2(s * 0.5, -s * 0.3)],
                stroke,
            );
            painter.line_segment(
                [body.left_top() + vec2(s * 0.5, -s * 0.3), body.left_top() + vec2(s * 0.7, 0.0)],
                stroke,
            );
        }
        Icon::Connect => {
            painter.circle_stroke(c, s * 0.45, stroke);
            painter.line_segment([c - vec2(s * 0.9, 0.0), c - vec2(s * 0.45, 0.0)], stroke);
            painter.line_segment([c + vec2(s * 0.45, 0.0), c + vec2(s * 0.9, 0.0)], stroke);
        }
        Icon::First | Icon::Last => {
            let dir = if icon == Icon::Last { 1.0 } else { -1.0 };
            let tri = vec![
                c + vec2(-dir * s * 0.45, -s * 0.5),
                c + vec2(dir * s * 0.35, 0.0),
                c + vec2(-dir * s * 0.45, s * 0.5),
            ];
            painter.add(Shape::convex_polygon(tri, color, Stroke::NONE));
            let x = c.x + dir * s * 0.5;
            painter.line_segment([pos2(x, c.y - s * 0.5), pos2(x, c.y + s * 0.5)], stroke);
        }
        Icon::Previous | Icon::Next => {
            let dir = if icon == Icon::Next { 1.0 } else { -1.0 };
            let tri = vec![
                c + vec2(-dir * s * 0.4, -s * 0.5),
                c + vec2(dir * s * 0.45, 0.0),
                c + vec2(-dir * s * 0.4, s * 0.5),
            ];
            painter.add(Shape::convex_polygon(tri, color, Stroke::NONE));
        }
    }
}

/// A small square button with an icon and a tooltip. The tooltip text is
/// also the accessibility label.
pub fn icon_button(ui: &mut Ui, icon: Icon, label: &str, enabled: bool) -> Response {
    let size = Vec2::splat(ui.spacing().interact_size.y);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    if ui.is_rect_visible(rect) {
        let visuals = if enabled {
            ui.style().interact(&response)
        } else {
            &ui.style().visuals.widgets.noninteractive
        };
        if response.hovered() && enabled {
            ui.painter()
                .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
        }
        let color = if enabled {
            visuals.fg_stroke.color
        } else {
            ui.visuals().weak_text_color()
        };
        paint_icon(ui, rect.shrink(4.0), icon, color);
    }
    response.on_hover_text(label)
}

/// A segmented control: one button for each option, the selected one
/// highlighted. Returns true if the value changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str)],
    accent: Color32,
) -> bool {
    let mut changed = false;
    let height = ui.spacing().interact_size.y;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galleys: Vec<_> = options
        .iter()
        .map(|(_, text)| {
            ui.painter()
                .layout_no_wrap((*text).to_owned(), font.clone(), Color32::PLACEHOLDER)
        })
        .collect();
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + 22.0).collect();
    let total = vec2(widths.iter().sum::<f32>() + 4.0, height + 2.0);
    let (outer, _) = ui.allocate_exact_size(total, Sense::hover());
    let visuals = ui.visuals().clone();
    ui.painter().rect_filled(
        outer,
        CornerRadius::same(7),
        visuals.widgets.inactive.weak_bg_fill,
    );
    let mut x = outer.min.x + 2.0;
    for (((option, text), galley), width) in options.iter().zip(galleys).zip(widths) {
        let rect = Rect::from_min_size(pos2(x, outer.min.y + 2.0), vec2(width, height - 2.0));
        x += width;
        let id = ui.id().with(("segment", text));
        let response = ui.interact(rect, id, Sense::click());
        let selected = *value == *option;
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, *text)
        });
        if response.clicked() && !selected {
            *value = *option;
            changed = true;
        }
        let text_color = if selected {
            Color32::WHITE
        } else if response.hovered() {
            visuals.strong_text_color()
        } else {
            visuals.text_color()
        };
        if selected {
            ui.painter().rect_filled(rect, CornerRadius::same(6), accent);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(6), visuals.widgets.hovered.weak_bg_fill);
        }
        let pos = rect.center() - galley.size() * 0.5;
        ui.painter().galley(pos, galley, text_color);
    }
    changed
}

/// A filled circle in the text line, for connection states.
pub fn status_dot(ui: &mut Ui, color: Color32) {
    let size = vec2(10.0, ui.spacing().interact_size.y);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// The FastTick logo: a small tree of three nodes in the accent colour.
pub fn logo(ui: &mut Ui, accent: Color32) -> Response {
    let size = Vec2::splat(ui.spacing().interact_size.y);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    let c = rect.center();
    let s = rect.width() * 0.5;
    let stroke = Stroke::new(1.6, accent);
    let root = Rect::from_center_size(c + vec2(0.0, -s * 0.48), vec2(s * 0.8, s * 0.42));
    let left = Rect::from_center_size(c + vec2(-s * 0.48, s * 0.5), vec2(s * 0.62, s * 0.42));
    let right = Rect::from_center_size(c + vec2(s * 0.48, s * 0.5), vec2(s * 0.62, s * 0.42));
    let bus = c.y + s * 0.02;
    painter.line_segment([root.center_bottom(), pos2(c.x, bus)], stroke);
    painter.line_segment([pos2(left.center().x, bus), pos2(right.center().x, bus)], stroke);
    painter.line_segment([pos2(left.center().x, bus), left.center_top()], stroke);
    painter.line_segment([pos2(right.center().x, bus), right.center_top()], stroke);
    painter.rect_filled(root, CornerRadius::same(2), accent);
    painter.rect_filled(left, CornerRadius::same(2), accent.gamma_multiply(0.75));
    painter.rect_filled(right, CornerRadius::same(2), accent.gamma_multiply(0.75));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Choice {
        A,
        B,
    }

    #[test]
    fn segmented_control_changes_value() {
        let mut harness = Harness::new_ui_state(
            |ui, value: &mut Choice| {
                segmented(ui, value, &[(Choice::A, "Alpha"), (Choice::B, "Beta")], Color32::BLUE);
            },
            Choice::A,
        );
        harness.get_by_label("Beta").click();
        harness.run();
        assert_eq!(*harness.state(), Choice::B);
    }

    #[test]
    fn icon_buttons_have_labels() {
        let mut clicked = false;
        {
            let mut harness = Harness::new_ui(|ui| {
                for icon in [
                    Icon::Plus,
                    Icon::Minus,
                    Icon::Fit,
                    Icon::TopDown,
                    Icon::LeftRight,
                    Icon::Open,
                    Icon::Connect,
                    Icon::First,
                    Icon::Previous,
                    Icon::Next,
                ] {
                    icon_button(ui, icon, &format!("{icon:?}"), true);
                }
                if icon_button(ui, Icon::Last, "Last", true).clicked() {
                    clicked = true;
                }
                icon_button(ui, Icon::Plus, "Disabled", false);
                logo(ui, Color32::BLUE);
                status_dot(ui, Color32::GREEN);
            });
            harness.get_by_label("Last").click();
            harness.run();
        }
        assert!(clicked);
    }
}
