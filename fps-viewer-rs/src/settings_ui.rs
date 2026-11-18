//! The settings window, drawn with Direct2D to match the original WPF design: Mica backdrop,
//! glassy cards, animated toggles, segmented controls, accent swatches, sliders, keycaps and a
//! live preview "screen" that doubles as the position picker.
//!
//! It is an immediate-mode UI: every paint lays out and draws the whole window and records
//! hit rectangles; mouse input is resolved against the last paint's rectangles.

use std::time::Instant;

use windows::core::{w, Interface};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT, D2D_SIZE_U};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmFlush, DwmSetWindowAttribute, DWMWA_CLOAK, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, InvalidateRect, MonitorFromPoint, ScreenToClient, ValidateRect, MONITORINFO,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::app;
use crate::gfx::{
    argb, draw_glyph, pt, rgb, scale_translate, translate, Font, Gfx, Paint, Rect, Text, ACCENT_FROM, ACCENT_TO,
    NORMAL, SEMIBOLD,
};
use crate::overlay_draw::{self, OverlayModel};
use crate::settings::{OverlayPosition, OverlayStyle, Settings, THEMES};
use crate::util::{hinstance, register_class};

// ───────────────────────── design tokens ─────────────────────────

const WINDOW_BG: u32 = rgb(0x0E1015);
const CARD_BG: u32 = 0x0DFF_FFFF;
const CARD_BORDER: u32 = 0x14FF_FFFF;
const DIVIDER: u32 = 0x0FFF_FFFF;
const TEXT_PRIMARY: u32 = rgb(0xF2F4F8);
const TEXT_SECONDARY: u32 = rgb(0xA3ABBA);
const TEXT_TERTIARY: u32 = rgb(0x6E7687);
const CONTROL_BG: u32 = 0x12FF_FFFF;
const CONTROL_BG_HOVER: u32 = 0x1CFF_FFFF;
const CONTROL_BORDER: u32 = 0x1FFF_FFFF;
const WHITE: u32 = 0xFFFF_FFFF;

const ACCENT: [(f32, u32); 2] = [(0.0, ACCENT_FROM), (1.0, ACCENT_TO)];

const LEFT_COL: f32 = 410.0;
const MIN_W: f32 = 880.0;
const MIN_H: f32 = 560.0;
const ANIM_TIMER: usize = 1;
const TOGGLE_ANIM_SECS: f32 = 0.16;

// ───────────────────────── model ─────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tg {
    OverlayEnabled,
    ColorByPerformance,
    ShowGraph,
    ShowAppName,
    BatterySaver,
    AutoHide,
    FullscreenOnly,
    VulkanOpenGl,
    StartWithWindows,
    HideFromCapture,
}

const TOGGLE_COUNT: usize = 10;
const ALL_TOGGLES: [Tg; TOGGLE_COUNT] = [
    Tg::OverlayEnabled,
    Tg::ColorByPerformance,
    Tg::ShowGraph,
    Tg::ShowAppName,
    Tg::BatterySaver,
    Tg::AutoHide,
    Tg::FullscreenOnly,
    Tg::VulkanOpenGl,
    Tg::StartWithWindows,
    Tg::HideFromCapture,
];

impl Tg {
    pub fn get(self, s: &Settings) -> bool {
        match self {
            Tg::OverlayEnabled => s.overlay_enabled,
            Tg::ColorByPerformance => s.color_by_performance,
            Tg::ShowGraph => s.show_graph,
            Tg::ShowAppName => s.show_app_name,
            Tg::BatterySaver => s.battery_saver,
            Tg::AutoHide => s.auto_hide,
            Tg::FullscreenOnly => s.fullscreen_only,
            Tg::VulkanOpenGl => s.vulkan_opengl_support,
            Tg::StartWithWindows => s.start_with_windows,
            Tg::HideFromCapture => s.hide_from_capture,
        }
    }

