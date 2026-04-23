# Loupe

A small, native Windows magnifier. Pick a region of your screen with a global
hotkey or the tray icon and watch it live, zoomed in, inside a floating
always-on-top window. Built in Rust on top of the Win32 Magnification API
(`windows-rs`), no GUI framework, no Electron.

## Features (v0.1)

- Per-monitor DPI v2 awareness — sharp on any scaling factor.
- **Renderer (tray → *Renderer*)**: **Classic** uses the built-in Win32
  `WC_MAGNIFIER` control (bilinear scaling). **GPU** uses **Windows.Graphics.Capture**
  (WGC) + **Direct3D 11** + a **Lanczos-3** pixel shader for sharper upscaling at
  high zoom. The choice is stored in `%APPDATA%\loupe\config.toml` as `renderer =
  classic` or `gpu`. If GPU init fails (e.g. unsupported OS), the app falls back
  to Classic and logs a message. **Minimum for GPU:** Windows 10 version 1809
  (17763) or later for WGC; some early Win10 builds briefly show a yellow capture
  border (removed in newer Windows).
- Always-on-top, resizable, draggable magnifier window. Double-click the
  magnified view to enter borderless fullscreen (no title bar, covers the
  entire monitor); double-click again to restore. **Mouse wheel** over the
  magnified view zooms in or out (adjusts the captured screen rectangle
  about its center). (A nearly invisible layered child sits above the
  magnifier for hit-testing — the classic magnifier control itself does not
  get mouse hits.)

  On the **Classic** renderer the magnification factor is clamped to **at
  least 1.5×** — the underlying `WC_MAGNIFIER` API becomes visibly jittery
  near 1.0× and the safe threshold drifts between systems. For sub-1.5×
  overviews switch to the **GPU** renderer (tray → right-click → *Renderer*
  → *GPU*); the choice is remembered in `config.toml`.
- **Persistent on-desktop source frame**: after picking a region, a thin
  **red outline** stays on the desktop showing exactly which screen
  rectangle is being magnified. **It matches the rainbow selection
  rectangle pixel-for-pixel — same position, same size — always.** This
  is the highest-priority UX guarantee: the source rectangle is never
  silently shrunk by any zoom cap. After each region selection the
  loupe's client area is also **reshaped to the selection's aspect ratio
  while preserving total area** (anchored at the current top-left,
  clamped to the monitor work area; skipped in fullscreen), so for any
  reasonable selection there is no letterboxing inside the magnified
  view and the Classic 1.5× safe range is comfortably satisfied. The
  frame is **click-through in the middle** (apps underneath stay
  clickable) and **draggable by the border** — click the red edge and
  drag to pan the source rectangle (mouse direction matches source
  movement; no inversion). Wheel-zoom keeps the frame in sync; on
  Classic, scrolling out beyond the 1.5× floor is a no-op so the source
  and the frame never drift apart. Resizing the loupe window keeps
  `current_source` (and therefore the red frame) unchanged, so the
  frame still matches your selection 1:1 — if that pushes the Classic
  scale below 1.5× and you see flicker, switch the renderer to **GPU**.
  Toggle visibility from the tray (right-click → *Show source frame*);
  state defaults to *visible* and is in-memory only. The frame is
  hidden from screen capture (`SetWindowDisplayAffinity(
  WDA_EXCLUDEFROMCAPTURE)`, Windows 10 v2004+) so it never appears
  inside the magnified view.
- Drag-to-select source rectangle on a transparent fullscreen overlay with a
  static rainbow border.
- Live update: Classic path refreshes the source at ~60 Hz via a timer; the GPU
  path is driven by capture frames (no periodic `WM_TIMER`). On the GPU path
  every captured frame is also `CopyResource`d into a self-owned cache
  texture, so pan / wheel-zoom / window-resize / fullscreen toggle can
  immediately re-render from the cached frame instead of waiting for the
  next WGC frame (important on a static desktop, where WGC may not push a
  frame for seconds).
