# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **Selection rectangle == on-desktop red frame, pixel-for-pixel** (P0 UX
  guarantee). The Classic-renderer 1.5× **cap** is no longer applied in
  `apply_source` or `WM_SIZE`, so the source rectangle (and the persistent
  red frame) is now *literally* the rectangle the user dragged — same
  position, same width, same height. The cap was the sole reason
  `current_source` could differ from the rainbow selection, and from the
  red frame the user sees on the desktop. `apply_source` still calls
  `match_loupe_aspect_to_selection` to reshape the loupe client area to
  the selection's aspect ratio while preserving total area (anchored at
  the current top-left, clamped to the monitor work area; no-op while in
  fullscreen), so for any reasonable selection the natural fit on Classic
  remains comfortably above 1.5× and there is no flicker. **Trade-off**:
  if the user picks an extreme selection or shrinks the loupe so much
  that the resulting scale falls below 1.5× on Classic, the picture may
  flicker (use the GPU renderer in that case) — but the red frame still
  tracks `current_source` 1:1, never silently shrinking around its centre
  the way the cap used to (`src/main.rs`).

- **Wheel zoom-out now stops at the Classic 1.5× floor instead of silently
  capping**: scrolling beyond the limit used to keep shrinking
  `current_source` while the cap snapped it back, so the on-desktop red
  frame would suddenly shrink and stop tracking the source. Once the next
  zoom-out step would trip the cap on Classic, `wheel_zoom_source` now
  returns immediately — the wheel feels "blocked" but
  `current_source` and the red frame stay in lock-step (`src/main.rs`).

- **Layered source-frame ghost trails after `MoveWindow`**: shrinking the
  red outline (e.g. via wheel zoom) left **concentric stale-border rings**
  on the desktop because the DWM compositor doesn't always invalidate the
  area uncovered by an `LWA_COLORKEY` layered window. `source_frame::move_to`
  / `hide` now call `RedrawWindow(NULL, old ∪ new, RDW_INVALIDATE |
  RDW_ERASE | RDW_ALLCHILDREN)` to force every top-level window in the
  affected screen rectangle to repaint and overwrite the stale red pixels
  (`src/source_frame.rs`).

- **Source-frame `CreateWindowExW` failed with `ERROR_MENU_HANDLE` (1401)**:
  the `hMenu` slot was being passed a child-window id, which is only valid
  for `WS_CHILD`; the source frame is `WS_POPUP` (top-level) so Windows
  rejected the call. Now passes `None` (`src/source_frame.rs`).

### Added

- **Persistent on-desktop source frame**: after a region is selected, a thin
  **red outline** is now drawn directly on the desktop at the source
  rectangle's native screen coordinates (`src/source_frame.rs`). The frame is
  always-topmost, **click-through in the middle** (apps underneath stay
  interactive), and **draggable by the border** — clicking the red edge and
  moving the mouse pans the source rect. The drag uses absolute math
  (`start_source_top_left + (mouse_now - start_mouse)`) so the host can
  clamp / reposition the frame mid-drag without the delta drifting.
  Wheel-zoom and host-resize keep the frame in sync via
  `source_frame::move_to`. The frame is **excluded from screen capture** via
  `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` (Windows 10 v2004+) so it
  never appears inside the magnified view, regardless of renderer.
- **Tray menu — *Show source frame*** toggles the on-desktop outline. State
  defaults to *visible* and is in-memory only (not persisted to
  `config.toml`). When toggled on with no current source, the frame stays
  hidden until the next region selection (`src/tray.rs`, `src/main.rs`).

### Changed

- **In-magnifier drag-to-pan removed**: the previous left/right-button drag
  on the hit overlay used reverse-direction math (drag right → source moves
  left → magnified content moves right) which was confusing. Panning is now
  exclusively done by dragging the on-desktop red **source frame** (same
  direction as the mouse). The hit overlay keeps its other roles —
  `WM_LBUTTONDBLCLK` for fullscreen toggle and `WM_MOUSEWHEEL` for zoom.
  `WM_APP_PAN_DELTA` is gone; `WM_APP_SOURCE_MOVE_TO` (absolute new
  top-left) takes its place (`src/source_frame.rs`, `src/main.rs`,
  `src/magnifier/legacy.rs`).