    pub fn set(self, s: &mut Settings, v: bool) {
        match self {
            Tg::OverlayEnabled => s.overlay_enabled = v,
            Tg::ColorByPerformance => s.color_by_performance = v,
            Tg::ShowGraph => s.show_graph = v,
            Tg::ShowAppName => s.show_app_name = v,
            Tg::BatterySaver => s.battery_saver = v,
            Tg::AutoHide => s.auto_hide = v,
            Tg::FullscreenOnly => s.fullscreen_only = v,
            Tg::VulkanOpenGl => s.vulkan_opengl_support = v,
            Tg::StartWithWindows => s.start_with_windows = v,
            Tg::HideFromCapture => s.hide_from_capture = v,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slider {
    Scale,
    Opacity,
}

impl Slider {
    fn range(self) -> (f64, f64) {
        match self {
            Slider::Scale => (0.6, 2.0),
            Slider::Opacity => (0.3, 1.0),
        }
    }
    fn get(self, s: &Settings) -> f64 {
        match self {
            Slider::Scale => s.scale,
            Slider::Opacity => s.overlay_opacity,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SegKind {
    Style,
    Target,
    Hz,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    None,
    Toggle(Tg),
    Style(OverlayStyle),
    Target(u32),
    Hz(u32),
    Swatch(usize),
    Slider(Slider),
    Hotspot(OverlayPosition),
    Done,
    Quit,
    Reset,
    Thumb,
}

/// What the user asked for; applied by the app.
pub enum Action {
    Toggle(Tg, bool),
    Style(OverlayStyle),
    Target(u32),
    Hz(u32),
    Theme(usize),
    Slider(Slider, f64),
    Position(OverlayPosition),
    Done,
    Quit,
    Reset,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Active,
    Idle,
    Error,
}

pub struct Status {
    pub kind: StatusKind,
    pub headline: String,
    pub detail: String,
}

impl Default for Status {
    fn default() -> Self {
        Status { kind: StatusKind::Idle, headline: "Starting…".into(), detail: String::new() }
    }
}

enum Row {
    Toggle(&'static str, &'static str, Tg),
    Seg(&'static str, &'static str, SegKind),
    Swatches(&'static str, &'static str),
    Slider(&'static str, Slider),
    Keys(&'static str, &'static str),
    /// Text only — a term and its explanation, no control.
    Info(&'static str, &'static str),
}

const SECTIONS: [(&str, &[Row]); 6] = [
    (
        "APPEARANCE",
        &[
            Row::Toggle("Show overlay", "Display live frame rate on top of games.", Tg::OverlayEnabled),
            Row::Seg(
                "Style",
                "Minimal: FPS only. Compact: adds CPU and RAM. Detailed: adds the game's name and a graph.",
                SegKind::Style,
            ),
            Row::Swatches("Accent", "Colors the graph and highlights."),
            Row::Slider("Size", Slider::Scale),
            Row::Slider("Opacity", Slider::Opacity),
        ],
    ),
    (
        "READING THE OVERLAY",
        &[
            Row::Info("FPS", "Frames per second — how many new images the game shows each second. Higher is smoother."),
            Row::Info(
                "CPU · RAM",
                "How busy your processor is (all cores) and how much memory is in use out of the total installed — for the whole PC, not just the game.",
            ),
            Row::Info(
                "Graph",
                "How long each frame took over the last 4 seconds — flat is smooth. The dashed line is your target; red dots mark stutters.",
            ),
        ],
    ),
    (
        "DETAILS",
        &[
            Row::Seg(
                "Target frame rate",
                "Colors and the graph's guide line are relative to this. Auto uses your display's refresh rate.",
                SegKind::Target,
            ),
            Row::Toggle(
                "Color by performance",
                "Green at 90% of target or better, amber above 50%, red below.",
                Tg::ColorByPerformance,
            ),
            Row::Toggle(
                "Frame-time graph",
                "A 4-second sparkline that makes stutters easy to spot (Detailed style).",
                Tg::ShowGraph,
            ),
            Row::Toggle(
                "Show app name",
                "Label the overlay with the game being measured (Detailed style).",
                Tg::ShowAppName,
            ),
        ],
    ),
    (
        "PERFORMANCE & POWER",
        &[
            Row::Seg("Update rate", "How often the numbers refresh. Lower is lighter on battery.", SegKind::Hz),
            Row::Toggle("Battery saver", "On battery: refresh once a second and skip the graph.", Tg::BatterySaver),
            Row::Toggle(
                "Hide when idle",
                "Only appear while the focused app is actually rendering frames.",
                Tg::AutoHide,
            ),
            Row::Toggle(
                "Fullscreen games only",
                "Ignore browsers and windowed apps — show only for apps covering the whole screen.",
                Tg::FullscreenOnly,
            ),
            Row::Toggle(
                "Vulkan & OpenGL support",
                "Also listen to the kernel graphics driver. DirectX games don't need this.",
                Tg::VulkanOpenGl,
            ),
        ],
    ),
    (
        "SYSTEM",
        &[
            Row::Toggle("Start with Windows", "Launch quietly to the tray when you sign in.", Tg::StartWithWindows),
            Row::Toggle(
                "Hide from screenshots & recordings",
                "You still see the overlay; captures, streams and screenshots won't.",
                Tg::HideFromCapture,
            ),
        ],
    ),
    (
        "SHORTCUTS",
        &[
            Row::Keys("Show / hide overlay", "F"),
            Row::Keys("Unlock overlay to drag it", "M"),
            Row::Keys("Cycle style", "S"),
            Row::Keys("Open settings", "O"),
        ],
    ),
];

const STYLE_SEGS: [(&str, u32); 3] = [("Minimal", 0), ("Compact", 1), ("Detailed", 2)];
const TARGET_SEGS: [(&str, u32); 6] = [("Auto", 0), ("30", 30), ("60", 60), ("120", 120), ("144", 144), ("240", 240)];
const HZ_SEGS: [(&str, u32); 3] = [("1× / s", 1), ("2× / s", 2), ("4× / s", 4)];

#[derive(Clone, Copy)]
struct Knob {
    pos: f32,
    from: f32,
    start: Option<Instant>,
    last: bool,
}

/// Cache key for static text layouts: (string address, size, weight, wrap width, font).
type TextKey = (usize, u32, u16, u32, u8);

pub struct SettingsWindow {
    pub hwnd: HWND,
    rt: Option<ID2D1HwndRenderTarget>,
    paint: Option<Paint>,
    dpi: u32,
    mica: bool,
    size: (f32, f32),

    mouse: (f32, f32),
    tracking: bool,
    hover: Hit,
    pressed: Hit,
    hits: Vec<(Rect, Hit)>,
    slider_rects: [Rect; 2],

    scroll: f32,
    content_h: f32,
    thumb: Rect,
    thumb_drag: Option<(f32, f32)>,

    knobs: [Knob; TOGGLE_COUNT],
    knobs_ready: bool,
    animating: bool,

    texts: Vec<(TextKey, Text)>,
    mountains: Vec<ID2D1Geometry>,
}

impl SettingsWindow {
    pub fn new(g: &Gfx) -> Option<SettingsWindow> {
        unsafe {
            register_class(w!("FpsViewer.Settings"), Some(wndproc), true);

            // Center on the primary monitor's work area at 1000×700 DIPs.
            let mon = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let (mut dpi, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
            let k = dpi as f32 / 96.0;
            let work = mi.rcWork;
            let cw = ((1000.0 * k) as i32).min(work.right - work.left - 40);
            let ch = ((700.0 * k) as i32).min(work.bottom - work.top - 40);
            let mut r = RECT { left: 0, top: 0, right: cw, bottom: ch };
            let _ = AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE::default(), dpi);
            let (ww, wh) = (r.right - r.left, r.bottom - r.top);
            let x = work.left + (work.right - work.left - ww) / 2;
            let y = work.top + (work.bottom - work.top - wh) / 2;

            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("FpsViewer.Settings"),
                w!("FPS Viewer"),
                WS_OVERLAPPEDWINDOW,
                x,
                y,
                ww,
                wh,
                None,
                None,
                Some(hinstance()),
                None,
            )
            .ok()?;

            // Dark title bar; Mica (Windows 11 22H2+) with the backdrop extended under the content.
            let dark: i32 = 1;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const i32 as _, 4);
            let backdrop: i32 = 2; // DWMSBT_MAINWINDOW
            let mica =
                DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const i32 as _, 4).is_ok();
            if mica {
                let m = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
                let _ = DwmExtendFrameIntoClientArea(hwnd, &m);
            }

            let shapes: [&[(f32, f32)]; 3] = [
                &[(0., 128.), (36., 104.), (70., 120.), (118., 84.), (160., 116.), (206., 92.), (252., 122.), (290., 100.), (320., 112.), (320., 180.), (0., 180.)],
                &[(0., 148.), (48., 124.), (96., 144.), (146., 116.), (198., 148.), (246., 128.), (292., 146.), (320., 136.), (320., 180.), (0., 180.)],
                &[(0., 166.), (64., 150.), (132., 164.), (200., 148.), (262., 166.), (320., 156.), (320., 180.), (0., 180.)],
            ];
            let mountains = shapes
                .iter()
                .filter_map(|pts| {
                    let v: Vec<Vector2> = pts.iter().map(|&(x, y)| pt(x, y)).collect();
                    g.path(&v, true)
                })
                .collect();

            Some(SettingsWindow {
                hwnd,
                rt: None,
                paint: None,
                dpi: GetDpiForWindow(hwnd).max(96),
                mica,
                size: (1000.0, 700.0),
                mouse: (-1.0, -1.0),
                tracking: false,
                hover: Hit::None,
                pressed: Hit::None,
                hits: Vec::with_capacity(64),
                slider_rects: [Rect::default(); 2],
                scroll: 0.0,
                content_h: 0.0,
                thumb: Rect::default(),
                thumb_drag: None,
                knobs: [Knob { pos: 0.0, from: 0.0, start: None, last: false }; TOGGLE_COUNT],
                knobs_ready: false,
                animating: false,
                texts: Vec::new(),
                mountains,
            })
        }
    }

    /// Shows (or restores) the window. The first time, it's shown *cloaked* — DWM composes it
    /// but keeps it off screen — and painted directly, then uncloaked once the first frame is
    /// on the glass. Without this, Windows displays the blank (white) client area for the
    /// moment it takes Direct2D to create its device and draw, which is long on a cold start.
    pub fn show(&mut self, g: &Gfx, s: &Settings, preview: &OverlayModel, status: &Status) {
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            } else if !IsWindowVisible(self.hwnd).as_bool() {
                let hwnd = self.hwnd;
                let cloak = move |on: i32| {
                    let _ = DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, &on as *const i32 as _, 4);
                };
                cloak(1);
                let _ = ShowWindow(self.hwnd, SW_SHOWNORMAL);
                // Painted here rather than via WM_PAINT: the caller holds the app state, so a
                // re-entrant WM_PAINT couldn't reach this window.
                self.paint(g, s, preview, status);
                let _ = ValidateRect(Some(self.hwnd), None);
                let _ = DwmFlush();
                cloak(0);
            } else {
                let _ = ShowWindow(self.hwnd, SW_SHOWNORMAL);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// Handles a window message. Returns the result (None = default handling) and any action.
    #[allow(clippy::too_many_arguments)]
    pub fn handle(
        &mut self,
        g: &Gfx,
        s: &Settings,
        preview: &OverlayModel,
        status: &Status,
        msg: u32,
        wp: WPARAM,
        lp: LPARAM,
    ) -> (Option<LRESULT>, Option<Action>) {
        let k = 96.0 / self.dpi as f32;
        let mouse_pos = |lp: LPARAM| ((lp.0 as i16) as f32 * k, ((lp.0 >> 16) as i16) as f32 * k);
        match msg {
            WM_PAINT => {
                self.paint(g, s, preview, status);
                unsafe {
                    let _ = ValidateRect(Some(self.hwnd), None);
                }
                (Some(LRESULT(0)), None)
            }
            WM_ERASEBKGND => (Some(LRESULT(1)), None),
            WM_SIZE => {
                let (w, h) = (lp.0 as u32 & 0xFFFF, (lp.0 as u32 >> 16) & 0xFFFF);
                if let Some(rt) = &self.rt {
                    unsafe {
                        let _ = rt.Resize(&D2D_SIZE_U { width: w, height: h });
                    }
                }
                self.invalidate();
                (Some(LRESULT(0)), None)
            }
            WM_DPICHANGED => {
                self.dpi = (((wp.0 >> 16) & 0xFFFF) as u32).max(96);
                if let Some(rt) = &self.rt {
                    unsafe { rt.SetDpi(self.dpi as f32, self.dpi as f32) };
                }
                // SAFETY: lParam points at the suggested window rectangle.
                let r = unsafe { *(lp.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(
                        self.hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                (Some(LRESULT(0)), None)
            }
            WM_GETMINMAXINFO => {
                // SAFETY: lParam points at a MINMAXINFO for this message.
                let info = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
                let scale = self.dpi as f32 / 96.0;
                let mut r = RECT { left: 0, top: 0, right: (MIN_W * scale) as i32, bottom: (MIN_H * scale) as i32 };
                unsafe {
                    let _ =
                        AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE::default(), self.dpi);
                }
                info.ptMinTrackSize = POINT { x: r.right - r.left, y: r.bottom - r.top };
                (Some(LRESULT(0)), None)
            }
            WM_MOUSEMOVE => {
                let (x, y) = mouse_pos(lp);
                self.mouse = (x, y);
                if !self.tracking {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe {
                        let _ = TrackMouseEvent(&mut tme);
                    }
                    self.tracking = true;
                }
                if let Some((start_y, start_scroll)) = self.thumb_drag {
                    let h = self.size.1;
                    let track = h - self.thumb.h;
                    if track > 0.0 {
                        let max = (self.content_h - h).max(0.0);
                        self.set_scroll(start_scroll + (y - start_y) / track * max);
                    }
                    return (Some(LRESULT(0)), None);
                }
                if let Hit::Slider(sl) = self.pressed {
                    return (Some(LRESULT(0)), Some(Action::Slider(sl, self.slider_value(sl, x))));
                }
                let hit = self.hit_test(x, y);
                if hit != self.hover {
                    self.hover = hit;
                    self.invalidate();
                }
                (Some(LRESULT(0)), None)
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.mouse = (-1.0, -1.0);
                if self.hover != Hit::None {
                    self.hover = Hit::None;
                    self.invalidate();
                }
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = mouse_pos(lp);
                let hit = self.hit_test(x, y);
                self.pressed = hit;
                unsafe { SetCapture(self.hwnd) };
                self.invalidate();
                match hit {
                    Hit::Thumb => {
                        self.thumb_drag = Some((y, self.scroll));
                        (Some(LRESULT(0)), None)
                    }
                    Hit::Slider(sl) => (Some(LRESULT(0)), Some(Action::Slider(sl, self.slider_value(sl, x)))),
                    _ => (Some(LRESULT(0)), None),
                }
            }
            WM_LBUTTONUP => {
                let (x, y) = mouse_pos(lp);
                let pressed = std::mem::replace(&mut self.pressed, Hit::None);
                unsafe {
                    let _ = ReleaseCapture();
                }
                self.thumb_drag = None;
                self.hover = self.hit_test(x, y);
                self.invalidate();
                if pressed != self.hover {
                    return (Some(LRESULT(0)), None);
                }
                let action = match pressed {
                    Hit::Toggle(t) => Some(Action::Toggle(t, !t.get(s))),
                    Hit::Style(v) => Some(Action::Style(v)),
                    Hit::Target(v) => Some(Action::Target(v)),
                    Hit::Hz(v) => Some(Action::Hz(v)),
                    Hit::Swatch(i) => Some(Action::Theme(i)),
                    Hit::Hotspot(p) => Some(Action::Position(p)),
                    Hit::Done => Some(Action::Done),
                    Hit::Quit => Some(Action::Quit),
                    Hit::Reset => Some(Action::Reset),
                    _ => None,
                };
                (Some(LRESULT(0)), action)
            }
            WM_CAPTURECHANGED => {
                self.pressed = Hit::None;
                self.thumb_drag = None;
                (Some(LRESULT(0)), None)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) as i16) as f32;
                let mut p = POINT { x: (lp.0 as i16) as i32, y: ((lp.0 >> 16) as i16) as i32 };
                unsafe {
                    let _ = ScreenToClient(self.hwnd, &mut p);
                }
                if p.x as f32 * k >= LEFT_COL {
                    self.set_scroll(self.scroll - delta / 120.0 * 48.0);
                }
                (Some(LRESULT(0)), None)
            }
            WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
                let hand = self.hover != Hit::None;
                unsafe {
                    SetCursor(LoadCursorW(None, if hand { IDC_HAND } else { IDC_ARROW }).ok());
                }
                (Some(LRESULT(1)), None)
            }
            WM_TIMER if wp.0 == ANIM_TIMER => {
                self.invalidate();
                (Some(LRESULT(0)), None)
            }
            WM_KEYDOWN if wp.0 as u16 == VK_ESCAPE.0 || wp.0 as u16 == VK_RETURN.0 => {
                (Some(LRESULT(0)), Some(Action::Done))
            }
            WM_CLOSE => (Some(LRESULT(0)), Some(Action::Done)),
            _ => (None, None),
        }
    }

    fn set_scroll(&mut self, v: f32) {
        let max = (self.content_h - self.size.1).max(0.0);
        let v = v.clamp(0.0, max);
        if v != self.scroll {
            self.scroll = v;
            self.invalidate();
        }
    }

    fn hit_test(&self, x: f32, y: f32) -> Hit {
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map_or(Hit::None, |(_, h)| *h)
    }

    fn slider_value(&self, sl: Slider, x: f32) -> f64 {
        let r = self.slider_rects[sl as usize];
        let t = ((x - (r.x + 10.0)) / (r.w - 20.0).max(1.0)).clamp(0.0, 1.0) as f64;
        let (lo, hi) = sl.range();
        lo + t * (hi - lo)
    }

    /// A cached layout for a static string (labels never change, so lay them out once).
    fn st(&mut self, g: &Gfx, s: &'static str, font: Font, size: f32, weight: u16, wrap: Option<f32>) -> Text {
        let key: TextKey = (s.as_ptr() as usize, size.to_bits(), weight, wrap.map_or(0, |w| w.to_bits()), font as u8);
        if let Some((_, t)) = self.texts.iter().find(|(k, _)| *k == key) {
            return t.clone();
        }
        let t = match wrap {
            Some(w) => g.text_wrapped(s, font, size, weight, w),
            None => g.text(s, font, size, weight),
        };
        if self.texts.len() > 400 {
            self.texts.clear();
        }
        self.texts.push((key, t.clone()));
        t
    }

    fn ensure_target(&mut self, g: &Gfx) -> bool {
        if self.rt.is_some() {
            return true;
        }
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rc);
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: self.dpi as f32,
                dpiY: self.dpi as f32,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.hwnd,
                pixelSize: D2D_SIZE_U { width: (rc.right - rc.left) as u32, height: (rc.bottom - rc.top) as u32 },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let Ok(rt) = g.d2d.CreateHwndRenderTarget(&props, &hprops) else { return false };
            let Ok(paint) = rt.cast().and_then(Paint::new) else { return false };
            self.rt = Some(rt);
            self.paint = Some(paint);
            self.texts.clear(); // cached layouts may carry the old target's effect brushes
            true
        }
    }

    // ───────────────────────── painting ─────────────────────────

    fn paint(&mut self, g: &Gfx, s: &Settings, preview: &OverlayModel, status: &Status) {
        if !self.ensure_target(g) {
            return;
        }
        let Some(rt) = self.rt.clone() else { return };
        let Some(mut p) = self.paint.take() else { return };

        let size = unsafe { rt.GetSize() };
        self.size = (size.width, size.height);
        self.hits.clear();
        self.update_knobs(s);

        unsafe { rt.BeginDraw() };
        p.set_transform(&Matrix3x2::identity());
        p.clear(if self.mica { 0 } else { WINDOW_BG });

        self.paint_left(g, &mut p, s, preview, status);
        self.paint_right(g, &mut p, s);

        let result = unsafe { rt.EndDraw(None, None) };
        self.paint = Some(p);
        if result.is_err() {
            // Device lost (driver update, GPU reset…): rebuild on the next paint.
            self.rt = None;
            self.paint = None;
            self.texts.clear();
            self.invalidate();
        }

        let animating = self.knobs.iter().any(|k| k.start.is_some());
        if animating != self.animating {
            self.animating = animating;
            unsafe {
                if animating {
                    SetTimer(Some(self.hwnd), ANIM_TIMER, 15, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), ANIM_TIMER);
                }
            }
        }
    }

