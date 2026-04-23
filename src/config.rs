// Persistent user configuration.
//
// Stored as a minimal hand-parsed TOML-ish text file at
//   %APPDATA%\loupe\config.toml
// so users can inspect or reset it with any text editor.
//
// Keys:
//   hotkey_mods, hotkey_vkey
//   renderer = classic | gpu

use std::fs;
use std::path::PathBuf;

use crate::magnifier::RendererKind;

/// The directory and file where configuration is stored.
fn config_path() -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(PathBuf::from(appdata).join("loupe").join("config.toml"))
}

// ── Public types ─────────────────────────────────────────────────────────────

pub struct HotkeyConfig {
    pub mods: u32,
    pub vkey: u32,
}

pub struct AppConfig {
    pub hotkey: Option<HotkeyConfig>,
    pub renderer: RendererKind,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            hotkey: None,
            renderer: RendererKind::Classic,
        }
    }
}

// ── Load ─────────────────────────────────────────────────────────────────────

/// Full config. Missing keys use defaults. Missing file yields `default()`.
pub fn load_config() -> AppConfig {
    let Some(path) = config_path() else {
        return AppConfig::default();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return AppConfig::default();
    };

    let mut mods: Option<u32> = None;
    let mut vkey: Option<u32> = None;
    let mut renderer: Option<RendererKind> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(v) = line.strip_prefix("hotkey_mods") {
            mods = v.trim_start_matches([' ', '=']).trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("hotkey_vkey") {
            vkey = v.trim_start_matches([' ', '=']).trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("renderer") {
            let s = v.trim_start_matches([' ', '=']).trim();
            if let Some(k) = RendererKind::from_str(s) {
                renderer = Some(k);
            }
        }
    }

    let hotkey = match (mods, vkey) {
        (Some(m), Some(v)) => Some(HotkeyConfig { mods: m, vkey: v }),
        _ => None,
    };

    AppConfig {
        hotkey,
        renderer: renderer.unwrap_or(RendererKind::Classic),
    }
}

// ── Save ─────────────────────────────────────────────────────────────────────

fn write_file(cfg: &AppConfig) {
    let Some(path) = config_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let hk = if let Some(h) = &cfg.hotkey {
        format!("hotkey_mods = {}\nhotkey_vkey = {}\n", h.mods, h.vkey)
    } else {
        String::new()
    };
    let contents = format!(
        "# Loupe configuration — edit with any text editor.\n\
         # Delete this file to reset all settings.\n\
         \n\
         {hk}\
         renderer = {}\n",
        cfg.renderer.as_str()
    );
    let _ = fs::write(&path, contents);
}

/// Persist a hotkey binding, preserving the renderer line.
pub fn save_hotkey(mods: u32, vkey: u32) {
    let mut c = load_config();
    c.hotkey = Some(HotkeyConfig { mods, vkey });
    write_file(&c);
}

/// Store renderer mode (GPU vs classic).
pub fn save_renderer(kind: RendererKind) {
    let mut c = load_config();
    c.renderer = kind;
    write_file(&c);
}
