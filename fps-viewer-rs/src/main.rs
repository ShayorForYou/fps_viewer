//! FPS Viewer — a featherweight frame-rate overlay for Windows 10/11.
//!
//! Reads GPU present events through ETW (like PresentMon), never injects into games, and
//! draws a small glassy overlay with Direct2D. See README.md for details.

#![windows_subsystem = "windows"]
// `PCWSTR(1 as *const u16)` is Win32's MAKEINTRESOURCE(1) (the app icon), not a real pointer.
#![allow(clippy::manual_dangling_ptr)]

mod app;
mod etw;
mod gfx;
mod overlay;
mod overlay_draw;
mod settings;
mod settings_ui;
mod stats;
mod sys;
mod tray;
mod util;

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, ReleaseMutex, SetEvent};

fn main() {
    std::panic::set_hook(Box::new(|info| util::log(&format!("Fatal: {info}"))));

    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a.eq_ignore_ascii_case(flag));

    // Used by the installer, which runs elevated. Exit code 0 = success.
    if has("--install") {
        let mut ok = sys::launcher_register();
        if has("--startup") {
            ok &= sys::startup_set_enabled(true);
        } else if has("--no-startup") {
            ok &= sys::startup_set_enabled(false);
        }
        std::process::exit(if ok { 0 } else { 1 });
    }
    if has("--uninstall") {
        std::process::exit(if sys::remove_tasks() { 0 } else { 1 });
    }

    // The exe starts as the user; reading frame timings needs admin rights. Hand off to an
    // elevated copy — silently through the launch task when installed, else via UAC.
    if !sys::is_elevated() && sys::relaunch_elevated(&args) {
        return;
    }

    let background = has("--background");

    unsafe {
        // Single instance: a second launch just brings up the running instance's settings.
        let mutex = CreateMutexW(None, true, w!("Local\\FpsViewer.Instance"));
        let already_running = GetLastError() == ERROR_ALREADY_EXISTS;
        let Ok(show_signal) = CreateEventW(None, false, false, w!("Local\\FpsViewer.Show")) else { return };

        if already_running {
            let _ = SetEvent(show_signal);
        } else {
            app::run(background, show_signal);
        }

        let _ = CloseHandle(show_signal);
        if let Ok(m) = mutex {
            if !already_running {
                let _ = ReleaseMutex(m);
            }
            let _ = CloseHandle(m);
        }
    }
}