    fn update_knobs(&mut self, s: &Settings) {
        let now = Instant::now();
        for (i, t) in ALL_TOGGLES.iter().enumerate() {
            let on = t.get(s);
            let k = &mut self.knobs[i];
            if !self.knobs_ready {
                *k = Knob { pos: on as u8 as f32, from: 0.0, start: None, last: on };
                continue;
            }
            if on != k.last {
                k.last = on;
                k.from = k.pos;
                k.start = Some(now);
            }
            if let Some(start) = k.start {
                let t = (now.duration_since(start).as_secs_f32() / TOGGLE_ANIM_SECS).min(1.0);
                let eased = 1.0 - (1.0 - t).powi(3); // cubic ease-out
                let to = on as u8 as f32;
                k.pos = k.from + (to - k.from) * eased;
                if t >= 1.0 {
                    k.pos = to;
                    k.start = None;
                }
            }
        }
        self.knobs_ready = true;
    }

    fn paint_left(&mut self, g: &Gfx, p: &mut Paint, s: &Settings, preview: &OverlayModel, status: &Status) {
        let h = self.size.1;
        let x0 = 28.0;
        let cw = LEFT_COL - 28.0 - 14.0;

        // Hero
        draw_glyph(p, x0, 14.0, 44.0);
        let title = self.st(g, "FPS Viewer", Font::Display, 24.0, SEMIBOLD, None);
        let sub = self.st(g, "A featherweight frame-rate overlay", Font::Ui, 12.5, NORMAL, None);
        let ty = 14.0 + (44.0 - (title.h + 1.0 + sub.h)) / 2.0;
        p.text(&title, x0 + 58.0, ty, TEXT_PRIMARY);
        p.text(&sub, x0 + 58.0, ty + title.h + 1.0, TEXT_SECONDARY);

        // Preview "screen" — also the position picker.
        let screen = Rect::new(x0, 80.0, cw, 206.0);
        self.draw_scene(g, p, screen);
        for pos in OverlayPosition::ANCHORS {
            let (cx, cy) = match pos {
                OverlayPosition::TopLeft => (screen.x + 19.0, screen.y + 19.0),
                OverlayPosition::TopCenter => (screen.cx(), screen.y + 19.0),
                OverlayPosition::TopRight => (screen.right() - 19.0, screen.y + 19.0),
                OverlayPosition::BottomLeft => (screen.x + 19.0, screen.bottom() - 19.0),
                OverlayPosition::BottomCenter => (screen.cx(), screen.bottom() - 19.0),
                _ => (screen.right() - 19.0, screen.bottom() - 19.0),
            };
            self.hits.push((Rect::new(cx - 15.0, cy - 15.0, 30.0, 30.0), Hit::Hotspot(pos)));
            if s.position != pos {
                let hover = self.hover == Hit::Hotspot(pos);
                let r = if hover { 7.0 } else { 5.0 };
                p.fill_circle(cx, cy, r, if hover { 0x80FF_FFFF } else { 0x33FF_FFFF });
                p.stroke_circle(cx, cy, r, 0x99FF_FFFF, 1.0);
            }
        }
        self.draw_preview_overlay(g, p, s, preview, screen);

        // Hint ("Alt+Shift+M" emphasised)
        let hint = self.st(
            g,
            "Click a dot to place the overlay, or press Alt+Shift+M in-game to drag it anywhere.",
            Font::Ui,
            11.5,
            NORMAL,
            Some(cw - 2.0),
        );
        const KEY_AT: u32 = 43; // "Alt+Shift+M"
        hint.style_range(KEY_AT, 11, None, Some(SEMIBOLD));
        let secondary = p.effect(TEXT_SECONDARY);
        hint.color_range(KEY_AT, 11, &secondary);
        let hint_y = screen.bottom() + 10.0;
        p.text(&hint, x0 + 2.0, hint_y, TEXT_TERTIARY);

        // Live status card
        let card_y = hint_y + hint.h + 18.0;
        let text_w = cw - 32.0 - 22.0;
        let headline = g.text_trimmed(&status.headline, Font::Ui, 13.0, SEMIBOLD, text_w);
        let detail = g.text_wrapped(&status.detail, Font::Ui, 12.0, NORMAL, text_w);
        let card = Rect::new(x0, card_y, cw, 12.0 + headline.h + 2.0 + detail.h + 12.0);
        p.fill_round(card, 12.0, CARD_BG);
        p.stroke_round(card, 12.0, CARD_BORDER, 1.0);
        let kind = match status.kind {
            StatusKind::Active => rgb(0x34D399),
            StatusKind::Idle => rgb(0x8A93A6),
            StatusKind::Error => rgb(0xFB7185),
        };
        let (dx, dy) = (x0 + 21.0, card_y + 22.0);
        p.fill_circle(dx, dy, 5.0, argb(0x40, kind));
        p.fill_circle(dx, dy, 3.0, kind);
        p.text(&headline, x0 + 38.0, card_y + 12.0, TEXT_PRIMARY);
        p.text(&detail, x0 + 38.0, card_y + 12.0 + headline.h + 2.0, TEXT_SECONDARY);

        // Buttons, pinned to the bottom.
        let quit_t = self.st(g, "Quit FPS Viewer", Font::Ui, 12.5, NORMAL, None);
        let done_t = self.st(g, "Done", Font::Ui, 12.5, SEMIBOLD, None);
        let bh = quit_t.h.max(done_t.h) + 16.0;
        let by = (h - 24.0 - bh).max(card.bottom() + 16.0);
        self.button(p, Rect::new(x0, by, quit_t.w + 30.0, bh), &quit_t, Hit::Quit, false);
        let done_w = done_t.w + 44.0;
        self.button(p, Rect::new(x0 + cw - done_w, by, done_w, bh), &done_t, Hit::Done, true);
    }