- **Tray icon**: a **single left-click** now starts **region selection** (same as
  *New loupe* / the global hotkey). The configuration menu opens on **right-click**
  only (`src/main.rs`). Tooltip text updated (`src/tray.rs`).

- **Windows subsystem**: Local `cargo run` / `cargo build` now keeps the **console**
  attached by default so logs (`eprintln!`, etc.) are visible. Release builds
  use Cargo feature **`hide_console`** (`#![windows_subsystem = "windows"]`);
  GitHub Actions `release.yml` runs `cargo build --release --features hide_console`.
  `cargo clippy` in CI uses `--all-features` so both configurations are checked.

- **README — Build**: Document MSVC, GNU (MSYS2), and LLVM-MinGW (gnullvm) setup;
  explain **linker `x86_64-w64-mingw32-clang` not found** and `rustup default`
  alternatives; GNU section notes **`dlltool.exe` not found** (MinGW `bin` must
  be on `PATH` outside MSYS2).

### Changed

- **Classic renderer — minimum magnification clamped to 1.5×**: `WC_MAGNIFIER`
  becomes visibly jittery (intermittent black specks / dropped frames) once
  the magnification factor approaches 1.0×, with the exact breakdown threshold
  drifting between systems. Wheel-zoom, drag-region and host-resize now cap
  the source rectangle so the resulting scale is **at least 1.5×** on the
  Classic path (`MIN_CLASSIC_SCALE` in `src/main.rs`). Sub-1.5× overviews are
  available on the GPU renderer (D3D11 + Lanczos), which has no such
  limitation — switch via tray → right-click → *Renderer* → *GPU*; the
  choice is remembered in `config.toml`.

### Fixed

- **GPU renderer — pan / zoom / resize / fullscreen had no visible effect on a
  static desktop**: the GPU path (`WGC` + `D3D11`) only re-renders when WGC
  pushes a new frame via `FrameArrived`. With nothing moving on screen, mouse
  pan, wheel zoom, window resize and the fullscreen toggle would all silently
  update internal state (source rect, swap-chain size) but the picture would
  freeze on the last captured frame — the user perceived this as "mouse
  signals failing" (only double-click, which moves the OS-level window,
  appeared to work). Each `on_frame` now `CopyResource`s the WGC pool texture
  into a self-owned `cached_tex` (`src/magnifier/gpu/mod.rs`), and
  `GpuRenderer::resize_to` / `fit_source` / `set_source` call a new
  `redraw()` that re-renders from `cached_tex` with the latest
  source/viewport. The cache is dropped when `rebuild()` switches the
  captured monitor so the first redraw after a switch isn't stale.

- **Classic magnifier — aspect ratio / “wrong region” after resize or fullscreen**:
  the ~60 Hz refresh called only `MagSetWindowSource`, which clears the custom
  `MagSetWindowTransform` on many Windows builds so the control falls back to
  stretching the source to the client. `Renderer::set_source` for classic now
  re-applies `fit_source` using the host’s client size (`GetParent` +
  `GetClientRect`) so uniform scale and letterboxing persist (`src/magnifier/mod.rs`).

- **Classic magnifier — letterboxed view not centered** (e.g. portrait fullscreen):
  `fit_source` applies offsets `ox`/`oy` in **`MAGTRANSFORM` columns `v[2]` and `v[5]`**
  (third column of the affine matrix), not `v[6]`/`v[7]`, matching Win32 `M * [x,y,1]ᵀ`
  layout (`src/magnifier/legacy.rs`).

- **Main window creation failed** (`failed to create main window`): child
  `CreateWindowExW` calls for `WC_MAGNIFIER` and the hit-test overlay now pass
  the module **`HINSTANCE`** (`Some(instance)` instead of `None`), matching the
  Microsoft Magnification sample. If the overlay still cannot be created, the
  app starts without it and prints a warning (double-click fullscreen disabled)
  instead of aborting the whole host window.

