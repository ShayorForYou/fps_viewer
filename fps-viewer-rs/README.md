# FPS Viewer (Rust)

A native Rust rewrite of FPS Viewer: the same overlay, features, settings file and design as
the C#/WPF version, in a single ~0.5 MB `.exe` that needs nothing installed (no .NET, no
Visual C++ runtime).

| | C# / WPF (self-contained) | Rust |
|---|---|---|
| `FpsViewer.exe` (x64) | 68 MB | **~0.55 MB** |
| Runtime needed | none (bundled) | none |
| Dependencies | .NET 9, WPF, WinForms | `windows` crate (Microsoft's Win32 bindings) |

## Build

### From macOS or Linux (cross-compile)

One-time setup:

```bash
brew install rustup llvm lld          # or your distro's rustup + llvm + lld
rustup-init -y
rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc
cargo install cargo-xwin --locked     # downloads the MSVC CRT + Windows SDK libs on first use
```

Build:

```bash
cargo xwin build --release --target x86_64-pc-windows-msvc    # Intel/AMD PCs
cargo xwin build --release --target aarch64-pc-windows-msvc   # Snapdragon / ARM64 PCs
```

Output: `target/<target>/release/FpsViewer.exe`.

`build.rs` compiles `assets/app.rc` (icon, UAC + DPI manifest, version info) with `llvm-rc`.
It looks in Homebrew's LLVM automatically; set `RC=/path/to/llvm-rc` if yours lives elsewhere.

### On Windows

Install [Rust](https://rustup.rs) and the *Visual Studio Build Tools* (workload "Desktop
development with C++", which includes the Windows SDK), then:

```powershell
cargo build --release
```

`build.rs` finds `rc.exe` from the Windows SDK (or `llvm-rc` if LLVM is installed).

### Run

The app needs administrator rights (the manifest asks for them), because Windows only lets
elevated processes open real-time ETW sessions. Run `FpsViewer.exe` and accept the UAC prompt;
`FpsViewer.exe --background` starts silently to the tray (used by "Start with Windows").

## Layout

```
src/
├─ main.rs          Entry point, single-instance guard
├─ app.rs           App state, message loop, hotkeys, focus tracking, power/display events
├─ etw.rs           ETW real-time session; kernel-filtered present events; lock-light callback
├─ stats.rs         FPS, frame time, graph buckets — zero allocations per update
├─ overlay.rs       Click-through layered window, software-rendered (never touches the game's GPU)
├─ overlay_draw.rs  The three overlay styles + frame-time graph (shared with the settings preview)
├─ settings_ui.rs   Mica settings window: cards, toggles, segments, sliders, live preview
├─ gfx.rs           Direct2D/DirectWrite helpers, fonts, brush caches
├─ tray.rs          Tray icon + native dark context menu
├─ settings.rs      Settings model + JSON file (same format as the C# version)
├─ sys.rs           Process names, monitors, CPU/RAM load, EcoQoS, working-set trim, startup task
└─ util.rs          Strings, QPC clock, log file, window-class helpers
```

## Performance notes

- **Overlay rendering** uses Direct2D's software rasterizer into a small DIB and
  `UpdateLayeredWindow`, so it costs a fraction of a millisecond 1–4× per second and never
  competes with the game for the GPU or creates a Direct3D device.
- **The settings window** uses a hardware render target only while it is open; closing it
  releases the device and trims the working set.
- **ETW events** are filtered in the kernel to four event IDs, and the callback rejects other
  processes with a single atomic compare before touching any lock.
- **Stats** reuse buffers allocated once at startup. CPU and RAM come from two cheap system
  calls (`GetSystemTimes`, `GlobalMemoryStatusEx`) once per update.
- The process runs at below-normal priority with EcoQoS ("Efficiency mode"), and its timers
  are coalescable so Windows can batch wake-ups.

## Differences from the C# version

- Event timestamps are requested in raw QPC units (`PROCESS_TRACE_MODE_RAW_TIMESTAMP`). Without
  it ETW converts them to system time, which can't be compared with the QPC clock and could stop
  "Hide when idle" from noticing that a game had stopped rendering.
- The tray menu is the native Windows menu in dark mode, instead of a custom-drawn WinForms menu.
- The settings window uses the standard Windows 11 title bar (with Mica and dark mode) instead
  of a custom-drawn title strip.
- Tooltips on the accent swatches and position dots aren't shown.