    /// The stylised dusk landscape used as the preview "game" (pure vectors, no images).
    fn draw_scene(&mut self, g: &Gfx, p: &mut Paint, r: Rect) {
        let mask = g.rounded_rect_geometry(r, 14.0);
        p.push_layer(mask.as_ref(), 1.0);
        let k = (r.w / 320.0).max(r.h / 180.0); // UniformToFill
        let (ox, oy) = (r.x + (r.w - 320.0 * k) / 2.0, r.y + (r.h - 180.0 * k) / 2.0);
        p.set_transform(&scale_translate(k, ox, oy));

        let sky = p.linear(
            &[(0.0, rgb(0x140B2E)), (0.45, rgb(0x3B1856)), (0.78, rgb(0xB2456E)), (1.0, rgb(0xF08A5D))],
            0.0,
            0.0,
            0.0,
            180.0,
        );
        p.fill_round_with(Rect::new(0.0, 0.0, 320.0, 180.0), 0.0, &sky);
        for (x, y, rad) in [
            (34., 22., 0.8),
            (92., 40., 0.6),
            (150., 16., 0.9),
            (198., 34., 0.6),
            (262., 20., 0.8),
            (300., 48., 0.6),
            (120., 58., 0.5),
        ] {
            p.fill_circle(x, y, rad, 0x59FF_FFFF);
        }
        let sun = p.radial(&[(0.0, rgb(0xFFF1D0)), (0.55, rgb(0xFFC58A)), (1.0, 0x00FF_9E6B)], 224.0, 122.0, 34.0, 34.0);
        p.fill_circle_with(224.0, 122.0, 34.0, &sun);
        for (geo, c) in self.mountains.iter().zip([rgb(0x4A1E5E), rgb(0x2A1240), rgb(0x150A22)]) {
            p.fill_geometry(geo, c);
        }

        p.set_transform(&Matrix3x2::identity());
        p.pop();
        p.stroke_round(r, 14.0, 0x26FF_FFFF, 1.0);
    }

