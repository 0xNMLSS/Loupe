# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
