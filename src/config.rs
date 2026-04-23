// Persistent user configuration.
//
// Stored as a minimal hand-parsed TOML-ish text file at
//   %APPDATA%\loupe\config.toml
// so users can inspect or reset it with any text editor.
//
// Current keys:
//   hotkey_mods = <u32>   HOT_KEY_MODIFIERS raw value
//   hotkey_vkey = <u32>   virtual-key code

use std::fs;
use std::path::PathBuf;

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

// ── Load ─────────────────────────────────────────────────────────────────────

/// Read the config file and return the saved hotkey, if any.
/// Returns `None` on missing file, parse error, or incomplete data.
pub fn load_hotkey() -> Option<HotkeyConfig> {
    let text = fs::read_to_string(config_path()?).ok()?;
    let mut mods: Option<u32> = None;
    let mut vkey: Option<u32> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(v) = line.strip_prefix("hotkey_mods") {
            mods = v.trim_start_matches([' ', '=']).trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("hotkey_vkey") {
            vkey = v.trim_start_matches([' ', '=']).trim().parse().ok();
        }
    }

    Some(HotkeyConfig { mods: mods?, vkey: vkey? })
}

// ── Save ─────────────────────────────────────────────────────────────────────

/// Persist a hotkey binding. Silently ignores write errors (best-effort).
pub fn save_hotkey(mods: u32, vkey: u32) {
    let Some(path) = config_path() else { return };

    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }

    let contents = format!(
        "# Loupe configuration — edit with any text editor.\n\
         # Delete this file to reset all settings.\n\
         \n\
         hotkey_mods = {mods}\n\
         hotkey_vkey = {vkey}\n"
    );
    let _ = fs::write(&path, contents);
}
