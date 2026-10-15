//! Light and dark themes: the egui [`Visuals`] of the shell and the
//! [`CanvasPalette`] of the tree canvas.
//!
//! FastTick uses a calm neutral scheme with one accent colour (blue). The
//! status colours come from a colour-blind-friendly set (amber, bluish
//! green, vermillion, grey); the canvas also shows each status with its own
//! shape, so colour is never the only signal. The category colours use
//! other hues (blue, indigo, violet, pink, bronze), and each category also
//! has its own glyph.

use egui::{Color32, CornerRadius, Shadow, Stroke, Theme, ThemePreference, Visuals};
use fasttick_core::status::NodeStatus;

use crate::canvas::NodeCategory;

/// The theme that the user chose.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum ThemeChoice {
    /// Always light.
    Light,
    /// Always dark.
    Dark,
    /// Follow the system setting.
    #[default]
    System,
}

impl ThemeChoice {
    /// All choices, in menu order.
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::Light, ThemeChoice::Dark, ThemeChoice::System];

    /// The display name.
    pub fn name(self) -> &'static str {
        match self {
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
            ThemeChoice::System => "System",
        }
    }

    /// The egui preference.
    pub fn preference(self) -> ThemePreference {
        match self {
            ThemeChoice::Light => ThemePreference::Light,
            ThemeChoice::Dark => ThemePreference::Dark,
            ThemeChoice::System => ThemePreference::System,
        }
    }

    /// Parses a command-line value (`light`, `dark`, `system`).
    pub fn parse(text: &str) -> Option<ThemeChoice> {
        match text.to_ascii_lowercase().as_str() {
            "light" => Some(ThemeChoice::Light),
            "dark" => Some(ThemeChoice::Dark),
            "system" => Some(ThemeChoice::System),
            _ => None,
        }
    }
}

/// The accent colour (light theme).
pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(0x25, 0x63, 0xEB);
/// The accent colour (dark theme).
pub const ACCENT_DARK: Color32 = Color32::from_rgb(0x5B, 0x8D, 0xEF);

/// Installs the FastTick visuals for both themes and the user's choice.
pub fn install(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_visuals_of(Theme::Light, light_visuals());
    ctx.set_visuals_of(Theme::Dark, dark_visuals());
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 5.0);
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
        style.spacing.menu_margin = egui::Margin::same(6);
        style.spacing.interact_size.y = 22.0;
        style.interaction.tooltip_delay = 0.35;
        use egui::{FontFamily, FontId, TextStyle};
        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(11.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(18.0, FontFamily::Name(crate::fonts::MEDIUM.into())),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.5, FontFamily::Monospace),
            ),
        ]
        .into();
    });
    ctx.set_theme(choice.preference());
}

fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

fn light_visuals() -> Visuals {
    let mut v = Visuals::light();
    let accent = ACCENT_LIGHT;
    v.panel_fill = rgb(0xF4F5F7);
    v.window_fill = rgb(0xFFFFFF);
    v.extreme_bg_color = rgb(0xFFFFFF);
    v.faint_bg_color = rgb(0xEEF0F3);
    v.code_bg_color = rgb(0xEEF0F3);
    v.text_edit_bg_color = Some(rgb(0xEEF0F3));
    v.window_stroke = Stroke::new(1.0, rgb(0xD5D9E0));
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.window_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(28),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(24),
    };
    v.selection.bg_fill = rgb(0xD6E4FD);
    v.selection.stroke = Stroke::new(1.0, accent);
    v.hyperlink_color = accent;
    v.warn_fg_color = rgb(0xB86E00);
    v.error_fg_color = rgb(0xC9302C);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, rgb(0xDDE1E6));
    w.noninteractive.fg_stroke = Stroke::new(1.0, rgb(0x2B3038));
    w.inactive.weak_bg_fill = rgb(0xE8EBEF);
    w.inactive.bg_fill = rgb(0xE3E6EA);
    w.inactive.fg_stroke = Stroke::new(1.0, rgb(0x2B3038));
    w.hovered.weak_bg_fill = rgb(0xDCE1E7);
    w.hovered.bg_fill = rgb(0xDCE1E7);
    w.hovered.bg_stroke = Stroke::new(1.0, rgb(0xB9C0CA));
    w.active.weak_bg_fill = rgb(0xCDD4DD);
    w.active.bg_fill = rgb(0xCDD4DD);
    w.open.weak_bg_fill = rgb(0xE3E6EA);
    for state in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        state.corner_radius = CornerRadius::same(5);
    }
    v
}

