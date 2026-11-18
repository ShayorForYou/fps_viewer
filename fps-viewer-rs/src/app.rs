//! The application: owns all state, reacts to focus changes, hotkeys, timers, power and
//! display events, and applies settings changes. Everything runs on one UI thread (plus the
//! ETW consumer thread), driven by a single message loop.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use windows::core::w;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Threading::{GetCurrentProcessId, INFINITE};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_ALT, MOD_NOREPEAT, MOD_SHIFT};
use windows::Win32::UI::Shell::NIN_SELECT;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::etw;
use crate::gfx::Gfx;
use crate::overlay::Overlay;
use crate::overlay_draw::OverlayModel;
use crate::settings::{self, OverlayPosition, OverlayStyle, Settings};
use crate::settings_ui::{Action, SettingsWindow, Slider, Status, StatusKind};
use crate::stats::{DemoFeed, Engine, FrameStats};
use crate::sys;
use crate::tray::{self, Tray};
use crate::util::{hinstance, log, register_class};

pub const WM_APP_TRAY: u32 = WM_APP + 1;
pub const WM_APP_OVERLAY_MOVED: u32 = WM_APP + 2;
/// wParam: bit 0 = requested state, bit 1 = success.
const WM_APP_STARTUP_RESULT: u32 = WM_APP + 3;
/// wParam: whether the startup task currently exists.
const WM_APP_STARTUP_QUERY: u32 = WM_APP + 4;

const TIMER_TICK: usize = 1;
const TIMER_SAVE: usize = 2;
const TOPMOST_EVERY_TICKS: u32 = 4;

const HK_TOGGLE: i32 = 1;
const HK_MOVE: i32 = 2;
const HK_STYLE: i32 = 3;
const HK_SETTINGS: i32 = 4;

const NIN_KEYSELECT: u32 = NIN_SELECT + 1;

/// Shell surfaces that occasionally present frames but are never what the user wants measured.
const SHELL_PROCESSES: [&str; 12] = [
    "explorer",
    "ShellExperienceHost",
    "StartMenuExperienceHost",
    "SearchHost",
    "SearchApp",
    "TextInputHost",
    "LockApp",
    "dwm",
    "ShellHost",
    "Widgets",
    "WidgetBoard",
    "PeopleExperienceHost",
];

static MAIN_HWND: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app state. Returns `None` while the state is already borrowed (a window
/// message re-entered us, e.g. during window creation) — callers then fall back to defaults.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

/// Posts a message to the app's hidden main window.
pub fn post(msg: u32, wparam: usize) {
    let hwnd = HWND(MAIN_HWND.load(Ordering::Relaxed));
    if !hwnd.is_invalid() {
        unsafe {
            let _ = PostMessageW(Some(hwnd), msg, WPARAM(wparam), LPARAM(0));
        }
    }
}

/// Routes a settings-window message to the app.
pub fn settings_message(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    with_app(|a| a.on_settings_message(hwnd, msg, wp, lp)).flatten()
}

/// Creates everything, runs the message loop and tears down. `show_signal` is the event a
/// second launch sets to bring up the settings window.
pub fn run(background: bool, show_signal: HANDLE) {
    unsafe {
        register_class(w!("FpsViewer.Main"), Some(main_wndproc), false);
        // A hidden top-level window (not message-only) so it also receives broadcasts:
        // power changes, display changes and Explorer restarts.
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("FpsViewer.Main"),
            w!("FPS Viewer"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance()),
            None,
        ) else {
            log("Could not create the main window.");
            return;
        };
        MAIN_HWND.store(hwnd.0, Ordering::Relaxed);

        let app = match App::new(hwnd) {
            Ok(a) => a,
            Err(e) => {
                log(&format!("Startup failed: {e}"));
                let text = crate::util::wide(&format!(
                    "FPS Viewer couldn't start its graphics engine (Direct2D).\n\n{e}\n\n\
                     This happens on systems without working Direct3D 10 support, such as Wine on macOS. \
                     On Windows 10/11, updating the graphics driver usually fixes it."
                ));
                MessageBoxW(None, windows::core::PCWSTR(text.as_ptr()), w!("FPS Viewer"), MB_ICONERROR | MB_OK);
                let _ = DestroyWindow(hwnd);
                return;
            }
        };
        TASKBAR_CREATED.store(app.tray.taskbar_created, Ordering::Relaxed);
        APP.with(|cell| *cell.borrow_mut() = Some(app));

        with_app(|a| {
            a.on_foreground(GetForegroundWindow());
            a.tick();
            if !background || a.settings.first_run {
                a.show_settings();
            }
        });

        // Message loop that also wakes for the "show settings" signal from a second launch.
        let handles = [show_signal];
        'outer: loop {
            let r = MsgWaitForMultipleObjectsEx(Some(&handles), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            if r == WAIT_OBJECT_0 {
                with_app(|a| a.show_settings());
            }
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    break 'outer;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        if let Some(app) = APP.with(|cell| cell.borrow_mut().take()) {
            app.shutdown();
        }
        MAIN_HWND.store(std::ptr::null_mut(), Ordering::Relaxed);
        let _ = DestroyWindow(hwnd);
    }
}