    fn draw_preview_overlay(&mut self, g: &Gfx, p: &mut Paint, s: &Settings, m: &OverlayModel, screen: Rect) {
        let k = 0.6 * m.scale;
        let (w, h) = overlay_draw::render(g, None, m);
        let (ow, oh) = (w * k, h * k);
        let margin = 10.0;
        let (x, y) = match s.position {
            OverlayPosition::TopLeft => (margin, margin),
            OverlayPosition::TopCenter => ((screen.w - ow) / 2.0, margin),
            OverlayPosition::TopRight => (screen.w - ow - margin, margin),
            OverlayPosition::BottomLeft => (margin, screen.h - oh - margin),
            OverlayPosition::BottomCenter => ((screen.w - ow) / 2.0, screen.h - oh - margin),
            OverlayPosition::BottomRight => (screen.w - ow - margin, screen.h - oh - margin),
            OverlayPosition::Custom => {
                (s.custom_x as f32 * (screen.w - ow).max(0.0), s.custom_y as f32 * (screen.h - oh).max(0.0))
            }
        };
        p.push_layer(None, m.opacity);
        p.set_transform(&scale_translate(k, screen.x + x.max(0.0), screen.y + y.max(0.0)));
        overlay_draw::render(g, Some(p), m);
        p.set_transform(&Matrix3x2::identity());
        p.pop();
    }

