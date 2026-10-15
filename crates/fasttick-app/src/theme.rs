//! The themes: the egui [`Visuals`] of the shell and the [`CanvasPalette`]
//! of the tree canvas.
//!
//! FastTick has its own light and dark theme: a calm neutral scheme with
//! one accent colour (blue). The status colours come from a
//! colour-blind-friendly set (amber, bluish green, vermillion, grey); the
//! canvas also shows each status with its own shape, so colour is never the
//! only signal. The category colours use other hues (blue, indigo, violet,
//! pink, bronze), and each category also has its own glyph.
//!
//! The other themes come from the OpenWire project (MIT licence, the
//! `css/themes.css` file): OpenWire Light and Dark, the four Catppuccin
//! flavours (Catppuccin, MIT licence) and three VS Code themes (Dark
//! Modern, Dark+ and Light Modern). [`ThemeSpec`] holds their colours. The
//! Catppuccin themes also use Catppuccin colours for the categories and
//! the statuses; the others use the FastTick colours of the same scheme
//! (light or dark).

use egui::{Color32, CornerRadius, Shadow, Stroke, Theme, ThemePreference, Visuals};
use fasttick_core::status::NodeStatus;

use crate::canvas::NodeCategory;

/// The theme that the user chose.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum ThemeChoice {
    /// FastTick light.
    Light,
    /// FastTick dark.
    Dark,
    /// FastTick light or dark, as the system setting.
    #[default]
    System,
    /// OpenWire Light.
    OpenWireLight,
    /// OpenWire Dark.
    OpenWireDark,
    /// Catppuccin Latte (light).
    CatppuccinLatte,
    /// Catppuccin Frappé (dark).
    CatppuccinFrappe,
    /// Catppuccin Macchiato (dark).
    CatppuccinMacchiato,
    /// Catppuccin Mocha (dark).
    CatppuccinMocha,
    /// VS Code Dark Modern.
    VsCodeDarkModern,
    /// VS Code Dark+.
    VsCodeDarkPlus,
    /// VS Code Light Modern.
    VsCodeLightModern,
}

impl ThemeChoice {
    /// All choices, in menu order.
    pub const ALL: [ThemeChoice; 12] = [
        ThemeChoice::Light,
        ThemeChoice::Dark,
        ThemeChoice::System,
        ThemeChoice::OpenWireLight,
        ThemeChoice::OpenWireDark,
        ThemeChoice::CatppuccinLatte,
        ThemeChoice::CatppuccinFrappe,
        ThemeChoice::CatppuccinMacchiato,
        ThemeChoice::CatppuccinMocha,
        ThemeChoice::VsCodeDarkModern,
        ThemeChoice::VsCodeDarkPlus,
        ThemeChoice::VsCodeLightModern,
    ];

