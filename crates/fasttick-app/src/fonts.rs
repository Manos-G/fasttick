//! Fonts.
//!
//! egui's default fonts are off (one of them uses a licence that is not
//! OSI-approved). FastTick bundles Roboto (Apache-2.0) for proportional
//! text. For monospace text it looks for a monospace font that is installed
//! on the system. If it finds none, it uses Roboto.
//!
//! Roboto has no emoji and few symbols. The UI draws its icons with the
//! painter and does not depend on symbol glyphs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

/// The family name of Roboto Medium (labels, headings).
pub const MEDIUM: &str = "medium";
/// The family name of Roboto Bold.
pub const BOLD: &str = "bold";
/// The family name of Roboto Italic.
pub const ITALIC: &str = "italic";

static ROBOTO_REGULAR: &[u8] = include_bytes!("../assets/fonts/Roboto-Regular.ttf");
static ROBOTO_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Roboto-Medium.ttf");
static ROBOTO_BOLD: &[u8] = include_bytes!("../assets/fonts/Roboto-Bold.ttf");
static ROBOTO_ITALIC: &[u8] = include_bytes!("../assets/fonts/Roboto-Italic.ttf");

/// Monospace fonts to look for, by file name, in order of preference.
const MONOSPACE_FILES: &[&str] = &[
    "DejaVuSansMono.ttf",
    "LiberationMono-Regular.ttf",
    "NotoSansMono-Regular.ttf",
    "UbuntuSansMono-Regular.ttf",
    "FreeMono.ttf",
    "Menlo.ttc",
    "consola.ttf",
    "cour.ttf",
];

/// Directories that contain fonts on Linux, macOS and Windows.
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        "/usr/share/fonts",
        "/usr/local/share/fonts",
        "/System/Library/Fonts",
        "/Library/Fonts",
        "C:\\Windows\\Fonts",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
        dirs.push(home.join("Library/Fonts"));
    }
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data).join("fonts"));
    }
    dirs
}

/// Finds an installed monospace font. Returns the first file of
/// [`MONOSPACE_FILES`] that exists in a font directory (searched to a depth
/// of four).
pub fn find_monospace_font() -> Option<PathBuf> {
    find_font_in(&font_dirs(), MONOSPACE_FILES)
}

/// Finds the first of `names` in `dirs` (each searched to a depth of four).
pub fn find_font_in(dirs: &[PathBuf], names: &[&str]) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        collect_files(dir, 4, names, &mut found);
    }
    // Keep the order of preference of `names`.
    names.iter().find_map(|name| {
        found
            .iter()
            .find(|path| path.file_name().is_some_and(|f| f == *name))
            .cloned()
    })
}

fn collect_files(dir: &Path, depth: usize, names: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth > 0 {
                collect_files(&path, depth - 1, names, out);
            }
        } else if path
            .file_name()
            .and_then(|f| f.to_str())
            .is_some_and(|f| names.contains(&f))
        {
            out.push(path);
        }
    }
}

/// The font definitions: Roboto, and the system monospace font if one was
/// found (`monospace` is the font file content).
pub fn definitions(monospace: Option<Vec<u8>>) -> FontDefinitions {
    let mut fonts = FontDefinitions::empty();
    let mut add = |name: &str, data: FontData| {
        fonts.font_data.insert(name.to_owned(), Arc::new(data));
    };
    add("roboto", FontData::from_static(ROBOTO_REGULAR));
    add("roboto-medium", FontData::from_static(ROBOTO_MEDIUM));
    add("roboto-bold", FontData::from_static(ROBOTO_BOLD));
    add("roboto-italic", FontData::from_static(ROBOTO_ITALIC));
    let has_mono = monospace.is_some();
    if let Some(bytes) = monospace {
        add("system-mono", FontData::from_owned(bytes));
    }

    let with_fallback = |primary: &str| -> Vec<String> {
        let mut list = vec![primary.to_owned()];
        if primary != "roboto" {
            list.push("roboto".to_owned());
        }
        // The system font has many symbols that Roboto lacks (for example
        // in node names or values). Use it as the last fallback.
        if has_mono {
            list.push("system-mono".to_owned());
        }
        list
    };
    fonts
        .families
        .insert(FontFamily::Proportional, with_fallback("roboto"));
    fonts.families.insert(
        FontFamily::Monospace,
        if has_mono {
            vec!["system-mono".to_owned(), "roboto".to_owned()]
        } else {
            vec!["roboto".to_owned()]
        },
    );
    fonts
        .families
        .insert(FontFamily::Name(MEDIUM.into()), with_fallback("roboto-medium"));
    fonts
        .families
        .insert(FontFamily::Name(BOLD.into()), with_fallback("roboto-bold"));
    fonts
        .families
        .insert(FontFamily::Name(ITALIC.into()), with_fallback("roboto-italic"));
    fonts
}

/// Loads the system monospace font, if any. Fonts larger than 32 MiB are
/// ignored.
pub fn load_monospace() -> Option<Vec<u8>> {
    let path = find_monospace_font()?;
    let meta = std::fs::metadata(&path).ok()?;
    if meta.len() > 32 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    // A TrueType or OpenType file starts with one of these tags. A file
    // with other content would make egui panic when it parses the font.
    let valid = bytes.len() > 12
        && matches!(
            &bytes[..4],
            [0x00, 0x01, 0x00, 0x00] | b"OTTO" | b"true" | b"ttcf"
        );
    valid.then_some(bytes)
}

/// Installs the fonts in a context.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions(load_monospace()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_without_system_font() {
        let fonts = definitions(None);
        assert_eq!(fonts.families[&FontFamily::Monospace], vec!["roboto"]);
        assert_eq!(fonts.families[&FontFamily::Proportional], vec!["roboto"]);
        assert_eq!(
            fonts.families[&FontFamily::Name(MEDIUM.into())],
            vec!["roboto-medium", "roboto"]
        );
        assert!(!fonts.font_data.contains_key("system-mono"));
        // No egui default font is present.
        for name in FontDefinitions::builtin_font_names() {
            assert!(!fonts.font_data.contains_key(*name));
        }
    }

    #[test]
    fn definitions_with_system_font() {
        let fonts = definitions(Some(ROBOTO_REGULAR.to_vec()));
        assert_eq!(
            fonts.families[&FontFamily::Monospace],
            vec!["system-mono", "roboto"]
        );
        assert_eq!(
            fonts.families[&FontFamily::Proportional],
            vec!["roboto", "system-mono"]
        );
    }

    #[test]
    fn finds_fonts_by_preference() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("FreeMono.ttf"), b"x").unwrap();
        std::fs::write(dir.path().join("LiberationMono-Regular.ttf"), b"x").unwrap();
        let found = find_font_in(&[dir.path().to_owned()], MONOSPACE_FILES).unwrap();
        assert_eq!(found.file_name().unwrap(), "LiberationMono-Regular.ttf");
        assert_eq!(find_font_in(&[dir.path().join("missing")], MONOSPACE_FILES), None);
    }
}
