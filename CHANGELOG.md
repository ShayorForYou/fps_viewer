# Changelog

All notable changes to FPS Viewer. Each release's notes on GitHub come from its section here.

## [1.0.0] - 2026-11-18

The first release of FPS Viewer, rebuilt from the ground up in Rust as a single ~0.5 MB app.

### Highlights

- **Accurate, anti-cheat-friendly frame rate.** Reads the same GPU *present* events as PresentMon, through Windows' event tracing. Nothing is injected into games.
- **Three overlay styles.**
  - **Minimal:** just FPS.
  - **Compact:** FPS, CPU and RAM on one line.
  - **Detailed:** adds the game's name and a 4-second frame-time graph that marks stutters.
- **CPU and memory at a glance.** Whole-PC CPU use and RAM shown as used of total, for example *9.1 / 16 GB*.
- **Drag it anywhere.** Unlock with Alt+Shift+M, drop it in any spot, including screen corners, and it remembers where. Or pick one of six anchor points.
- **Installer or portable.** The installer puts FPS Viewer in Program Files, opens it without a UAC prompt every time, and can start it with Windows. The portable exe needs no install.

### Also included

- Five accent themes (Aurora, Mint, Sunset, Ocean, Mono), plus size (60–200 %) and opacity (30–100 %).
- Colors relative to your target frame rate. On Auto, the target is the display's refresh rate.
- *Hide when idle*, *Fullscreen games only*, and optional Vulkan & OpenGL support.
- *Battery saver*, an adjustable update rate, Efficiency mode and below-normal priority, so it stays out of your game's way.
- *Hide from screenshots & recordings*.
- Tray icon, global shortcuts and a Windows 11-style settings window with a live preview.