    /// The display name.
    pub fn name(self) -> &'static str {
        match self {
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
            ThemeChoice::System => "System",
            ThemeChoice::OpenWireLight => "OpenWire Light",
            ThemeChoice::OpenWireDark => "OpenWire Dark",
            ThemeChoice::CatppuccinLatte => "Catppuccin Latte",
            ThemeChoice::CatppuccinFrappe => "Catppuccin Frappé",
            ThemeChoice::CatppuccinMacchiato => "Catppuccin Macchiato",
            ThemeChoice::CatppuccinMocha => "Catppuccin Mocha",
            ThemeChoice::VsCodeDarkModern => "VS Code Dark Modern",
            ThemeChoice::VsCodeDarkPlus => "VS Code Dark+",
            ThemeChoice::VsCodeLightModern => "VS Code Light Modern",
        }
    }

    /// The command-line name, for example `catppuccin-mocha`.
    pub fn slug(self) -> &'static str {
        match self {
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
            ThemeChoice::System => "system",
            ThemeChoice::OpenWireLight => "openwire-light",
            ThemeChoice::OpenWireDark => "openwire-dark",
            ThemeChoice::CatppuccinLatte => "catppuccin-latte",
            ThemeChoice::CatppuccinFrappe => "catppuccin-frappe",
            ThemeChoice::CatppuccinMacchiato => "catppuccin-macchiato",
            ThemeChoice::CatppuccinMocha => "catppuccin-mocha",
            ThemeChoice::VsCodeDarkModern => "vscode-dark-modern",
            ThemeChoice::VsCodeDarkPlus => "vscode-dark-plus",
            ThemeChoice::VsCodeLightModern => "vscode-light-modern",
        }
    }

    /// The group in the menu.
    pub fn family(self) -> &'static str {
        match self {
            ThemeChoice::Light | ThemeChoice::Dark | ThemeChoice::System => "FastTick",
            ThemeChoice::OpenWireLight | ThemeChoice::OpenWireDark => "OpenWire",
            ThemeChoice::CatppuccinLatte
            | ThemeChoice::CatppuccinFrappe
            | ThemeChoice::CatppuccinMacchiato
            | ThemeChoice::CatppuccinMocha => "Catppuccin",
            ThemeChoice::VsCodeDarkModern
            | ThemeChoice::VsCodeDarkPlus
            | ThemeChoice::VsCodeLightModern => "VS Code",
        }
    }

    /// The egui preference: light or dark for a theme with a fixed scheme.
    pub fn preference(self) -> ThemePreference {
        match self {
            ThemeChoice::Light => ThemePreference::Light,
            ThemeChoice::Dark => ThemePreference::Dark,
            ThemeChoice::System => ThemePreference::System,
            other => match other.spec().map(|s| s.dark) {
                Some(true) => ThemePreference::Dark,
                _ => ThemePreference::Light,
            },
        }
    }

    /// Parses a command-line value: the command-line name (`dark`,
    /// `catppuccin-mocha`) or the display name, in any case.
    pub fn parse(text: &str) -> Option<ThemeChoice> {
        let key = |t: &str| -> String {
            t.chars()
                .filter(|c| c.is_alphanumeric() || *c == '+')
                .flat_map(char::to_lowercase)
                .collect()
        };
        let wanted = key(text);
        ThemeChoice::ALL
            .into_iter()
            .find(|c| key(c.slug()) == wanted || key(c.name()) == wanted)
    }

    /// The colours of a theme from OpenWire. `None` for the FastTick
    /// themes.
    fn spec(self) -> Option<&'static ThemeSpec> {
        let index = match self {
            ThemeChoice::Light | ThemeChoice::Dark | ThemeChoice::System => return None,
            ThemeChoice::OpenWireLight => 0,
            ThemeChoice::OpenWireDark => 1,
            ThemeChoice::CatppuccinLatte => 2,
            ThemeChoice::CatppuccinFrappe => 3,
            ThemeChoice::CatppuccinMacchiato => 4,
            ThemeChoice::CatppuccinMocha => 5,
            ThemeChoice::VsCodeDarkModern => 6,
            ThemeChoice::VsCodeDarkPlus => 7,
            ThemeChoice::VsCodeLightModern => 8,
        };
        Some(&SPECS[index])
    }
}

/// The colours of a theme from OpenWire. The names are the names of the
/// CSS variables in `css/themes.css` of OpenWire (`ink` is `--ink`, `sheet`
/// is `--ow-sheet`).
#[derive(Debug, Clone, Copy)]
struct ThemeSpec {
    dark: bool,
    ink: u32,
    ink_muted: u32,
    bg: u32,
    chrome: u32,
    surface: u32,
    rule: u32,
    rule_strong: u32,
    marker: u32,
    focus: u32,
    error: u32,
    warning: u32,
    info: u32,
    /// `--selected`: a colour and its alpha.
    selected: (u32, u8),
    sheet: u32,
    grid_major: u32,
    text: u32,
    muted: u32,
    /// The card fill: `--ow-body` or `--surface`, the one that is easier
    /// to see on `sheet`.
    card: u32,
    /// The names of conditions on the cards (FastTick or Catppuccin).
    condition: u32,
    /// Category colours (see [`CanvasPalette::categories`]); `None` for
    /// the FastTick colours of the scheme.
    categories: Option<[u32; 6]>,
    /// RUNNING, SUCCESS, FAILURE and SKIPPED; `None` for the FastTick
    /// colours of the scheme.
    statuses: Option<[u32; 4]>,
    /// Text and glyphs on the accent and the status colours.
    on_color: u32,
}