- **Hit overlay `CreateWindowExW` failed with ERROR_INVALID_HANDLE (6)**:
  creating the child with `WS_EX_LAYERED | WS_EX_NOACTIVATE` was rejected on
  some setups. The overlay is now created as a normal child, then
  `WS_EX_LAYERED` is applied via `GWL_EXSTYLE` before `SetLayeredWindowAttributes`.

- **Docs — build failures on Windows**: README troubleshooting now covers
  `cargo` not on `PATH` (PowerShell), **`link.exe` not found** (MSVC tools not
  installed / wrong shell), and **`unable to find library -lgcc_eh` / `-lgcc`**
  (GNU / LLVM-MinGW layout).

- **GNU / gnullvm builds — `windres` not found**: `embed-resource` only invoked
  plain `windres` on non-MSVC Windows, which often fails under Git Bash when
  MinGW `bin` is not on `PATH`. `build.rs` now resolves `windres` via `WINDRES`,
  `PATH`, `x86_64-w64-mingw32-windres`-style names, and common MSYS2 `bin`
  directories, while keeping `embed-resource` for MSVC (`rc.exe`).

### Added

- **Wheel zoom on the magnified view**: scroll the mouse wheel over the
  magnified area to zoom in or out (scales the source `RECT` about its center;
  same hit overlay as double-click / right-drag). Posted as `WM_APP_WHEEL_ZOOM`
  (`src/magnifier/legacy.rs`, `src/main.rs`).

- **Pan the magnified region**: hold **left or right mouse button** on the
  magnified view and drag to slide the source rectangle on the virtual desktop
  (uniform scale matches resize; requires the hit overlay). Implemented via
  `WM_APP_PAN_DELTA` (`src/magnifier/legacy.rs`, `src/main.rs`).

- **Optional GPU renderer** (`src/magnifier/gpu/`): Windows.Graphics.Capture screen
  capture, D3D11 swapchain on a child window, and Lanczos-3 upsampling (HLSL
  compiled at runtime with `D3DCompile`). Tray menu *Renderer* switches between
  **Classic** (`WC_MAGNIFIER`) and **GPU**; `renderer` in `%APPDATA%\loupe\config.toml`
  persists the choice. If WGC or D3D11 setup fails, the app falls back to Classic.

- **Double-click for borderless fullscreen**: double-click the magnified view
  to toggle borderless fullscreen (strips title bar and borders, covers the
  entire monitor). The window style and placement are saved before entering
  fullscreen and fully restored on the second double-click.
  `WC_MAGNIFIER` does not receive mouse hits (input passes through), so a
  nearly transparent layered child (`loupe.hit`, `CS_DBLCLKS`) sits above the
  magnifier, handles `WM_LBUTTONDBLCLK`, and posts `WM_APP_TOGGLE_FULLSCREEN`
  to the host (`src/magnifier.rs`, `src/main.rs`).

- **Hotkey persistence** (`src/config.rs`). The bound hotkey is saved to
  `%APPDATA%\loupe\config.toml` immediately after a successful
  `RegisterHotKey` call. On the next launch Loupe reads the file and
  re-registers the hotkey automatically — no need to rebind every session.
  The config file is plain text and can be deleted to reset the setting.

### Changed

- Application renamed from **lens** to **Loupe**. Binary is now `loupe.exe`;
  window class names updated to `loupe.*`; icon file renamed to `loupe.ico`.

### Fixed

- **Hotkey bind dialog — text truncated**: window was too short (130 px) to show all
  instructions. Increased to 200 × 400 px so all text fits comfortably.
- **Hotkey bind dialog — no current binding shown**: dialog now displays
  "Current shortcut: Ctrl+Alt+F1" (or "(none — not set)") so the user always
  knows what shortcut is currently active. Uses `GetKeyNameTextW` +
  `MapVirtualKeyW` for locale-aware key labels.