pub struct App {
    hwnd: HWND,
    gfx: Gfx,
    settings: Settings,
    overlay: Overlay,
    overlay_model: OverlayModel,
    preview: OverlayModel,
    status: Status,
    engine: Engine,
    demo: DemoFeed,
    load_sampler: sys::LoadSampler,
    load: sys::Load,
    settings_win: Option<SettingsWindow>,
    tray: Tray,
    hook: HWINEVENTHOOK,

    target_hwnd: HWND,
    target_pid: u32,
    target_name: String,
    refresh_rate: u32,
    on_battery: bool,
    monitor_error: Option<String>,

    last_stats: FrameStats,
    last_preview: FrameStats,
    ticks_since_topmost: u32,
    was_active: bool,
    tick_ms: u32,
    save_pending: bool,
    syncing_startup: bool,
}

impl App {
    fn new(hwnd: HWND) -> windows::core::Result<App> {
        sys::apply_power_saver();
        sys::enable_dark_menus();

        let settings = settings::load();
        let gfx = Gfx::new()?;
        let mut overlay = Overlay::new(&gfx)?;
        overlay.set_excluded_from_capture(settings.hide_from_capture);

        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_foreground_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };

        for (id, vk) in [(HK_TOGGLE, 'F'), (HK_MOVE, 'M'), (HK_STYLE, 'S'), (HK_SETTINGS, 'O')] {
            if unsafe { RegisterHotKey(Some(hwnd), id, MOD_ALT | MOD_SHIFT | MOD_NOREPEAT, vk as u32) }.is_err() {
                log(&format!("Hotkey Alt+Shift+{vk} is already in use by another app."));
            }
        }

