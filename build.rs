//! Compile `app.rc` so the icon is embedded. MSVC builds use `embed-resource`
//! (finds `rc.exe`). GNU / gnullvm builds use `windres`, with extra discovery
//! because plain `windres` is often missing from `PATH` in Git Bash.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn main() {
    if env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "msvc" {
        embed_resource::compile("app.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("failed to compile app.rc");
        return;
    }

    compile_app_rc_with_windres();
}

fn compile_app_rc_with_windres() {
    let windres = find_windres().unwrap_or_else(|| {
        let target = env::var("TARGET").unwrap_or_default();
        panic!(
            "windres not found (required to compile app.rc for {target}).\n\
             Install MinGW-w64 binutils and add its bin folder to PATH, or set WINDRES to the full path of windres.exe.\n\
             Example (MSYS2 UCRT64): pacman -S mingw-w64-ucrt-x86_64-binutils\n\
             Then use that environment's bin (e.g. C:\\msys64\\ucrt64\\bin) on PATH when running cargo."
        );
    });

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let prefix = "app";
    let out_file = Path::new(&out_dir).join(format!("lib{prefix}.a"));
    let out_file_str = out_file.to_str().expect("OUT_DIR must be UTF-8");

    let pe_target = windres_pe_target();

    let resource = "app.rc";
    let st = Command::new(&windres)
        .args(["--input", resource, "--output-format=coff", "--target"])
        .arg(&*pe_target)
        .args(["-c", "65001"])
        .args(["--output", out_file_str, "--include-dir", &out_dir])
        .status()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", windres.display()));

    if !st.success() {
        panic!("windres failed to compile {resource}");
    }

    println!("cargo:rustc-link-arg-bins={out_file_str}");
    println!("cargo:rerun-if-changed={resource}");
    println!("cargo:rerun-if-changed=assets/loupe.ico");
}

fn windres_pe_target() -> Cow<'static, OsStr> {
    if let Some(host) = env::var_os("MINGW_CHOST") {
        return Cow::Owned(host);
    }
    let target = env::var_os("TARGET").expect("TARGET");
    let bytes = target.as_encoded_bytes();
    let pe = match bytes {
        [b'x', b'8', b'6', b'_', b'6', b'4', ..] => "pe-x86-64",
        [b'a', b'a', b'r', b'c', b'h', b'6', b'4', ..] => "pe-aarch64-little",
        _ => "pe-i386",
    };
    Cow::Borrowed(OsStr::new(pe))
}

fn find_windres() -> Option<PathBuf> {
    if let Ok(p) = env::var("WINDRES") {
        let pb = PathBuf::from(p.trim());
        if is_windres(&pb) {
            return Some(pb);
        }
    }

    if let Some(p) = find_on_path("windres") {
        return Some(p);
    }

    let arch = env::var("TARGET").ok()?.split('-').next()?.to_string();
    if let Some(p) = find_on_path(&format!("{arch}-w64-mingw32-windres")) {
        return Some(p);
    }

    try_well_known_windres_paths()
}

fn is_windres(path: &Path) -> bool {
    path.is_file()
        && Command::new(path)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        for candidate in [PathBuf::from(name), PathBuf::from(format!("{name}.exe"))] {
            let full = dir.join(&candidate);
            if is_windres(&full) {
                return Some(full);
            }
        }
    }
    None
}

fn try_well_known_windres_paths() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(m) = env::var("MINGW_PREFIX") {
        roots.push(PathBuf::from(m.trim()));
    }
    for r in [r"C:\msys64", r"C:\msys32"] {
        roots.push(PathBuf::from(r));
    }

    for root in roots {
        for sub in ["ucrt64/bin", "mingw64/bin", "clang64/bin", "mingw32/bin"] {
            let w = root.join(sub).join("windres.exe");
            if is_windres(&w) {
                return Some(w);
            }
        }
    }

    if let Ok(home) = env::var("USERPROFILE") {
        let scoop = PathBuf::from(home)
            .join("scoop")
            .join("apps")
            .join("mingw")
            .join("current")
            .join("bin")
            .join("windres.exe");
        if is_windres(&scoop) {
            return Some(scoop);
        }
    }

    if let Ok(entries) = fs::read_dir(r"C:\msys64") {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                let w = p.join("bin").join("windres.exe");
                if is_windres(&w) {
                    return Some(w);
                }
            }
        }
    }

    None
}