    fn button(&mut self, p: &mut Paint, r: Rect, t: &Text, hit: Hit, accent: bool) {
        self.hits.push((r, hit));
        let hover = self.hover == hit;
        let pressed = hover && self.pressed == hit;
        let opacity = if pressed {
            0.75
        } else if hover && accent {
            0.9
        } else {
            1.0
        };
        if opacity < 1.0 {
            p.push_layer(None, opacity);
        }
        if accent {
            let b = p.linear(&ACCENT, r.x, r.y, r.right(), r.bottom());
            p.fill_round_with(r, 8.0, &b);
        } else {
            p.fill_round(r, 8.0, if hover { CONTROL_BG_HOVER } else { CONTROL_BG });
            p.stroke_round(r, 8.0, CONTROL_BORDER, 1.0);
        }
        p.text(t, r.x + (r.w - t.w) / 2.0, r.y + (r.h - t.h) / 2.0, if accent { WHITE } else { TEXT_PRIMARY });
        if opacity < 1.0 {
            p.pop();
        }
    }

    fn paint_right(&mut self, g: &Gfx, p: &mut Paint, s: &Settings) {
        let (w, h) = self.size;
        let view = Rect::new(LEFT_COL, 0.0, w - LEFT_COL, h);
        let has_bar = self.content_h > h + 0.5;
        let x0 = LEFT_COL + 14.0;
        let cw = view.w - 14.0 - 28.0 - if has_bar { 10.0 } else { 0.0 };
        self.scroll = self.scroll.clamp(0.0, (self.content_h - h).max(0.0));
        let sy = -self.scroll;

        p.push_clip(view);
        p.set_transform(&translate(0.0, sy));
        let hits_start = self.hits.len();

        let mut y = 14.0;
        for (i, (label, rows)) in SECTIONS.iter().enumerate() {
            let lt = self.st(g, label, Font::Ui, 11.0, SEMIBOLD, None);
            if i > 0 {
                y += 8.0;
            }
            p.text(&lt, x0 + 2.0, y, TEXT_TERTIARY);
            y += lt.h + 10.0;
            y = self.card(g, p, s, rows, x0, y, cw) + 14.0;
        }

        // Footer
        let reset_t = self.st(g, "Reset appearance", Font::Ui, 12.5, NORMAL, None);
        let saved = self.st(g, "Changes are saved automatically.", Font::Ui, 12.0, NORMAL, None);
        let bh = reset_t.h + 16.0;
        y += 4.0;
        let reset = Rect::new(x0 + cw - reset_t.w - 30.0, y, reset_t.w + 30.0, bh);
        self.button(p, reset, &reset_t, Hit::Reset, false);
        p.text(&saved, x0 + 2.0, y + (bh - saved.h) / 2.0, TEXT_TERTIARY);
        y += bh + 24.0;
        let bar_changed = (y > h + 0.5) != has_bar;
        self.content_h = y;

        // Hit rectangles were recorded in content coordinates: shift into window space and
        // drop the ones scrolled out of view.
        let mut i = hits_start;
        while i < self.hits.len() {
            let r = self.hits[i].0.offset(0.0, sy);
            if r.bottom() <= 0.0 || r.y >= h {
                self.hits.swap_remove(i);
            } else {
                self.hits[i].0 = r;
                i += 1;
            }
        }

        p.set_transform(&Matrix3x2::identity());
        p.pop_clip();

        // Slim scrollbar
        if self.content_h > h + 0.5 {
            let th = (h * h / self.content_h).max(24.0);
            let max = self.content_h - h;
            let ty = if max > 0.0 { self.scroll / max * (h - th) } else { 0.0 };
            self.thumb = Rect::new(w - 10.0, ty, 10.0, th);
            self.hits.push((self.thumb, Hit::Thumb));
            let active = self.hover == Hit::Thumb || self.thumb_drag.is_some();
            p.fill_round(Rect::new(w - 8.0, ty, 6.0, th), 3.0, if active { 0x4DFF_FFFF } else { 0x26FF_FFFF });
        }
        if bar_changed {
            self.invalidate(); // the scrollbar appearing/disappearing changes the content width
        }
    }

