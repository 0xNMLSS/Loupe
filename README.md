# lens

A small, native Windows magnifier. Pick a region of your screen with a global
hotkey or the tray icon and watch it live, zoomed in, inside a floating
always-on-top window. Built in Rust on top of the Win32 Magnification API
(`windows-rs`), no GUI framework, no Electron.

## Features (v0.1)

- Per-monitor DPI v2 awareness — sharp on any scaling factor.
- Always-on-top, resizable, draggable magnifier window.
- Drag-to-select source rectangle on a transparent fullscreen overlay.
- Live update of the magnified view (~60 Hz) as the source area changes.
- Activation via global hotkey `Ctrl + Alt + Z` and tray icon menu.
- Clean teardown: hotkey unregistered, tray icon removed, magnifier
  uninitialized.

## Build

Requires the MSVC Rust toolchain on Windows 10/11.

```powershell
cargo build --release
```

The binary lands at `target\release\lens.exe`.

## Run

```powershell
cargo run --release
```

Then press `Ctrl + Alt + Z` (or right-click the tray icon → *New lens*),
drag a rectangle on screen, release the mouse, and the magnified view
appears.

Press `Esc` during region selection to cancel.

## Project layout

```
src/
  main.rs        entry point, message loop, host window
  hotkey.rs      global hotkey registration
  tray.rs        notification-area icon and menu
  region.rs      transparent overlay for region selection
  magnifier.rs   WC_MAGNIFIER child + uniform-scale transform
  dpi.rs         per-monitor DPI v2 awareness
assets/
  lens.ico       application icon (generated, see make_icon.ps1)
  make_icon.ps1  GDI+ script that (re)generates lens.ico
app.rc           resource script: embeds lens.ico as resource id 1
build.rs         compiles app.rc via the embed-resource crate
```

## Regenerating the icon

If you tweak the icon design, regenerate the `.ico` from PowerShell at the
repository root:

```powershell
powershell -ExecutionPolicy Bypass -File assets\make_icon.ps1
```

Then run `cargo build` again — the build script picks up the new `lens.ico`
automatically.

## Development

```powershell
cargo fmt --check
cargo clippy -- -D warnings
cargo build
```

## License

Dual-licensed under MIT or Apache-2.0, at your option.