/// The themes from OpenWire, in the order of [`ThemeChoice::spec`].
const SPECS: [ThemeSpec; 9] = [
    // OpenWire Light.
    ThemeSpec {
        dark: false,
        ink: 0x26323f,
        ink_muted: 0x56636f,
        bg: 0xe7ecf0,
        chrome: 0xf5f7f9,
        surface: 0xffffff,
        rule: 0xd5dde4,
        rule_strong: 0xb3bfca,
        marker: 0xf2c12e,
        focus: 0x1f5fbf,
        error: 0xc42b27,
        warning: 0xb35c0a,
        info: 0x1f5fbf,
        selected: (0xf2c12e, 77),
        sheet: 0xe5ecf2,
        grid_major: 0xcfd9e3,
        text: 0x1d2833,
        muted: 0x52606d,
        card: 0xffffff,
        condition: 0x7445c2,
        categories: None,
        statuses: None,
        on_color: 0xffffff,
    },
    // OpenWire Dark.
    ThemeSpec {
        dark: true,
        ink: 0xe6ebf0,
        ink_muted: 0xa3b1be,
        bg: 0x12181e,
        chrome: 0x1b232b,
        surface: 0x243039,
        rule: 0x2e3a45,
        rule_strong: 0x4a5a69,
        marker: 0xf2c12e,
        focus: 0x8ab4ff,
        error: 0xff8a80,
        warning: 0xffb45e,
        info: 0x8ab4ff,
        selected: (0xf2c12e, 56),
        sheet: 0x161d24,
        grid_major: 0x29343f,
        text: 0xe6ebf0,
        muted: 0x9aabbb,
        card: 0x243039,
        condition: 0xb89cf0,
        categories: None,
        statuses: None,
        on_color: 0x12181e,
    },
    // Catppuccin Latte.
    ThemeSpec {
        dark: false,
        ink: 0x4c4f69,
        ink_muted: 0x6c6f85,
        bg: 0xdce0e8,
        chrome: 0xe6e9ef,
        surface: 0xeff1f5,
        rule: 0xccd0da,
        rule_strong: 0xacb0be,
        marker: 0xdf8e1d,
        focus: 0x1e66f5,
        error: 0xd20f39,
        warning: 0xfe640b,
        info: 0x1e66f5,
        selected: (0xdf8e1d, 71),
        sheet: 0xdce0e8,
        grid_major: 0xccd0da,
        text: 0x4c4f69,
        muted: 0x6c6f85,
        card: 0xeff1f5,
        condition: 0x8839ef,
        // Blue, pink, lavender, mauve, teal, overlay 1.
        categories: Some([0x1e66f5, 0xea76cb, 0x7287fd, 0x8839ef, 0x179299, 0x8c8fa1]),
        // Yellow, green, red, overlay 1.
        statuses: Some([0xdf8e1d, 0x40a02b, 0xd20f39, 0x8c8fa1]),
        on_color: 0xffffff,
    },
    // Catppuccin Frappé.
    ThemeSpec {
        dark: true,
        ink: 0xc6d0f5,
        ink_muted: 0xa5adce,
        bg: 0x232634,
        chrome: 0x292c3c,
        surface: 0x303446,
        rule: 0x414559,
        rule_strong: 0x626880,
        marker: 0xe5c890,
        focus: 0x8caaee,
        error: 0xe78284,
        warning: 0xef9f76,
        info: 0x8caaee,
        selected: (0xe5c890, 71),
        sheet: 0x232634,
        grid_major: 0x414559,
        text: 0xc6d0f5,
        muted: 0xa5adce,
        card: 0x303446,
        condition: 0xca9ee6,
        categories: Some([0x8caaee, 0xf4b8e4, 0xbabbf1, 0xca9ee6, 0x81c8be, 0x838ba7]),
        statuses: Some([0xe5c890, 0xa6d189, 0xe78284, 0x838ba7]),
        on_color: 0x232634,
    },
    // Catppuccin Macchiato.
    ThemeSpec {
        dark: true,
        ink: 0xcad3f5,
        ink_muted: 0xa5adcb,
        bg: 0x181926,
        chrome: 0x1e2030,
        surface: 0x24273a,
        rule: 0x363a4f,
        rule_strong: 0x5b6078,
        marker: 0xeed49f,
        focus: 0x8aadf4,
        error: 0xed8796,
        warning: 0xf5a97f,
        info: 0x8aadf4,
        selected: (0xeed49f, 71),
        sheet: 0x181926,
        grid_major: 0x363a4f,
        text: 0xcad3f5,
        muted: 0xa5adcb,
        card: 0x24273a,
        condition: 0xc6a0f6,
        categories: Some([0x8aadf4, 0xf5bde6, 0xb7bdf8, 0xc6a0f6, 0x8bd5ca, 0x8087a2]),
        statuses: Some([0xeed49f, 0xa6da95, 0xed8796, 0x8087a2]),
        on_color: 0x181926,
    },
    // Catppuccin Mocha.
    ThemeSpec {
        dark: true,
        ink: 0xcdd6f4,
        ink_muted: 0xa6adc8,
        bg: 0x11111b,
        chrome: 0x181825,
        surface: 0x1e1e2e,
        rule: 0x313244,
        rule_strong: 0x585b70,
        marker: 0xf9e2af,
        focus: 0x89b4fa,
        error: 0xf38ba8,
        warning: 0xfab387,
        info: 0x89b4fa,
        selected: (0xf9e2af, 71),
        sheet: 0x11111b,
        grid_major: 0x313244,
        text: 0xcdd6f4,
        muted: 0xa6adc8,
        card: 0x1e1e2e,
        condition: 0xcba6f7,
        categories: Some([0x89b4fa, 0xf5c2e7, 0xb4befe, 0xcba6f7, 0x94e2d5, 0x7f849c]),
        statuses: Some([0xf9e2af, 0xa6e3a1, 0xf38ba8, 0x7f849c]),
        on_color: 0x11111b,
    },
    // VS Code Dark Modern.
    ThemeSpec {
        dark: true,
        ink: 0xcccccc,
        ink_muted: 0x9d9d9d,
        bg: 0x181818,
        chrome: 0x181818,
        surface: 0x313131,
        rule: 0x2b2b2b,
        rule_strong: 0x3c3c3c,
        marker: 0x0078d4,
        focus: 0x0078d4,
        error: 0xf85149,
        warning: 0xcca700,
        info: 0x3794ff,
        selected: (0x0078d4, 77),
        sheet: 0x1f1f1f,
        grid_major: 0x303030,
        text: 0xcccccc,
        muted: 0x9d9d9d,
        card: 0x2b2b2b,
        condition: 0xb89cf0,
        categories: None,
        statuses: None,
        on_color: 0xffffff,
    },
    // VS Code Dark+.
    ThemeSpec {
        dark: true,
        ink: 0xd4d4d4,
        ink_muted: 0xa0a0a0,
        bg: 0x1e1e1e,
        chrome: 0x252526,
        surface: 0x3c3c3c,
        rule: 0x333333,
        rule_strong: 0x474747,
        marker: 0x007acc,
        focus: 0x007fd4,
        error: 0xf48771,
        warning: 0xcca700,
        info: 0x3794ff,
        selected: (0x007acc, 77),
        sheet: 0x1e1e1e,
        grid_major: 0x2d2d30,
        text: 0xd4d4d4,
        muted: 0xa0a0a0,
        card: 0x2d2d30,
        condition: 0xb89cf0,
        categories: None,
        statuses: None,
        on_color: 0xffffff,
    },
    // VS Code Light Modern.
    ThemeSpec {
        dark: false,
        ink: 0x3b3b3b,
        ink_muted: 0x616161,
        bg: 0xf8f8f8,
        chrome: 0xf8f8f8,
        surface: 0xffffff,
        rule: 0xe5e5e5,
        rule_strong: 0xcecece,
        marker: 0x005fb8,
        focus: 0x005fb8,
        error: 0xe51400,
        warning: 0xbf8803,
        info: 0x005fb8,
        selected: (0x005fb8, 36),
        sheet: 0xf3f3f3,
        grid_major: 0xe5e5e5,
        text: 0x3b3b3b,
        muted: 0x767676,
        card: 0xffffff,
        condition: 0x7445c2,
        categories: None,
        statuses: None,
        on_color: 0xffffff,
    },
];