fn dark_visuals() -> Visuals {
    let mut v = Visuals::dark();
    let accent = ACCENT_DARK;
    v.panel_fill = rgb(0x1D1F23);
    v.window_fill = rgb(0x25282D);
    v.extreme_bg_color = rgb(0x15171A);
    v.faint_bg_color = rgb(0x24272C);
    v.code_bg_color = rgb(0x2A2D33);
    v.text_edit_bg_color = Some(rgb(0x17191C));
    v.window_stroke = Stroke::new(1.0, rgb(0x383C44));
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.window_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(90),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(80),
    };
    v.selection.bg_fill = rgb(0x2C4A80);
    v.selection.stroke = Stroke::new(1.0, accent);
    v.hyperlink_color = accent;
    v.warn_fg_color = rgb(0xF2B33D);
    v.error_fg_color = rgb(0xF06A5C);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, rgb(0x33373E));
    w.noninteractive.fg_stroke = Stroke::new(1.0, rgb(0xD5D9DF));
    w.inactive.weak_bg_fill = rgb(0x2C3036);
    w.inactive.bg_fill = rgb(0x30343A);
    w.inactive.fg_stroke = Stroke::new(1.0, rgb(0xD5D9DF));
    w.hovered.weak_bg_fill = rgb(0x363B42);
    w.hovered.bg_fill = rgb(0x363B42);
    w.hovered.bg_stroke = Stroke::new(1.0, rgb(0x4A505A));
    w.active.weak_bg_fill = rgb(0x40464F);
    w.active.bg_fill = rgb(0x40464F);
    w.open.weak_bg_fill = rgb(0x30343A);
    for state in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        state.corner_radius = CornerRadius::same(5);
    }
    v
}

/// The colours of the tree canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasPalette {
    /// True for the dark palette.
    pub dark: bool,
    /// Canvas background.
    pub background: Color32,
    /// Background dots.
    pub grid: Color32,
    /// Card fill.
    pub card: Color32,
    /// Card border.
    pub card_border: Color32,
    /// Card border when the pointer is over the card.
    pub card_border_hover: Color32,
    /// Card shadow.
    pub shadow: Color32,
    /// Label text.
    pub text: Color32,
    /// Type line and port names.
    pub text_weak: Color32,
    /// Port values.
    pub text_value: Color32,
    /// Port values that are blackboard references (`{key}`).
    pub text_reference: Color32,
    /// The names of pre-conditions and post-conditions on the cards.
    pub condition: Color32,
    /// Separator between header and ports.
    pub separator: Color32,
    /// Edges.
    pub edge: Color32,
    /// The accent colour (selection, focus).
    pub accent: Color32,
    /// Search match highlight.
    pub search: Color32,
    /// Background of an expanded SubTree.
    pub group_fill: Color32,
    /// Border of an expanded SubTree.
    pub group_border: Color32,
    /// Rubber-band fill.
    pub band_fill: Color32,
    /// Breakpoint marker.
    pub breakpoint: Color32,
    /// Warning marker.
    pub warning: Color32,
    /// Error marker.
    pub error: Color32,
    /// Glyphs on a solid status or marker colour.
    pub on_color: Color32,
    /// Category colours, by [`NodeCategory`] (see [`CanvasPalette::category`]).
    pub categories: [Color32; 6],
    /// RUNNING.
    pub running: Color32,
    /// SUCCESS.
    pub success: Color32,
    /// FAILURE.
    pub failure: Color32,
    /// SKIPPED.
    pub skipped: Color32,
}

impl CanvasPalette {
    /// The light palette.
    pub fn light() -> Self {
        CanvasPalette {
            dark: false,
            background: rgb(0xF2F3F5),
            grid: rgb(0xD8DBE0),
            card: rgb(0xFFFFFF),
            card_border: rgb(0xD3D8DF),
            card_border_hover: rgb(0x9AA3B0),
            shadow: Color32::from_black_alpha(18),
            text: rgb(0x1E2329),
            text_weak: rgb(0x6A7380),
            text_value: rgb(0x2E3640),
            text_reference: rgb(0x1F6FC9),
            condition: rgb(0x7445C2),
            separator: rgb(0xE6E9ED),
            edge: rgb(0x9DA5B1),
            accent: ACCENT_LIGHT,
            search: Color32::from_rgba_unmultiplied(0xFF, 0xD2, 0x3F, 150),
            group_fill: Color32::from_rgba_unmultiplied(0x9C, 0x7C, 0x3C, 12),
            group_border: Color32::from_rgba_unmultiplied(0x9C, 0x7C, 0x3C, 70),
            band_fill: Color32::from_rgba_unmultiplied(0x25, 0x63, 0xEB, 28),
            breakpoint: rgb(0xD93636),
            warning: rgb(0xD98E04),
            error: rgb(0xD93636),
            on_color: Color32::WHITE,
            categories: [
                rgb(0x2F8AC9), // Action
                rgb(0xCF4F8F), // Condition
                rgb(0x5560D6), // Control
                rgb(0x9A55C9), // Decorator
                rgb(0x9C7C3C), // SubTree
                rgb(0x8A93A0), // Unknown
            ],
            running: rgb(0xE19A0C),
            success: rgb(0x1E9E6A),
            failure: rgb(0xE0483A),
            skipped: rgb(0x8C96A3),
        }
    }