- User-bindable global hotkey (tray → right-click → *Bind hotkey…*).
- Clean teardown: hotkey unregistered, tray icon removed, magnifier
  uninitialized.

## Build

You need a Windows Rust **host** triple plus that toolchain’s C linker on
`PATH`. Pick one of the following.

### A. MSVC (simplest if you already use Visual Studio / Build Tools)

`rustup default stable-x86_64-pc-windows-msvc` alone does **not** install the
linker. You need **`link.exe`** from Microsoft’s C++ toolchain.

1. Install [Build Tools for Visual Studio](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
   and select the **Desktop development with C++** workload (MSVC toolset +
   Windows SDK). A full Visual Studio install with that workload also works.
2. Open a **new** terminal (or use **Developer PowerShell for VS** / **x64
   Native Tools Command Prompt for VS** from the Start menu — they put
   `link.exe` on `PATH` automatically).
3. Then:

```powershell
rustup default stable-x86_64-pc-windows-msvc
cargo build --release
```

### B. GNU (MSYS2 MinGW)

```powershell
rustup default stable-x86_64-pc-windows-gnu
```

The GNU target needs MinGW **binutils** and **gcc** on `PATH`, including
**`dlltool.exe`**, **`gcc.exe`**, **`ld.exe`**, and **`windres.exe`** (all ship
with the UCRT64 / MINGW64 toolchain in MSYS2).

- Install MSYS2, then in the **UCRT64** shell:
  `pacman -S mingw-w64-ucrt-x86_64-toolchain mingw-w64-ucrt-x86_64-binutils`
- Either run **`cargo build`** from that MSYS2 environment, **or** add
  `C:\msys64\ucrt64\bin` (or your MinGW `bin`) to the **Windows user `PATH`**
  and open a **new** PowerShell.

If you see **`dlltool.exe`: program not found** while using `*-windows-gnu`,
you are building from a shell where MinGW `bin` is not on `PATH` — fix the
path or use **A. MSVC** instead.

### C. LLVM-MinGW (gnullvm host)

If your default toolchain is `*-pc-windows-gnullvm`, the linker must be
**`x86_64-w64-mingw32-clang`** from [llvm-mingw](https://github.com/mstorsjo/llvm-mingw/releases)
(unzip and add the `bin` folder to `PATH`). If you see **linker
`x86_64-w64-mingw32-clang` not found**, either install that LLVM-MinGW `bin`
directory or switch host with **A** or **B** above (`rustup default …`).

### One-time PATH setup (Windows — gnullvm via winget)

If you installed Rust via `rustup` and LLVM-MinGW via `winget`, neither is
automatically added to the **permanent** Windows user `PATH`. Run this once in
**PowerShell** (adjust the LLVM-MinGW folder name to match your installed
version):

```powershell
$cargoBin = "$env:USERPROFILE\.cargo\bin"
$llvmBin  = "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\" +
            "MartinStorsjo.LLVM-MinGW.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\" +
            "llvm-mingw-20260421-ucrt-x86_64\bin"   # <-- update tag if newer

$cur = [System.Environment]::GetEnvironmentVariable("Path", "User")
foreach ($p in @($cargoBin, $llvmBin)) {
    if (($cur -split ";") -notcontains $p) { $cur = "$p;$cur" }
}
[System.Environment]::SetEnvironmentVariable("Path", $cur, "User")
Write-Host "Done — open a new PowerShell window and run: cargo build --release"
```

After that, **open a new PowerShell window** and `cargo build --release` works
without any extra setup. Common symptoms when this step is skipped:

| Error | Cause |
|---|---|
| `cargo: command not found` (Bash) or `CommandNotFoundException` (PS) | `~\.cargo\bin` not on **user** `PATH` — run `rustup` installer’s PATH step, or add it manually and **open a new terminal** |
| `lld: error: unable to find library -lgcc_eh` / `-lgcc` | **GNU / gnullvm**: linker sees `clang`/`lld` but not MinGW **runtime libs** (incomplete LLVM-MinGW tree, or `bin` on `PATH` without matching `lib\`). Fix: use a **full** [llvm-mingw](https://github.com/mstorsjo/llvm-mingw) unpack (same root for `bin\` + `x86_64-w64-mingw32\lib\`), or build from **MSYS2 UCRT64** with the full `mingw-w64-ucrt-x86_64-toolchain`, or switch to **A. MSVC** (`rustup default stable-x86_64-pc-windows-msvc`). |
| `error calling dlltool 'dlltool.exe': program not found` | MinGW `bin` not on `PATH` — see **B** |
| `linker 'x86_64-w64-mingw32-clang' not found` | LLVM-MinGW `bin\` not on `PATH` |
| `linker link.exe not found` | **MSVC** target but **Visual C++ Build Tools** (or VS with C++ workload) not installed, or you are in plain PowerShell without MSVC on `PATH`. Install the workload above, or build from **Developer PowerShell for VS** / **x64 Native Tools Command Prompt**. |

**Quick check (PowerShell)** — `cargo` only for this window:

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
cargo --version
```

---

The build embeds `assets/loupe.ico` via `app.rc`. For **GNU / gnullvm** targets,
`windres` must also be available (MinGW **binutils**). If **`windres`** is not
found:

- MSYS2: `pacman -S mingw-w64-ucrt-x86_64-binutils` and keep that `bin` on
  `PATH`, **or**
- Set `WINDRES` to the full path of `windres.exe`.

The build script also probes common MSYS2 `bin` paths when `PATH` is minimal.

```powershell
cargo build --release
```

The binary lands at `target\release\loupe.exe`.

## Run

```powershell
cargo run
# or
cargo run --release
```

By default the binary is linked as a **console** app so `eprintln!` and other
stderr output appear in the **same terminal window** you ran `cargo run` from
(PowerShell, cmd, Windows Terminal, etc.). There is no separate “debug cmd”
window unless you start one yourself (e.g. open `cmd.exe` and run
`target\release\loupe.exe` there).

To build a **GUI-only** executable (no extra console window when double-clicking
`loupe.exe`), pass the `hide_console` feature (this is what GitHub Actions
release uses):

```powershell
cargo build --release --features hide_console
```

Click the tray icon (or *New loupe* in the menu), drag a rectangle on screen, release
the mouse, and the magnified view appears.

To set a global hotkey: right-click the tray icon for the menu → *Bind hotkey…*, then hold
a modifier key (`Ctrl`, `Alt`, or `Shift`) and press any other key.

Press `Esc` during region selection to cancel.

## Project layout

```
src/
  main.rs          entry point, message loop, host window
  hotkey.rs        global hotkey registration
  hotkey_bind.rs   hotkey binding dialog
  tray.rs          notification-area icon and menu
  region.rs        transparent overlay for region selection
  magnifier.rs     WC_MAGNIFIER child + uniform-scale transform
  dpi.rs           per-monitor DPI v2 awareness
assets/
  loupe.ico        application icon (generated, see make_icon.ps1)
  make_icon.ps1    GDI+ script that (re)generates loupe.ico
app.rc             resource script: embeds loupe.ico as resource id 1
build.rs           embeds `app.rc` (MSVC: embed-resource + `rc.exe`; GNU: `windres`)
```

## Regenerating the icon

```powershell
powershell -ExecutionPolicy Bypass -File assets\make_icon.ps1
```

Then run `cargo build` again — the build script picks up the new `loupe.ico`
automatically.

## Development

```powershell
cargo fmt --check
cargo clippy --all-features -- -D warnings
cargo build
```

## License

Dual-licensed under MIT or Apache-2.0, at your option.