    /// Draws one settings card; returns its bottom edge.
    #[allow(clippy::too_many_arguments)]
    fn card(&mut self, g: &Gfx, p: &mut Paint, s: &Settings, rows: &[Row], x0: f32, y0: f32, cw: f32) -> f32 {
        let inner_x = x0 + 18.0;
        let inner_w = cw - 36.0;

        // Measure first (the card background goes under the rows).
        struct Laid {
            title: Text,
            desc: Option<Text>,
            ctrl: (f32, f32),
            h: f32,
        }
        let mut laid = Vec::with_capacity(rows.len());
        for row in rows {
            let (title, desc_s, ctrl) = match row {
                Row::Toggle(t, d, _) => (*t, Some(*d), (42.0, 22.0)),
                Row::Seg(t, d, kind) => (*t, Some(*d), self.seg_size(g, *kind)),
                Row::Swatches(t, d) => (*t, Some(*d), (5.0 * 34.0 + 4.0 * 10.0, 34.0)),
                Row::Slider(t, _) => (*t, None, (220.0 + 48.0, 24.0)),
                Row::Keys(t, key) => (*t, None, self.keys_size(g, key)),
                Row::Info(t, d) => (*t, Some(*d), (0.0, 0.0)),
            };
            let title_t = self.st(g, title, Font::Ui, 13.5, NORMAL, None);
            let text_w = (inner_w - ctrl.0 - 16.0).max(40.0);
            let desc = desc_s.map(|d| self.st(g, d, Font::Ui, 12.0, NORMAL, Some(text_w)));
            let text_h = title_t.h + desc.as_ref().map_or(0.0, |d| 2.0 + d.h);
            laid.push(Laid { title: title_t, desc, ctrl, h: text_h.max(ctrl.1) });
        }
        let rows_h: f32 = laid.iter().map(|l| l.h).sum::<f32>() + 21.0 * laid.len().saturating_sub(1) as f32;
        let card = Rect::new(x0, y0, cw, 14.0 + rows_h + 14.0);
        p.fill_round(card, 12.0, CARD_BG);
        p.stroke_round(card, 12.0, CARD_BORDER, 1.0);

        let mut y = y0 + 14.0;
        for (i, (row, l)) in rows.iter().zip(&laid).enumerate() {
            if i > 0 {
                p.fill_rect(Rect::new(inner_x, y + 10.0, inner_w, 1.0), DIVIDER);
                y += 21.0;
            }
            let text_h = l.title.h + l.desc.as_ref().map_or(0.0, |d| 2.0 + d.h);
            let ty = y + (l.h - text_h) / 2.0;
            p.text(&l.title, inner_x, ty, TEXT_PRIMARY);
            if let Some(d) = &l.desc {
                p.text(d, inner_x, ty + l.title.h + 2.0, TEXT_SECONDARY);
            }
            let ctrl = Rect::new(inner_x + inner_w - l.ctrl.0, y + (l.h - l.ctrl.1) / 2.0, l.ctrl.0, l.ctrl.1);
            match row {
                Row::Toggle(_, _, t) => self.toggle(p, ctrl, *t),
                Row::Seg(_, _, kind) => self.segments(g, p, s, ctrl, *kind),
                Row::Swatches(..) => self.swatches(p, s, ctrl),
                Row::Slider(_, sl) => self.slider(g, p, s, ctrl, *sl),
                Row::Keys(_, key) => self.keys(g, p, ctrl, key),
                Row::Info(..) => {}
            }
            y += l.h;
        }
        card.bottom()
    }

    fn toggle(&mut self, p: &mut Paint, r: Rect, t: Tg) {
        self.hits.push((r, Hit::Toggle(t)));
        let Knob { pos, last: on, .. } = self.knobs[t as usize];
        let hover = self.hover == Hit::Toggle(t);
        if on {
            let b = p.linear(&ACCENT, r.x, r.y, r.right(), r.bottom());
            p.fill_round_with(r, 11.0, &b);
        } else {
            p.fill_round(r, 11.0, if hover { 0x22FF_FFFF } else { 0x14FF_FFFF });
            p.stroke_round(r, 11.0, 0x4DFF_FFFF, 1.0);
        }
        let d = if on { 14.0 } else { 12.0 };
        let left = r.x + 1.0 + 4.0 + pos * 19.0;
        p.fill_circle(left + d / 2.0, r.cy(), d / 2.0, if on { WHITE } else { rgb(0xC9CED8) });
    }

