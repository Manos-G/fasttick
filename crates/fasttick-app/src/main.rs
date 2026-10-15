//! FastTick: a tree editor, live debugger and log replayer for
//! BehaviorTree.CPP 4.
//!
//! Usage: `fasttick [--theme THEME] [--mode edit|live|replay]
//! [--connect HOST[:PORT]] [FILE...]`

use fasttick_app::app::{FastTickApp, Mode, StartOptions, parse_endpoint};
use fasttick_app::theme::ThemeChoice;

const USAGE: &str = "Usage: fasttick [--theme THEME] [--mode edit|live|replay]
                [--connect HOST[:PORT]] [FILE...]

Opens tree XML files (.xml), projects (.ftproj) and logs (.btlog, or SQLite
.db3 and .btdb).

Options:
  --theme THEME          Use this theme for this session: light, dark, system,
                         openwire-light, openwire-dark, catppuccin-latte,
                         catppuccin-frappe, catppuccin-macchiato,
                         catppuccin-mocha, vscode-dark-modern,
                         vscode-dark-plus or vscode-light-modern
  --mode MODE            Start in this mode (edit, live or replay)
  --connect HOST[:PORT]  Connect to an executor (the default port is 1667)
  --help                 Show this text
  --version              Show the version";

/// The window icon (the same picture as `packaging/fasttick.svg`).
static ICON: &[u8] = include_bytes!("../assets/icon-64.png");

fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<StartOptions>, String> {
    let mut options = StartOptions::default();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "--version" | "-V" => {
                println!("fasttick {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--theme" => {
                let value = args.next().ok_or("--theme needs a value")?;
                options.theme =
                    Some(ThemeChoice::parse(&value).ok_or(format!("unknown theme {value:?}"))?);
            }
            "--mode" => {
                let value = args.next().ok_or("--mode needs a value")?;
                options.mode = Some(Mode::parse(&value).ok_or(format!("unknown mode {value:?}"))?);
            }
            "--connect" => {
                let value = args.next().ok_or("--connect needs a value")?;
                options.connect =
                    Some(parse_endpoint(&value).ok_or(format!("bad address {value:?}"))?);
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other:?}")),
            path => options.open.push(path.into()),
        }
    }
    Ok(Some(options))
}

fn main() -> eframe::Result {
    let options = match parse_args(std::env::args().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(()),
        Err(message) => {
            eprintln!("fasttick: {message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("FastTick")
        .with_app_id("fasttick")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([720.0, 480.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(ICON) {
        viewport = viewport.with_icon(icon);
    }
    let native = eframe::NativeOptions {
        viewport,
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native(
        "fasttick",
        native,
        Box::new(move |cc| Ok(Box::new(FastTickApp::new(cc, options)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Option<StartOptions>, String> {
        parse_args(list.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn parses_arguments() {
        let options = args(&["--theme", "dark", "--mode", "replay", "a.xml", "b.btlog"])
            .unwrap()
            .unwrap();
        assert_eq!(options.theme, Some(ThemeChoice::Dark));
        assert_eq!(options.mode, Some(Mode::Replay));
        assert_eq!(options.open.len(), 2);
        assert!(args(&["--theme"]).is_err());
        assert!(args(&["--theme", "pink"]).is_err());
        let options = args(&["--theme", "catppuccin-mocha"]).unwrap().unwrap();
        assert_eq!(options.theme, Some(ThemeChoice::CatppuccinMocha));
        assert!(args(&["--mode", "x"]).is_err());
        assert!(args(&["--frobnicate"]).is_err());
        let options = args(&["--connect", "robot:1700"]).unwrap().unwrap();
        assert_eq!(options.connect, Some(("robot".to_owned(), 1700)));
        assert!(args(&["--connect"]).is_err());
        assert!(args(&["--connect", "robot:99999"]).is_err());
        assert_eq!(args(&["--help"]).unwrap(), None);
        assert_eq!(args(&[]).unwrap(), Some(StartOptions::default()));
    }
}