        let mut app = App {
            hwnd,
            gfx,
            on_battery: sys::on_battery(),
            settings,
            overlay,
            overlay_model: OverlayModel::new("Waiting for a game"),
            preview: OverlayModel::new("Your game"),
            status: Status::default(),
            engine: Engine::new(),
            demo: DemoFeed::new(),
            load_sampler: sys::LoadSampler::new(),
            load: sys::Load::UNKNOWN,
            settings_win: None,
            tray: Tray::new(hwnd, WM_APP_TRAY),
            hook,
            target_hwnd: HWND::default(),
            target_pid: 0,
            target_name: String::new(),
            refresh_rate: 60,
            monitor_error: None,
            last_stats: FrameStats::INACTIVE,
            last_preview: FrameStats::INACTIVE,
            ticks_since_topmost: 0,
            was_active: false,
            tick_ms: 0,
            save_pending: false,
            syncing_startup: false,
        };
        app.apply_appearance();
        app.start_monitoring();
        app.update_tick_interval(false);
        Ok(app)
    }

    fn shutdown(mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_TICK);
            let _ = KillTimer(Some(self.hwnd), TIMER_SAVE);
            if !self.hook.is_invalid() {
                let _ = UnhookWinEvent(self.hook);
            }
        }
        if self.save_pending {
            settings::save(&self.settings);
        }
        self.settings_win = None;
        etw::stop();
        // The overlay window and tray icon are released by their Drop impls; hotkeys go
        // with the main window.
    }

    fn low_power(&self) -> bool {
        self.on_battery && self.settings.battery_saver
    }

    // ───────────────────────── monitoring ─────────────────────────

    fn start_monitoring(&mut self) {
        match etw::start(self.settings.vulkan_opengl_support) {
            Ok(()) => {
                self.monitor_error = None;
                etw::set_target(self.measured_pid());
            }
            Err(e) => {
                log(&format!("Monitoring unavailable: {e}"));
                self.monitor_error = Some(e);
            }
        }
    }

    /// The PID we actually record (0 when the shell is focused).
    fn measured_pid(&self) -> u32 {
        if self.target_name.is_empty() { 0 } else { self.target_pid }
    }

    fn on_foreground(&mut self, hwnd: HWND) {
        if hwnd.is_invalid() {
            return;
        }
        let pid = sys::window_pid(hwnd);
        if pid == 0 || pid == unsafe { GetCurrentProcessId() } {
            return; // our own windows keep the current target
        }

        self.target_hwnd = hwnd;
        if pid != self.target_pid {
            self.target_pid = pid;
            let path = sys::process_image_path(pid);
            let exe = path.as_deref().map(sys::file_stem).unwrap_or("");
            let is_shell = SHELL_PROCESSES.iter().any(|s| s.eq_ignore_ascii_case(exe));

            self.target_name = if is_shell { String::new() } else { sys::friendly_name(path.as_deref(), exe) };
            etw::set_target(self.measured_pid());
            self.overlay_model.app_name =
                if self.target_name.is_empty() { "Waiting for a game".into() } else { self.target_name.clone() };
            self.refresh_rate = sys::refresh_rate(hwnd);
            self.was_active = false;
        }

        self.overlay.place_on(hwnd, &self.settings);
        self.overlay.reassert_topmost();
    }

    fn target_fps(&self) -> f64 {
        if self.settings.target_fps > 0 {
            self.settings.target_fps as f64
        } else if self.refresh_rate > 0 {
            self.refresh_rate as f64
        } else {
            60.0
        }
    }

    /// Samples new numbers, then presents them.
    fn tick(&mut self) {
        let s = &self.settings;
        let want_graph = s.show_graph && !self.low_power() && s.style == OverlayStyle::Detailed;
        let mut stats =
            if self.monitor_error.is_none() { self.engine.compute(want_graph) } else { FrameStats::INACTIVE };
        if stats.active && s.fullscreen_only && !sys::is_fullscreen(self.target_hwnd) {
            stats = FrameStats::INACTIVE;
        }
        self.last_stats = stats;
        self.load = self.load_sampler.sample();
        if self.settings_win.is_some() {
            self.last_preview = if stats.active { stats } else { self.demo.next(self.target_fps()) };
        }
        self.present();
    }

    /// Redraws the overlay (and the settings preview) from the latest numbers.
    fn present(&mut self) {
        let target = self.target_fps();
        let stats = self.last_stats;
        let s = &self.settings;
        self.overlay_model.apply_stats(&stats, target, s.color_by_performance);
        self.overlay_model.apply_system(&self.load);

        let show = s.overlay_enabled && (stats.active || !s.auto_hide || self.overlay.move_mode());
        if show {
            self.overlay.render(&self.gfx, &self.overlay_model, self.target_hwnd, &self.settings);
            self.ticks_since_topmost += 1;
            if self.ticks_since_topmost >= TOPMOST_EVERY_TICKS {
                self.ticks_since_topmost = 0;
                self.overlay.reassert_topmost();
            }
        }
        self.overlay.set_shown(show);

        if stats.active != self.was_active {
            self.was_active = stats.active;
            let tip = if stats.active { format!("FPS Viewer — {}", self.target_name) } else { "FPS Viewer".into() };
            self.tray.set_tooltip(&tip);
        }

        if self.settings_win.is_some() {
            self.preview.app_name = if stats.active { self.target_name.clone() } else { "Your game".into() };
            let preview = self.last_preview;
            self.preview.apply_stats(&preview, target, self.settings.color_by_performance);
            self.preview.apply_system(&self.load);
            let low = self.low_power();
            update_status(&mut self.status, &self.settings, self.monitor_error.as_deref(), &stats, &self.target_name, target, low);
            if let Some(w) = &self.settings_win {
                w.invalidate();
            }
        }

        self.update_tick_interval(show);
    }

    fn update_tick_interval(&mut self, overlay_shown: bool) {
        let ms = if self.settings_win.is_some() {
            250 // live preview while configuring
        } else if overlay_shown && !self.low_power() {
            1000 / self.settings.update_hz.max(1)
        } else {
            1000 // idle / battery saver
        };
        if ms != self.tick_ms {
            self.tick_ms = ms;
            // Coalescable: Windows may batch our wake-ups with others to save power.
            unsafe { SetCoalescableTimer(Some(self.hwnd), TIMER_TICK, ms, None, ms / 10) };
        }
    }

    // ───────────────────────── settings ─────────────────────────

    fn apply_appearance(&mut self) {
        let low = self.low_power();
        self.overlay_model.apply_appearance(&self.settings, low);
        self.preview.apply_appearance(&self.settings, low);
    }

    /// Mutates settings, then reacts to whatever actually changed.
    fn change(&mut self, f: impl FnOnce(&mut Settings)) {
        let old = self.settings.clone();
        f(&mut self.settings);
        self.settings.normalize();
        if old == self.settings {
            return;
        }
        let new = self.settings.clone();

        self.save_pending = true;
        unsafe { SetTimer(Some(self.hwnd), TIMER_SAVE, 600, None) };

        if old.hide_from_capture != new.hide_from_capture {
            self.overlay.set_excluded_from_capture(new.hide_from_capture);
        }
        if old.vulkan_opengl_support != new.vulkan_opengl_support {
            etw::stop();
            self.start_monitoring();
        }
        if old.start_with_windows != new.start_with_windows && !self.syncing_startup {
            let enable = new.start_with_windows;
            spawn_small(move || {
                let ok = sys::startup_set_enabled(enable);
                post(WM_APP_STARTUP_RESULT, enable as usize | (ok as usize) << 1);
            });
        }
        if old.update_hz != new.update_hz {
            self.tick_ms = 0; // force the timer to be re-armed
        }
        self.apply_appearance();
        self.present();
    }

    fn apply_action(&mut self, action: Action) {
        match action {
            Action::Toggle(t, v) => self.change(|s| t.set(s, v)),
            Action::Style(v) => self.change(|s| s.style = v),
            Action::Target(v) => self.change(|s| s.target_fps = v),
            Action::Hz(v) => self.change(|s| s.update_hz = v),
            Action::Theme(i) => self.change(|s| s.theme = i),
            Action::Slider(Slider::Scale, v) => self.change(|s| s.scale = v),
            Action::Slider(Slider::Opacity, v) => self.change(|s| s.overlay_opacity = v),
            Action::Position(p) => self.change(|s| s.position = p),
            Action::Reset => self.change(|s| s.reset_appearance()),
            Action::Done => self.close_settings(),
            Action::Quit => quit(),
        }
    }

    fn show_settings(&mut self) {
        if self.settings_win.is_none() {
            let Some(win) = SettingsWindow::new(&self.gfx) else {
                log("Could not create the settings window.");
                return;
            };
            self.settings_win = Some(win);
            self.tick_ms = 0;
            // Reflect the real state of the startup task without blocking the UI.
            spawn_small(|| post(WM_APP_STARTUP_QUERY, sys::startup_is_enabled() as usize));
        }
        // Compute the preview and status first, so the very first frame is complete.
        self.tick();
        if let Some(w) = &mut self.settings_win {
            w.show(&self.gfx, &self.settings, &self.preview, &self.status);
        }
    }

    fn close_settings(&mut self) {
        if self.settings_win.take().is_none() {
            return;
        }
        if self.settings.first_run {
            self.change(|s| s.first_run = false);
            self.tray.balloon("FPS Viewer is running", "It lives in the tray. Press Alt+Shift+O to reopen settings.");
        }
        self.update_tick_interval(self.overlay.is_shown());
        // Hand the settings UI's memory back to Windows — we're about to sit idle for hours.
        sys::trim_working_set();
    }

    fn on_settings_message(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let win = self.settings_win.as_mut()?;
        if win.hwnd != hwnd {
            return None;
        }
        let (result, action) = win.handle(&self.gfx, &self.settings, &self.preview, &self.status, msg, wp, lp);
        if let Some(a) = action {
            self.apply_action(a);
        }
        result
    }

    // ───────────────────────── commands ─────────────────────────

    fn toggle_move_mode(&mut self) {
        let on = !self.overlay.move_mode();
        self.overlay.set_move_mode(on);
        self.overlay_model.edit_mode = on;
        if on && !self.settings.overlay_enabled {
            self.change(|s| s.overlay_enabled = true);
        } else {
            self.present();
        }
    }

    fn on_overlay_moved(&mut self) {
        if let Some((fx, fy)) = self.overlay.custom_fraction() {
            self.change(|s| {
                s.custom_x = fx;
                s.custom_y = fy;
                s.position = OverlayPosition::Custom;
            });
        }
    }

    fn on_hotkey(&mut self, id: i32) {
        match id {
            HK_TOGGLE => self.change(|s| s.overlay_enabled = !s.overlay_enabled),
            HK_MOVE => self.toggle_move_mode(),
            HK_STYLE => self.change(|s| s.style = s.style.next()),
            HK_SETTINGS => self.show_settings(),
            _ => {}
        }
    }

    fn on_tray_command(&mut self, cmd: u32) {
        match cmd {
            tray::CMD_SETTINGS => self.show_settings(),
            tray::CMD_TOGGLE_OVERLAY => self.change(|s| s.overlay_enabled = !s.overlay_enabled),
            tray::CMD_MOVE => self.toggle_move_mode(),
            tray::CMD_STYLE_MINIMAL => self.change(|s| s.style = OverlayStyle::Minimal),
            tray::CMD_STYLE_COMPACT => self.change(|s| s.style = OverlayStyle::Compact),
            tray::CMD_STYLE_DETAILED => self.change(|s| s.style = OverlayStyle::Detailed),
            tray::CMD_QUIT => quit(),
            _ => {}
        }
    }

    fn on_power(&mut self, event: u32) {
        if event == PBT_APMRESUMEAUTOMATIC && etw::is_running() {
            // ETW sessions can stall across sleep; restart to be safe.
            etw::stop();
            self.start_monitoring();
        }
        let on_battery = sys::on_battery();
        if on_battery != self.on_battery {
            self.on_battery = on_battery;
            self.apply_appearance();
            self.present();
        }
    }

    fn on_display_change(&mut self) {
        if !self.target_hwnd.is_invalid() {
            self.refresh_rate = sys::refresh_rate(self.target_hwnd);
        }
        self.present();
    }

    fn on_startup_result(&mut self, requested: bool, ok: bool) {
        if ok {
            return;
        }
        self.syncing_startup = true;
        self.change(|s| s.start_with_windows = !requested);
        self.syncing_startup = false;
        if requested && !sys::is_installed() {
            self.tray.balloon("FPS Viewer", "Start with Windows needs the installed version. Run FPS Viewer Setup first.");
        } else {
            self.tray.balloon("FPS Viewer", "Couldn't update the startup task.");
        }
    }

    fn on_startup_query(&mut self, enabled: bool) {
        self.syncing_startup = true;
        self.change(|s| s.start_with_windows = enabled);
        self.syncing_startup = false;
    }

    fn on_timer(&mut self, id: usize) {
        match id {
            TIMER_TICK => self.tick(),
            TIMER_SAVE => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_SAVE);
                }
                if self.save_pending {
                    self.save_pending = false;
                    settings::save(&self.settings);
                }
            }
            _ => {}
        }
    }
}