- `AppState` now stores `current_hotkey: Option<(HOT_KEY_MODIFIERS, u32)>`
  updated on every successful `RegisterHotKey` and passed into
  `hotkey_bind::show()`.

### Changed

- Region-selection overlay is now fully opaque (removed `WS_EX_LAYERED` /
  `LWA_ALPHA`). The background is a solid dark charcoal (`#282828`); the
  selected rectangle interior renders in a lighter gray (`#585858`) so the
  chosen area is visually distinct without requiring see-through blending.
- Selection border is now animated rainbow: an HSV hue cycles 0→359° at
  ~25 fps (40 ms `WM_TIMER`), converted to a full-saturation/value `COLORREF`
  and drawn via four `FillRect` bands at 4-pixel thickness.
- Hotkey binding is now user-driven rather than hardcoded.  
  At startup no global hotkey is registered.  
  Right-clicking the tray icon now shows three items:
  - **New lens** — start a selection immediately
  - **Bind hotkey…** — opens a small always-on-top capture window; press any
    modifier (`Ctrl`/`Alt`/`Shift`) + non-modifier key to set the shortcut;
    `Esc` cancels. The chosen combo is registered via `RegisterHotKey` with
    `MOD_NOREPEAT`.
  - **Quit**

### Previously Changed

- Region-selection overlay now draws a 4-pixel saturated-amber border
  (RGB 255, 229, 0) instead of the previous 1-pixel white `FrameRect`. The
  border is composited from four `FillRect` bands so the thickness is
  exact and predictable on any DPI, and the colour reads cleanly against
  both light and dark desktops.

### Fixed

- Resizing the lens window now scales the magnified content to fill the new
  client area while preserving aspect ratio. Previously the magnifier kept a
  1:1 transform regardless of host size, so the source rect was either
  cropped or shown in a corner. Implemented via `MagSetWindowTransform`
  (uniform-scale matrix derived from `client_size / source_size`) applied on
  every `WM_SIZE` and on every new region selection.

### Added

- Application icon. A custom magnifying-glass `lens.ico` is generated by
  `assets/make_icon.ps1` (256×256 anti-aliased GDI+ render, PNG-payload ICO),
  embedded into `lens.exe` via `embed-resource` build script + `app.rc`,
  and applied to both the main window class (`WNDCLASSW.hIcon`) and the
  notification-area icon (`NOTIFYICONDATAW.hIcon`).

## [0.1.0] - 2026-04-23

### Added

- Bootstrap of the `lens` binary crate with the `windows` 0.62 dependency
  enabling Win32 Magnification, Shell, HiDpi, GDI, WindowsAndMessaging,
  KeyboardAndMouse, and LibraryLoader feature gates.
- Per-monitor DPI v2 awareness via `SetProcessDpiAwarenessContext` so the
  magnifier renders crisply on mixed-DPI multi-monitor setups.
- Always-on-top, resizable, draggable host window backed by a `WC_MAGNIFIER`
  child control that resizes with `WM_SIZE`.
- Transparent fullscreen overlay for drag-to-select region capture, with
  Escape-to-cancel and a dim-with-clear-cut-out preview while dragging.
- Live ~60 Hz refresh of the magnified view via `SetTimer` +
  `MagSetWindowSource`.
- Global hotkey `Ctrl + Alt + Z` registered through `RegisterHotKey` to start
  a new lens session.
- Notification-area tray icon with a right-click menu (`New lens`, `Quit`).
- Graceful teardown: hotkey unregistered, tray icon removed,
  `MagUninitialize` called, refresh timer killed.
- `release` profile tuned for size (`opt-level = "z"`, `lto = true`,
  `codegen-units = 1`, `strip = true`, `panic = "abort"`) — produces a
  ~245 KB single-file `lens.exe`.

### Toolchain

- Targets `x86_64-pc-windows-gnullvm`, linked with `llvm-mingw` (no Visual
  Studio Build Tools required, no external mingw runtime needed).
