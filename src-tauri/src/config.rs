//! User settings, stored as JSON in the app's config folder.
//!
//! Every field has a default, so a config written by an older version (or a
//! hand-edited one with fields missing) still loads.

use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CornerStyle {
    /// DWM's standard 8px rounding.
    Round,
    /// DWM's small 4px rounding, as used for menus and tooltips.
    Small,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Border {
    /// Leave the 1px window border to Windows.
    System,
    /// No border at all.
    None,
    /// `Config::border_color`.
    Custom,
}

/// A global shortcut. `mods` uses the RegisterHotKey flags
/// (Alt 1, Ctrl 2, Shift 4, Win 8) and `vk` is a Windows virtual-key code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shortcut {
    pub mods: u32,
    pub vk: u32,
}

const ALT: u32 = 1;
const SHIFT: u32 = 4;
const WIN: u32 = 8;

const fn win_alt(vk: u32) -> Option<Shortcut> {
    Some(Shortcut { mods: WIN | ALT, vk })
}

/// Everything a shortcut can do. The serialised names are shared with the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    TidyWindow,
    TidyMonitor,
    TidyAll,
    SnapLeft,
    SnapRight,
    SnapFull,
    SnapCenter,
    RoundAll,
}

impl Action {
    pub const ALL: [Action; 8] = [
        Action::TidyWindow,
        Action::TidyMonitor,
        Action::TidyAll,
        Action::SnapLeft,
        Action::SnapRight,
        Action::SnapFull,
        Action::SnapCenter,
        Action::RoundAll,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Hotkeys {
    pub tidy_window: Option<Shortcut>,
    pub tidy_monitor: Option<Shortcut>,
    pub tidy_all: Option<Shortcut>,
    pub snap_left: Option<Shortcut>,
    pub snap_right: Option<Shortcut>,
    pub snap_full: Option<Shortcut>,
    pub snap_center: Option<Shortcut>,
    pub round_all: Option<Shortcut>,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            tidy_window: win_alt(0x0D), // Enter
            tidy_monitor: win_alt(0x41), // A
            tidy_all: Some(Shortcut { mods: WIN | ALT | SHIFT, vk: 0x41 }),
            snap_left: win_alt(0x25),
            snap_right: win_alt(0x27),
            snap_full: win_alt(0x26),
            snap_center: win_alt(0x28),
            round_all: win_alt(0x43), // C
        }
    }
}

impl Hotkeys {
    pub fn get(&self, action: Action) -> Option<Shortcut> {
        match action {
            Action::TidyWindow => self.tidy_window,
            Action::TidyMonitor => self.tidy_monitor,
            Action::TidyAll => self.tidy_all,
            Action::SnapLeft => self.snap_left,
            Action::SnapRight => self.snap_right,
            Action::SnapFull => self.snap_full,
            Action::SnapCenter => self.snap_center,
            Action::RoundAll => self.round_all,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Master switch, toggled from the tray. Off means Fillet does nothing.
    pub enabled: bool,
    pub round_corners: bool,
    pub corner_style: CornerStyle,
    pub border: Border,
    /// `#rrggbb`, used when `border` is `Custom`.
    pub border_color: String,
    /// Space between windows and the screen edge, in logical pixels.
    pub margin: u32,
    /// Space between neighbouring windows, in logical pixels.
    pub gap: u32,
    /// Add the gaps whenever a window is snapped to a screen edge or zone.
    pub auto_tidy: bool,
    /// Turn maximised windows into padded full-screen windows.
    pub pad_maximized: bool,
    /// Corner radius for `clip_apps`, in logical pixels.
    pub clip_radius: u32,
    /// Executable names (lowercase) Fillet never touches.
    pub ignored_apps: Vec<String>,
    /// Executable names whose windows ignore the DWM corner setting and are
    /// rounded by clipping instead.
    pub clip_apps: Vec<String>,
    pub hotkeys: Hotkeys,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            round_corners: true,
            corner_style: CornerStyle::Round,
            border: Border::System,
            border_color: "#5b6cf0".into(),
            margin: 15,
            gap: 10,
            auto_tidy: false,
            pad_maximized: false,
            clip_radius: 12,
            ignored_apps: Vec::new(),
            clip_apps: vec!["chrome.exe".into()],
            hotkeys: Hotkeys::default(),
        }
    }
}

pub const MAX_SPACING: u32 = 64;
pub const MAX_RADIUS: u32 = 32;

impl Config {
    /// Clamp values into range and tidy the app lists, so the engine can
    /// trust whatever it is given.
    pub fn normalized(mut self) -> Self {
        self.margin = self.margin.min(MAX_SPACING);
        self.gap = self.gap.min(MAX_SPACING);
        self.clip_radius = self.clip_radius.clamp(2, MAX_RADIUS);
        if parse_hex_color(&self.border_color).is_none() {
            self.border_color = Config::default().border_color;
        }
        for list in [&mut self.ignored_apps, &mut self.clip_apps] {
            for app in list.iter_mut() {
                *app = app.trim().to_lowercase();
            }
            list.retain(|a| !a.is_empty());
            list.sort();
            list.dedup();
        }
        self
    }

    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            // Some editors save UTF-8 with a byte-order mark.
            .and_then(|text| serde_json::from_str::<Config>(text.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
            .normalized()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        // Write then rename, so a crash mid-write never leaves a broken file.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(tmp, path)
    }

    /// The border colour as a Win32 COLORREF (0x00BBGGRR).
    pub fn border_colorref(&self) -> u32 {
        let (r, g, b) = parse_hex_color(&self.border_color).unwrap_or((0x5b, 0x6c, 0xf0));
        u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16)
    }
}

fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let cfg: Config = serde_json::from_str(r#"{ "margin": 20, "hotkeys": { "roundAll": null } }"#).unwrap();
        assert_eq!(cfg.margin, 20);
        assert_eq!(cfg.gap, Config::default().gap);
        assert_eq!(cfg.hotkeys.round_all, None);
        assert_eq!(cfg.hotkeys.tidy_window, Hotkeys::default().tidy_window);
    }

    #[test]
    fn normalizes_values() {
        let cfg = Config {
            margin: 500,
            clip_radius: 0,
            border_color: "nope".into(),
            ignored_apps: vec![" Code.EXE ".into(), "code.exe".into(), "".into()],
            ..Config::default()
        }
        .normalized();
        assert_eq!(cfg.margin, MAX_SPACING);
        assert_eq!(cfg.clip_radius, 2);
        assert_eq!(cfg.border_color, Config::default().border_color);
        assert_eq!(cfg.ignored_apps, vec!["code.exe".to_string()]);
    }

    #[test]
    fn colorref_is_bgr() {
        let cfg = Config { border_color: "#112233".into(), ..Config::default() };
        assert_eq!(cfg.border_colorref(), 0x332211);
    }
}