fn update_status(
    st: &mut Status,
    s: &Settings,
    error: Option<&str>,
    stats: &FrameStats,
    name: &str,
    target: f64,
    low_power: bool,
) {
    if let Some(e) = error {
        st.kind = StatusKind::Error;
        st.headline = "Can't read frame timings".into();
        st.detail = e.into();
    } else if !s.overlay_enabled {
        st.kind = StatusKind::Idle;
        st.headline = "Overlay is turned off".into();
        st.detail = "Switch on “Show overlay” or press Alt+Shift+F.".into();
    } else if stats.active {
        st.kind = StatusKind::Active;
        st.headline = format!("Measuring {name}");
        let api = if stats.from_kernel { "Vulkan / OpenGL" } else { "DirectX" };
        let rate = if low_power {
            "battery saver: 1 update/s".to_string()
        } else {
            format!("{} update{}/s", s.update_hz, if s.update_hz == 1 { "" } else { "s" })
        };
        st.detail = format!("{api} · target {target:.0} FPS · {rate}");
    } else {
        st.kind = StatusKind::Idle;
        st.headline = "Waiting for a game".into();
        st.detail = if s.fullscreen_only {
            "The overlay appears when a fullscreen game is focused.".into()
        } else {
            "Switch to any game or 3D app — the overlay appears automatically.".into()
        };
    }
}