    /// The dark palette.
    pub fn dark() -> Self {
        CanvasPalette {
            dark: true,
            background: rgb(0x17191C),
            grid: rgb(0x2B2F35),
            card: rgb(0x24272D),
            card_border: rgb(0x3A3F48),
            card_border_hover: rgb(0x6B7380),
            shadow: Color32::from_black_alpha(70),
            text: rgb(0xE8EAED),
            text_weak: rgb(0x9AA3AF),
            text_value: rgb(0xCDD3DA),
            text_reference: rgb(0x7FB2F5),
            condition: rgb(0xB89CF0),
            separator: rgb(0x33373F),
            edge: rgb(0x5D6570),
            accent: ACCENT_DARK,
            search: Color32::from_rgba_unmultiplied(0xE8, 0xB9, 0x1A, 110),
            group_fill: Color32::from_rgba_unmultiplied(0xC9, 0xA5, 0x62, 10),
            group_border: Color32::from_rgba_unmultiplied(0xC9, 0xA5, 0x62, 60),
            band_fill: Color32::from_rgba_unmultiplied(0x5B, 0x8D, 0xEF, 36),
            breakpoint: rgb(0xEF5350),
            warning: rgb(0xF2B33D),
            error: rgb(0xEF5350),
            on_color: Color32::WHITE,
            categories: [
                rgb(0x58A9E6), // Action
                rgb(0xE57DB0), // Condition
                rgb(0x8A92F0), // Control
                rgb(0xBC85E6), // Decorator
                rgb(0xC9A562), // SubTree
                rgb(0x8A93A0), // Unknown
            ],
            running: rgb(0xF2B33D),
            success: rgb(0x34C38A),
            failure: rgb(0xF06A5C),
            skipped: rgb(0x8C96A3),
        }
    }

    /// The palette for a theme.
    pub fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Light => CanvasPalette::light(),
            Theme::Dark => CanvasPalette::dark(),
        }
    }

    /// The colour of a category.
    pub fn category(&self, category: NodeCategory) -> Color32 {
        let index = match category {
            NodeCategory::Action => 0,
            NodeCategory::Condition => 1,
            NodeCategory::Control => 2,
            NodeCategory::Decorator => 3,
            NodeCategory::SubTree => 4,
            NodeCategory::Unknown => 5,
        };
        self.categories[index]
    }

    /// The colour of a status. IDLE has no colour.
    pub fn status(&self, status: NodeStatus) -> Option<Color32> {
        match status {
            NodeStatus::Idle => None,
            NodeStatus::Running => Some(self.running),
            NodeStatus::Success => Some(self.success),
            NodeStatus::Failure => Some(self.failure),
            NodeStatus::Skipped => Some(self.skipped),
        }
    }

    /// A colour mixed with the card fill: `amount` 0 gives the card colour,
    /// 1 gives `color`.
    pub fn tint(&self, color: Color32, amount: f32) -> Color32 {
        mix(self.card, color, amount)
    }
}

/// Mixes two opaque colours in gamma space. `t` 0 gives `a`, 1 gives `b`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance of an sRGB colour.
    fn luminance(c: Color32) -> f32 {
        let f = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_is_readable_on_cards() {
        for palette in [CanvasPalette::light(), CanvasPalette::dark()] {
            assert!(contrast(palette.text, palette.card) >= 7.0);
            assert!(contrast(palette.text_weak, palette.card) >= 3.5);
            assert!(contrast(palette.text_value, palette.card) >= 7.0);
            assert!(contrast(palette.condition, palette.card) >= 4.5);
        }
    }

    #[test]
    fn colours_are_distinct() {
        for palette in [CanvasPalette::light(), CanvasPalette::dark()] {
            let mut all: Vec<Color32> = palette.categories.to_vec();
            all.extend([palette.running, palette.success, palette.failure]);
            for (i, a) in all.iter().enumerate() {
                for b in &all[i + 1..] {
                    assert_ne!(a, b);
                }
            }
            assert_eq!(palette.status(NodeStatus::Idle), None);
            assert_eq!(palette.status(NodeStatus::Failure), Some(palette.failure));
        }
    }

    #[test]
    fn theme_choice_parse_and_mix() {
        assert_eq!(ThemeChoice::parse("Dark"), Some(ThemeChoice::Dark));
        assert_eq!(ThemeChoice::parse("x"), None);
        for choice in ThemeChoice::ALL {
            assert_eq!(ThemeChoice::parse(choice.name()), Some(choice));
        }
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(200, 100, 50);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 0.5), Color32::from_rgb(100, 50, 25));
    }
}