/// The accent colour (light theme).
pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(0x25, 0x63, 0xEB);
/// The accent colour (dark theme).
pub const ACCENT_DARK: Color32 = Color32::from_rgb(0x5B, 0x8D, 0xEF);

/// Installs the visuals of the user's choice and the FastTick style.
pub fn install(ctx: &egui::Context, choice: ThemeChoice) {
    set_visuals(ctx, choice);
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

/// Changes the theme: the visuals of both schemes and the preference. A
/// theme from OpenWire replaces the visuals of its scheme; the other
/// scheme keeps the FastTick visuals.
pub fn set_visuals(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_visuals_of(Theme::Light, light_visuals());
    ctx.set_visuals_of(Theme::Dark, dark_visuals());
    if let Some(spec) = choice.spec() {
        let theme = if spec.dark { Theme::Dark } else { Theme::Light };
        ctx.set_visuals_of(theme, spec_visuals(spec));
    }
    ctx.set_theme(choice.preference());
}

/// The canvas palette of a choice. `scheme` is the scheme in use (for the
/// FastTick themes, which follow it).
pub fn canvas_palette(choice: ThemeChoice, scheme: Theme) -> CanvasPalette {
    match choice.spec() {
        Some(spec) => CanvasPalette::from_spec(spec),
        None => CanvasPalette::for_theme(scheme),
    }
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

fn rgba(hex: u32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, alpha)
}

/// The visuals of a theme from OpenWire.
fn spec_visuals(s: &ThemeSpec) -> Visuals {
    let mut v = if s.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let ink = rgb(s.ink);
    let rule = rgb(s.rule);
    let strong = rgb(s.rule_strong);
    let chrome = rgb(s.chrome);
    let focus = rgb(s.focus);
    v.panel_fill = chrome;
    v.window_fill = rgb(s.surface);
    v.extreme_bg_color = rgb(s.bg);
    v.faint_bg_color = mix(chrome, rule, 0.45);
    v.code_bg_color = rgb(s.bg);
    v.text_edit_bg_color = Some(rgb(s.bg));
    v.weak_text_color = Some(rgb(s.ink_muted));
    v.window_stroke = Stroke::new(1.0, strong);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    let shadow = if s.dark { 90 } else { 28 };
    v.window_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(shadow),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(shadow - 6),
    };
    let (selected, alpha) = s.selected;
    // The `--selected` colour over the panel; the text stays readable.
    v.selection.bg_fill = mix(chrome, rgb(selected), f32::from(alpha) / 255.0);
    v.selection.stroke = Stroke::new(1.0, focus);
    v.text_cursor.stroke = Stroke::new(2.0, focus);
    v.hyperlink_color = rgb(s.info);
    v.warn_fg_color = rgb(s.warning);
    v.error_fg_color = rgb(s.error);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = chrome;
    w.noninteractive.weak_bg_fill = chrome;
    w.noninteractive.bg_stroke = Stroke::new(1.0, rule);
    w.noninteractive.fg_stroke = Stroke::new(1.0, ink);
    w.inactive.weak_bg_fill = mix(chrome, rule, 0.75);
    w.inactive.bg_fill = rule;
    w.inactive.fg_stroke = Stroke::new(1.0, ink);
    w.hovered.weak_bg_fill = mix(chrome, strong, 0.55);
    w.hovered.bg_fill = mix(chrome, strong, 0.55);
    w.hovered.bg_stroke = Stroke::new(1.0, strong);
    w.hovered.fg_stroke = Stroke::new(1.5, ink);
    w.active.weak_bg_fill = mix(strong, focus, 0.25);
    w.active.bg_fill = mix(strong, focus, 0.25);
    w.active.bg_stroke = Stroke::new(1.0, focus);
    w.active.fg_stroke = Stroke::new(2.0, ink);
    w.open.weak_bg_fill = rule;
    w.open.bg_fill = rule;
    w.open.fg_stroke = Stroke::new(1.0, ink);
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

    /// The palette of a theme from OpenWire.
    fn from_spec(s: &ThemeSpec) -> Self {
        let base = if s.dark {
            CanvasPalette::dark()
        } else {
            CanvasPalette::light()
        };
        let card = rgb(s.card);
        let accent = rgb(s.focus);
        let categories = s.categories.map_or(base.categories, |c| c.map(rgb));
        let [running, success, failure, skipped] = s.statuses.map_or(
            [base.running, base.success, base.failure, base.skipped],
            |c| c.map(rgb),
        );
        CanvasPalette {
            dark: s.dark,
            background: rgb(s.sheet),
            grid: rgb(s.grid_major),
            card,
            card_border: rgb(s.rule_strong),
            card_border_hover: rgb(s.muted),
            shadow: base.shadow,
            text: rgb(s.text),
            text_weak: readable(rgb(s.muted), card, rgb(s.text), 3.5),
            text_value: rgb(s.ink),
            text_reference: readable(rgb(s.info), card, rgb(s.text), 4.5),
            condition: readable(rgb(s.condition), card, rgb(s.text), 4.5),
            separator: rgb(s.rule),
            edge: mix(card, rgb(s.muted), 0.7),
            accent,
            search: rgba(s.marker, if s.dark { 110 } else { 150 }),
            group_fill: rgba(s.marker, if s.dark { 12 } else { 16 }),
            group_border: rgba(s.marker, if s.dark { 70 } else { 90 }),
            band_fill: rgba(s.focus, 36),
            breakpoint: rgb(s.error),
            warning: rgb(s.warning),
            error: rgb(s.error),
            on_color: rgb(s.on_color),
            categories,
            running,
            success,
            failure,
            skipped,
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

/// The WCAG contrast ratio of two colours (1 to 21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `color`, mixed towards `toward` in small steps until its contrast on
/// `background` is at least `min`.
fn readable(color: Color32, background: Color32, toward: Color32, min: f32) -> Color32 {
    let mut t = 0.0;
    let mut out = color;
    while contrast(out, background) < min && t < 1.0 {
        t += 0.05;
        out = mix(color, toward, t);
    }
    out
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

    /// The palettes of all themes.
    fn all_palettes() -> Vec<(ThemeChoice, CanvasPalette)> {
        let mut out = vec![
            (ThemeChoice::Light, CanvasPalette::light()),
            (ThemeChoice::Dark, CanvasPalette::dark()),
        ];
        for choice in ThemeChoice::ALL {
            if let Some(spec) = choice.spec() {
                out.push((choice, CanvasPalette::from_spec(spec)));
            }
        }
        out
    }

    #[test]
    fn all_themes_from_openwire_are_there() {
        let named: Vec<ThemeChoice> = ThemeChoice::ALL
            .into_iter()
            .filter(|c| c.spec().is_some())
            .collect();
        assert_eq!(named.len(), SPECS.len());
        for choice in named {
            let spec = choice.spec().unwrap();
            let dark = choice.preference() == ThemePreference::Dark;
            assert_eq!(dark, spec.dark, "{}", choice.name());
            assert_eq!(spec_visuals(spec).dark_mode, spec.dark);
        }
    }

    #[test]
    fn shell_text_is_readable() {
        for choice in ThemeChoice::ALL {
            let Some(spec) = choice.spec() else {
                continue;
            };
            let v = spec_visuals(spec);
            let text = v.widgets.noninteractive.fg_stroke.color;
            for fill in [v.panel_fill, v.window_fill, v.extreme_bg_color] {
                assert!(contrast(text, fill) >= 4.5, "{}", choice.name());
            }
            assert!(
                contrast(text, v.selection.bg_fill) >= 4.5,
                "{} selection",
                choice.name()
            );
            assert!(
                contrast(v.weak_text_color(), v.panel_fill) >= 3.0,
                "{}",
                choice.name()
            );
        }
    }

    #[test]
    fn text_on_colours_is_readable() {
        for (choice, palette) in all_palettes() {
            assert!(
                contrast(palette.on_color, palette.accent) >= 3.0,
                "{} accent",
                choice.name()
            );
            for status in [palette.running, palette.success, palette.failure] {
                assert!(
                    contrast(palette.on_color, status) >= 1.8,
                    "{} status",
                    choice.name()
                );
            }
        }
    }

    #[test]
    fn text_is_readable_on_cards() {
        for (choice, palette) in all_palettes() {
            let name = choice.name();
            assert!(contrast(palette.text, palette.card) >= 7.0, "{name} text");
            assert!(
                contrast(palette.text_weak, palette.card) >= 3.5,
                "{name} weak"
            );
            assert!(
                contrast(palette.text_value, palette.card) >= 6.5,
                "{name} value"
            );
            assert!(
                contrast(palette.condition, palette.card) >= 4.5,
                "{name} condition"
            );
            assert!(
                contrast(palette.text_reference, palette.card) >= 4.5,
                "{name} reference"
            );
        }
    }

    #[test]
    fn old_text_is_readable_on_cards() {
        for palette in [CanvasPalette::light(), CanvasPalette::dark()] {
            assert!(contrast(palette.text, palette.card) >= 7.0);
            assert!(contrast(palette.text_weak, palette.card) >= 3.5);
            assert!(contrast(palette.text_value, palette.card) >= 7.0);
            assert!(contrast(palette.condition, palette.card) >= 4.5);
        }
    }

    #[test]
    fn colours_are_distinct() {
        for (_, palette) in all_palettes() {
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
            assert_eq!(ThemeChoice::parse(choice.slug()), Some(choice));
        }
        assert_eq!(
            ThemeChoice::parse("Catppuccin Frappe"),
            Some(ThemeChoice::CatppuccinFrappe)
        );
        assert_eq!(
            ThemeChoice::parse("vscode-dark+"),
            Some(ThemeChoice::VsCodeDarkPlus)
        );
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(200, 100, 50);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 0.5), Color32::from_rgb(100, 50, 25));
    }
}
