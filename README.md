<div align="center">

<img src="docs/icon.png" width="96" height="96" alt="FPS Viewer icon">

# FPS Viewer

**A featherweight frame-rate overlay for Windows 10 and 11.**<br>
Accurate FPS, CPU and memory in a small glassy card: no injection, no bloat, about 0.5 MB.

[![Latest release](https://img.shields.io/github/v/release/ShayorForYou/fps_viewer?label=download&color=8B5CF6)](https://github.com/ShayorForYou/fps_viewer/releases/latest)
[![CI](https://github.com/ShayorForYou/fps_viewer/actions/workflows/ci.yml/badge.svg)](https://github.com/ShayorForYou/fps_viewer/actions/workflows/ci.yml)
![Windows 10 | 11](https://img.shields.io/badge/Windows-10%20%7C%2011-22D3EE)
![Size](https://img.shields.io/badge/size-~0.5%20MB-10B981)

[**Download**](https://github.com/ShayorForYou/fps_viewer/releases/latest) · [Features](#features) · [Install](#install) · [Shortcuts](#shortcuts) · [FAQ](#faq) · [Build from source](#build-from-source)

</div>

---

## Features

### Three overlay styles

| Style | Shows |
|---|---|
| **Minimal** | FPS, nothing else |
| **Compact** | FPS · CPU · RAM on a single line |
| **Detailed** | The game's name, a large FPS figure, CPU and RAM, and a 4-second frame-time graph that marks stutters |

Switch styles instantly with <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>S</kbd>.

### Accurate and safe with anti-cheat
- Reads the same GPU *present* events as PresentMon and CapFrameX, through Windows' built-in event tracing (ETW).
- **Never injects into games**, hooks them, or reads their memory. It runs as a separate window.
- DirectX 9, 10, 11 and 12 out of the box. Vulkan and OpenGL with one switch.

### Readable at a glance
- **Colors that mean something:** green at 90 % of your target frame rate or better, amber above 50 %, red below. *Auto* targets your monitor's refresh rate.
- **Whole-PC CPU and memory,** with RAM shown as used of total, for example *9.1 / 16 GB*.
- **Frame-time graph:** a flat line is smooth; red dots are stutters.

### Yours to arrange
- Six anchor points, or **drag it anywhere**, corners included. It remembers the spot and follows your game across monitors.
- Five accent themes: Aurora, Mint, Sunset, Ocean, Mono.
- Size from 60 to 200 % and opacity from 30 to 100 %.
- Optionally hidden from screenshots, recordings and streams.

### Light on your PC
- About 0.5 MB, with no runtime to install.
- Draws its overlay on the CPU in a fraction of a millisecond, so it never competes with your game for the GPU.
- Runs in Windows *Efficiency mode* at below-normal priority.
- Updates 1, 2 or 4 times a second, with a *Battery saver* mode.
- *Hide when idle* and *Fullscreen games only* keep it out of the way when you're not gaming.
- No network access, no telemetry, no accounts.

---

## Install

Download from the [**latest release**](https://github.com/ShayorForYou/fps_viewer/releases/latest):

| File | Best for |
|---|---|
| **`FpsViewer-Setup-x64.exe`** (recommended) | Everyday use. Installs to Program Files, adds a Start menu entry, **opens without a UAC prompt** and can **start with Windows**. Uninstall from *Settings → Apps*. |
| `FpsViewer-x64.exe` | Portable. Nothing to install; just run it. It asks for administrator rights each time it opens and can't start with Windows. |

**Requirements:** 64-bit Windows 10 or 11.

> [!NOTE]
> Releases aren't code-signed yet, so Windows SmartScreen may say *"Windows protected your PC"*. Click **More info → Run anyway**.
> Each release includes `SHA256SUMS.txt` to verify your download:
> ```powershell
> Get-FileHash .\FpsViewer-Setup-x64.exe
> ```

### Getting started

1. Run the setup and keep **Start FPS Viewer when I sign in** ticked if you'd like it always ready.
2. FPS Viewer opens its settings once, then lives in the **tray**, next to the clock.
3. Start a game: the overlay appears as soon as the game draws frames.
4. Press <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd> any time to change the style, colors or position.

---

## Shortcuts

| Keys | Action |
|---|---|
| <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>F</kbd> | Show / hide the overlay |
| <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>M</kbd> | Unlock the overlay to drag it, then press again to lock |
| <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>S</kbd> | Cycle Minimal → Compact → Detailed |
| <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd> | Open settings |

---

## FAQ

<details>
<summary><b>Why does it need administrator rights?</b></summary>

Windows only lets elevated apps listen to the real-time graphics events FPS Viewer measures. The installed version is granted that once, during setup. After that it opens from the Start menu or at sign-in with no prompt. The portable exe asks each time it opens.
</details>

<details>
<summary><b>Will it get me banned by anti-cheat?</b></summary>

FPS Viewer doesn't inject code, hook the game or read its memory, which is what anti-cheat systems look for. It's a normal window drawn on top of the game, like Xbox Game Bar. No third-party tool can promise how every anti-cheat behaves. In games with strict anti-cheat (Valorant, FACEIT, ESEA), keep **Hide from screenshots & recordings** off. It's off by default.
</details>

<details>
<summary><b>The overlay doesn't appear in my game.</b></summary>

- Set the game to **borderless** or **windowed fullscreen**. Nothing can draw over true *exclusive* fullscreen in older DirectX 9 and 11 games.
- Vulkan or OpenGL game? Turn on **Vulkan & OpenGL support** in settings.
- With **Fullscreen games only** or **Hide when idle** on, the overlay only shows while a full-screen game is rendering.
- Make sure the overlay is switched on: <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>F</kbd>.
</details>

<details>
<summary><b>What do CPU and RAM measure?</b></summary>

The whole PC, not just the game: how busy your processor is across all cores, and how much memory is in use out of the total installed.
</details>

<details>
<summary><b>Where are my settings? How do I uninstall?</b></summary>

Settings and a small diagnostics log live in `%APPDATA%\FpsViewer`. Uninstall the installed version from **Settings → Apps → FPS Viewer**, which also removes its startup entry. For the portable exe, just delete it. Your settings folder is kept in both cases; delete it by hand if you want a clean slate.
</details>

---

## Build from source

FPS Viewer is written in Rust against the Win32 APIs: Direct2D, DirectWrite and ETW. It can be built on Windows, or cross-compiled from macOS and Linux:

```bash
cd fps-viewer-rs
./release.sh --unsigned      # → dist/FpsViewer-x64.exe, dist/FpsViewer-Setup-x64.exe, SHA256SUMS.txt
```

See [`fps-viewer-rs/README.md`](fps-viewer-rs/README.md) for the toolchain setup, the code layout and performance notes.

### Branches and releases

| Branch | Purpose |
|---|---|
| `main` | Development. Every push is built by [CI](.github/workflows/ci.yml); the exe and setup are attached to the run as artifacts. |
| `production` | Releases. Every push builds both files and publishes a [GitHub Release](https://github.com/ShayorForYou/fps_viewer/releases) with notes from [`CHANGELOG.md`](CHANGELOG.md). |

To ship a new version:

1. Bump `version` in `fps-viewer-rs/Cargo.toml`. The exe's version info and the installer pick it up automatically.
2. Add a `## [x.y.z] - date` section to `CHANGELOG.md`.
3. Merge `main` into `production` and push. The release appears as `vX.Y.Z` a few minutes later.