fn quit() {
    unsafe { PostQuitMessage(0) };
}

/// A short-lived helper thread with a small stack (for blocking `schtasks` calls).
fn spawn_small(f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().stack_size(128 * 1024).spawn(f);
}

unsafe extern "system" fn on_foreground_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    with_app(|a| a.on_foreground(hwnd));
}

unsafe extern "system" fn main_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            with_app(|a| a.on_timer(wp.0));
            LRESULT(0)
        }
        WM_HOTKEY => {
            with_app(|a| a.on_hotkey(wp.0 as i32));
            LRESULT(0)
        }
        WM_APP_TRAY => {
            match (lp.0 & 0xFFFF) as u32 {
                WM_CONTEXTMENU => {
                    // Read state, then run the (modal) menu without holding the app borrowed.
                    let state = with_app(|a| (a.settings.overlay_enabled, a.overlay.move_mode(), a.settings.style));
                    if let Some((on, moving, style)) = state {
                        let cmd = tray::show_menu(hwnd, on, moving, style);
                        if cmd != 0 {
                            with_app(|a| a.on_tray_command(cmd));
                        }
                    }
                }
                NIN_SELECT | NIN_KEYSELECT => {
                    with_app(|a| a.show_settings());
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_OVERLAY_MOVED => {
            with_app(|a| a.on_overlay_moved());
            LRESULT(0)
        }
        WM_APP_STARTUP_RESULT => {
            with_app(|a| a.on_startup_result(wp.0 & 1 != 0, wp.0 & 2 != 0));
            LRESULT(0)
        }
        WM_APP_STARTUP_QUERY => {
            with_app(|a| a.on_startup_query(wp.0 != 0));
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            with_app(|a| a.on_power(wp.0 as u32));
            LRESULT(1)
        }
        WM_DISPLAYCHANGE => {
            with_app(|a| a.on_display_change());
            LRESULT(0)
        }
        WM_ENDSESSION if wp.0 != 0 => {
            // Signing out / shutting down: persist and release the ETW session now.
            with_app(|a| {
                if a.save_pending {
                    a.save_pending = false;
                    settings::save(&a.settings);
                }
            });
            etw::stop();
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            with_app(|a| a.tray.add());
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