    /// (options, min width, horizontal padding)
    fn seg_options(kind: SegKind) -> (&'static [(&'static str, u32)], f32, f32) {
        match kind {
            SegKind::Style => (&STYLE_SEGS, 64.0, 14.0),
            SegKind::Target => (&TARGET_SEGS, 40.0, 10.0),
            SegKind::Hz => (&HZ_SEGS, 64.0, 14.0),
        }
    }

    fn seg_widths(&mut self, g: &Gfx, kind: SegKind) -> ([f32; 6], usize, f32) {
        let (opts, min_w, pad) = Self::seg_options(kind);
        let mut widths = [0.0f32; 6];
        let mut th = 0.0f32;
        for (i, (label, _)) in opts.iter().enumerate() {
            let t = self.st(g, label, Font::Ui, 12.5, SEMIBOLD, None);
            let min = if kind == SegKind::Target && i == 0 { 50.0 } else { min_w };
            widths[i] = (t.w + 2.0 * pad).max(min).ceil();
            th = th.max(t.h);
        }
        (widths, opts.len(), th + 12.0)
    }

    fn seg_size(&mut self, g: &Gfx, kind: SegKind) -> (f32, f32) {
        let (widths, n, h) = self.seg_widths(g, kind);
        (widths[..n].iter().sum::<f32>() + 8.0, h + 8.0)
    }

    fn segments(&mut self, g: &Gfx, p: &mut Paint, s: &Settings, host: Rect, kind: SegKind) {
        p.fill_round(host, 9.0, 0x0AFF_FFFF);
        p.stroke_round(host, 9.0, CARD_BORDER, 1.0);
        let (widths, n, h) = self.seg_widths(g, kind);
        let (opts, ..) = Self::seg_options(kind);
        let mut x = host.x + 4.0;
        for i in 0..n {
            let (label, value) = opts[i];
            let (hit, checked) = match kind {
                SegKind::Style => (Hit::Style(OverlayStyle::ALL[i]), s.style == OverlayStyle::ALL[i]),
                SegKind::Target => (Hit::Target(value), s.target_fps == value),
                SegKind::Hz => (Hit::Hz(value), s.update_hz == value),
            };
            let r = Rect::new(x, host.y + 4.0, widths[i], h);
            self.hits.push((r, hit));
            let hover = self.hover == hit;
            if checked {
                p.fill_round(r, 7.0, 0x26FF_FFFF);
            } else if hover {
                p.fill_round(r, 7.0, 0x0FFF_FFFF);
            }
            let t = self.st(g, label, Font::Ui, 12.5, if checked { SEMIBOLD } else { NORMAL }, None);
            let c = if checked {
                WHITE
            } else if hover {
                TEXT_PRIMARY
            } else {
                TEXT_SECONDARY
            };
            p.text(&t, r.x + (r.w - t.w) / 2.0, r.y + (r.h - t.h) / 2.0, c);
            x += widths[i];
        }
    }

    fn swatches(&mut self, p: &mut Paint, s: &Settings, r: Rect) {
        for (i, theme) in THEMES.iter().enumerate() {
            let cell = Rect::new(r.x + i as f32 * 44.0, r.y, 34.0, 34.0);
            self.hits.push((cell, Hit::Swatch(i)));
            let checked = s.theme == i;
            let hover = self.hover == Hit::Swatch(i);
            let ring = if checked {
                0xFF
            } else if hover {
                0x59
            } else {
                0
            };
            if ring > 0 {
                p.stroke_circle(cell.cx(), cell.cy(), 17.0, argb(ring, 0xFFFFFF), 2.0);
            }
            let rad = if checked { 11.0 } else { 12.0 };
            let b = p.linear(
                &[(0.0, rgb(theme.from)), (1.0, rgb(theme.swatch_to))],
                cell.cx() - rad,
                cell.cy() - rad,
                cell.cx() + rad,
                cell.cy() + rad,
            );
            p.fill_circle_with(cell.cx(), cell.cy(), rad, &b);
        }
    }

    fn slider(&mut self, g: &Gfx, p: &mut Paint, s: &Settings, r: Rect, sl: Slider) {
        let track = Rect::new(r.x, r.y, 220.0, 24.0);
        self.slider_rects[sl as usize] = track.offset(0.0, -self.scroll);
        self.hits.push((track, Hit::Slider(sl)));
        let (lo, hi) = sl.range();
        let v = sl.get(s);
        let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
        let tx = track.x + 10.0 + t * (track.w - 20.0);
        let cy = track.cy();

        p.fill_round(Rect::new(track.x, cy - 2.0, track.w, 4.0), 2.0, 0x24FF_FFFF);
        if tx > track.x + 0.5 {
            let fill = Rect::new(track.x, cy - 2.0, tx - track.x, 4.0);
            let b = p.linear(&ACCENT, fill.x, fill.y, fill.right(), fill.bottom());
            p.fill_round_with(fill, 2.0, &b);
        }
        let hover = self.hover == Hit::Slider(sl);
        let dragging = self.pressed == Hit::Slider(sl);
        p.fill_circle(tx, cy, 10.0, rgb(0x2B2F3A));
        p.stroke_circle(tx, cy, 10.0, 0x40FF_FFFF, 1.0);
        let inner = if dragging {
            4.0
        } else if hover {
            6.0
        } else {
            5.0
        };
        p.fill_circle(tx, cy, inner, WHITE);

        let label = g.text(&format!("{:.0}%", (v * 100.0).round()), Font::Ui, 12.0, NORMAL);
        p.text(&label, r.right() - label.w, cy - label.h / 2.0, TEXT_SECONDARY);
    }

    fn keycap(&mut self, g: &Gfx, key: &'static str) -> (Text, f32, f32) {
        let t = self.st(g, key, Font::Ui, 11.0, SEMIBOLD, None);
        let (w, h) = ((t.w + 14.0).max(24.0), t.h + 4.0 + 3.0);
        (t, w, h)
    }

    fn keys_size(&mut self, g: &Gfx, key: &'static str) -> (f32, f32) {
        let mut w = 0.0;
        let mut h = 0.0f32;
        for k in ["Alt", "Shift", key] {
            let (_, kw, kh) = self.keycap(g, k);
            w += kw + 4.0;
            h = h.max(kh);
        }
        (w, h)
    }

    fn keys(&mut self, g: &Gfx, p: &mut Paint, r: Rect, key: &'static str) {
        let mut x = r.x;
        for k in ["Alt", "Shift", key] {
            let (t, w, h) = self.keycap(g, k);
            x += 4.0;
            let cap = Rect::new(x, r.y + (r.h - h) / 2.0, w, h);
            p.fill_round(cap, 5.0, 0x0FFF_FFFF);
            p.stroke_round(cap, 5.0, 0x2EFF_FFFF, 1.0);
            // Thicker bottom edge, like a physical key.
            p.fill_rect(Rect::new(cap.x + 4.0, cap.bottom() - 2.0, cap.w - 8.0, 1.0), 0x2EFF_FFFF);
            p.text(&t, cap.x + (cap.w - t.w) / 2.0, cap.y + 3.0, TEXT_PRIMARY);
            x += w;
        }
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), ANIM_TIMER);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(r) = app::settings_message(hwnd, msg, wp, lp) {
        return r;
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}
