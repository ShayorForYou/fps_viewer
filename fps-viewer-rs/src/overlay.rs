//! The on-screen overlay: a per-pixel-alpha layered window that is click-through, never
//! activates and sits topmost on the game's monitor. It is drawn by Direct2D's *software*
//! rasterizer into a small DIB, so it never competes with the game for the GPU. In move mode
//! it accepts the mouse and drags like a title bar.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{w, Interface};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app;
use crate::gfx::{scale_translate, Gfx, Paint};
use crate::overlay_draw::{self, OverlayModel};
use crate::settings::{OverlayPosition, Settings};
use crate::sys;
use crate::util::{hinstance, register_class};

/// Transparent margin (DIPs) around the card — room for the move-mode outline.
const WINDOW_MARGIN: f32 = 6.0;
/// Distance (DIPs) between the overlay window and the monitor edge.
const EDGE_MARGIN: f32 = 14.0;

static MOVE_MODE: AtomicBool = AtomicBool::new(false);
/// True while the user is dragging the overlay (between WM_ENTERSIZEMOVE and WM_EXITSIZEMOVE).
static DRAGGING: AtomicBool = AtomicBool::new(false);

pub struct Overlay {
    pub hwnd: HWND,
    mem_dc: HDC,
    dib: HBITMAP,
    old_bitmap: HGDIOBJ,
    dib_size: (i32, i32),
    rt: ID2D1DCRenderTarget,
    paint: Paint,
    excluded: bool,
    shown: bool,
    /// Window top-left on screen (physical pixels).
    pos: (i32, i32),
    /// The card's box (card + transparent margin) in physical pixels. Placement and the saved
    /// custom position only ever use this, never the move-mode hint.
    card: (i32, i32),
    /// Where the card's box sits inside the window (non-zero when the hint is above / left).
    card_off: (i32, i32),
    /// Which side of the card the move-mode hint goes on; frozen while dragging.
    hint_above: bool,
    hint_right: bool,
}

impl Overlay {
    pub fn new(g: &Gfx) -> windows::core::Result<Overlay> {
        unsafe {
            register_class(w!("FpsViewer.Overlay"), Some(wndproc), false);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                w!("FpsViewer.Overlay"),
                w!("FPS Viewer Overlay"),
                WS_POPUP,
                -10000,
                -10000,
                1,
                1,
                None,
                None,
                Some(hinstance()),
                None,
            )
            .inspect_err(|e| crate::util::log(&format!("CreateWindowExW(overlay) failed: {e}")))?;

            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = crate::util::ctx(g.d2d.CreateDCRenderTarget(&props), "CreateDCRenderTarget")?;
            let paint = Paint::new(rt.cast()?)?;
            let mem_dc = CreateCompatibleDC(None);

            Ok(Overlay {
                hwnd,
                mem_dc,
                dib: HBITMAP::default(),
                old_bitmap: HGDIOBJ::default(),
                dib_size: (0, 0),
                rt,
                paint,
                excluded: false,
                shown: false,
                pos: (i32::MIN, i32::MIN),
                card: (0, 0),
                card_off: (0, 0),
                hint_above: false,
                hint_right: false,
            })
        }
    }

    pub fn move_mode(&self) -> bool {
        MOVE_MODE.load(Ordering::Relaxed)
    }

    pub fn set_move_mode(&mut self, on: bool) {
        MOVE_MODE.store(on, Ordering::Relaxed);
        unsafe {
            let mut ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32;
            if on {
                ex &= !WS_EX_TRANSPARENT.0;
            } else {
                ex |= WS_EX_TRANSPARENT.0;
            }
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex as isize);
        }
    }

    pub fn set_excluded_from_capture(&mut self, exclude: bool) {
        if self.excluded == exclude {
            return;
        }
        self.excluded = exclude;
        unsafe {
            let _ = SetWindowDisplayAffinity(self.hwnd, if exclude { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
        }
    }

    pub fn is_shown(&self) -> bool {
        self.shown
    }

    pub fn set_shown(&mut self, shown: bool) {
        if shown == self.shown {
            return;
        }
        self.shown = shown;
        unsafe {
            let _ = ShowWindow(self.hwnd, if shown { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    /// Draws `m` and places the window on the monitor hosting `target`.
    pub fn render(&mut self, g: &Gfx, m: &OverlayModel, target: HWND, s: &Settings) {
        let anchor = if target.is_invalid() { self.hwnd } else { target };
        let Some(mon) = sys::monitor_of(anchor) else { return };
        let k = m.scale * mon.dpi as f32 / 96.0;

        let (cw, ch) = overlay_draw::render(g, None, m);
        let card = (((cw + 2.0 * WINDOW_MARGIN) * k).ceil() as i32, ((ch + 2.0 * WINDOW_MARGIN) * k).ceil() as i32);

        // Where the card goes on screen. Mid-drag, it stays wherever the user has it.
        let dragging = DRAGGING.load(Ordering::Relaxed);
        let card_pos = if dragging {
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(self.hwnd, &mut r);
            }
            (r.left + self.card_off.0, r.top + self.card_off.1)
        } else {
            place(&mon, card, s)
        };

        // Move mode: the hint goes below the card, or above it near the bottom edge; left-
        // aligned, or right-aligned near the right edge. It never pushes the card around.
        let hint = if m.edit_mode {
            let (hw, hh) = overlay_draw::hint(g, None);
            let size = ((hw * k).ceil() as i32, (hh * k).ceil() as i32);
            let gap = (4.0 * k).round() as i32;
            let inset = (WINDOW_MARGIN * k).round() as i32;
            if !dragging {
                self.hint_above = card_pos.1 + card.1 + gap + size.1 > mon.rect.bottom;
                self.hint_right = card_pos.0 + inset + size.0 > mon.rect.right;
            }
            let x = if self.hint_right { card.0 - inset - size.0 } else { inset };
            let y = if self.hint_above { -gap - size.1 } else { card.1 + gap };
            Some((x, y, size))
        } else {
            None
        };

        // Window = card box ∪ hint, in card-box coordinates.
        let (mut x0, mut y0, mut x1, mut y1) = (0, 0, card.0, card.1);
        if let Some((hx, hy, (hw, hh))) = hint {
            x0 = x0.min(hx);
            y0 = y0.min(hy);
            x1 = x1.max(hx + hw);
            y1 = y1.max(hy + hh);
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let card_off = (-x0, -y0);
        if !self.ensure_dib(w, h) {
            return;
        }

        unsafe {
            let bounds = RECT { left: 0, top: 0, right: w, bottom: h };
            if self.rt.BindDC(self.mem_dc, &bounds).is_err() {
                return;
            }
            self.rt.BeginDraw();
            self.paint.clear(0);
            let margin = WINDOW_MARGIN * k;
            self.paint.set_transform(&scale_translate(k, card_off.0 as f32 + margin, card_off.1 as f32 + margin));
            overlay_draw::render(g, Some(&mut self.paint), m);
            if let Some((hx, hy, _)) = hint {
                self.paint.set_transform(&scale_translate(k, (card_off.0 + hx) as f32, (card_off.1 + hy) as f32));
                overlay_draw::hint(g, Some(&mut self.paint));
            }
            if self.rt.EndDraw(None, None).is_err() {
                return;
            }
        }

        let (x, y) = (card_pos.0 - card_off.0, card_pos.1 - card_off.1);
        self.pos = (x, y);
        self.card = card;
        self.card_off = card_off;
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (m.opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            // Mid-drag, pass no position so Windows keeps the window where the cursor has it.
            let dst = POINT { x, y };
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                if dragging { None } else { Some(&dst) },
                Some(&SIZE { cx: w, cy: h }),
                Some(self.mem_dc),
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }

    /// Re-places the window (current size) without redrawing.
    pub fn place_on(&mut self, target: HWND, s: &Settings) {
        if self.card.0 == 0 || DRAGGING.load(Ordering::Relaxed) {
            return;
        }
        let anchor = if target.is_invalid() { self.hwnd } else { target };
        let Some(mon) = sys::monitor_of(anchor) else { return };
        let (cx, cy) = place(&mon, self.card, s);
        let (x, y) = (cx - self.card_off.0, cy - self.card_off.1);
        if (x, y) == self.pos {
            return;
        }
        self.pos = (x, y);
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        }
    }

    /// Some fullscreen-borderless games grab the topmost band; politely take it back.
    pub fn reassert_topmost(&self) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
        }
    }

    /// After a drag: the card's position as fractions (0–1) of its monitor's free space.
    /// Only the card counts (not the move-mode hint), so it can sit flush in any corner.
    pub fn custom_fraction(&mut self) -> Option<(f64, f64)> {
        let mut r = RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut r).ok()? };
        self.pos = (r.left, r.top);
        let (cx, cy) = (r.left + self.card_off.0, r.top + self.card_off.1);
        let (w, h) = self.card;
        // The monitor under the card's centre (the hint may hang onto a neighbouring one).
        let centre = POINT { x: cx + w / 2, y: cy + h / 2 };
        let mon = unsafe { MonitorFromPoint(centre, MONITOR_DEFAULTTONEAREST) };
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        unsafe { GetMonitorInfoW(mon, &mut mi).ok().ok()? };
        let m = mi.rcMonitor;
        let free_w = (m.right - m.left - w) as f64;
        let free_h = (m.bottom - m.top - h) as f64;
        let fx = if free_w > 0.0 { (cx - m.left) as f64 / free_w } else { 0.0 };
        let fy = if free_h > 0.0 { (cy - m.top) as f64 / free_h } else { 0.0 };
        Some((fx.clamp(0.0, 1.0), fy.clamp(0.0, 1.0)))
    }

    fn ensure_dib(&mut self, w: i32, h: i32) -> bool {
        if self.dib_size == (w, h) && !self.dib.is_invalid() {
            return true;
        }
        unsafe {
            if !self.dib.is_invalid() {
                SelectObject(self.mem_dc, self.old_bitmap);
                let _ = DeleteObject(self.dib.into());
                self.dib = HBITMAP::default();
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            match CreateDIBSection(Some(self.mem_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(dib) => {
                    self.dib = dib;
                    self.old_bitmap = SelectObject(self.mem_dc, dib.into());
                    self.dib_size = (w, h);
                    true
                }
                Err(_) => false,
            }
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            if !self.dib.is_invalid() {
                SelectObject(self.mem_dc, self.old_bitmap);
                let _ = DeleteObject(self.dib.into());
            }
            let _ = DeleteDC(self.mem_dc);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Window position (physical pixels) for the chosen anchor on monitor `mon`.
fn place(mon: &sys::MonitorInfo, (w, h): (i32, i32), s: &Settings) -> (i32, i32) {
    let m = mon.rect;
    let margin = (EDGE_MARGIN * mon.dpi as f32 / 96.0).round() as i32;
    let (mw, mh) = (m.right - m.left, m.bottom - m.top);
    let left = m.left + margin;
    let right = m.right - w - margin;
    let center = m.left + (mw - w) / 2;
    let top = m.top + margin;
    let bottom = m.bottom - h - margin;
    match s.position {
        OverlayPosition::TopLeft => (left, top),
        OverlayPosition::TopCenter => (center, top),
        OverlayPosition::TopRight => (right, top),
        OverlayPosition::BottomLeft => (left, bottom),
        OverlayPosition::BottomCenter => (center, bottom),
        OverlayPosition::BottomRight => (right, bottom),
        OverlayPosition::Custom => (
            m.left + (s.custom_x * (mw - w).max(0) as f64).round() as i32,
            m.top + (s.custom_y * (mh - h).max(0) as f64).round() as i32,
        ),
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Click-through normally; in move mode the whole card acts as a title bar.
        WM_NCHITTEST => LRESULT(if MOVE_MODE.load(Ordering::Relaxed) { HTCAPTION as isize } else { HTTRANSPARENT as isize }),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_SETCURSOR if MOVE_MODE.load(Ordering::Relaxed) => {
            unsafe { SetCursor(LoadCursorW(None, IDC_SIZEALL).ok()) };
            LRESULT(1)
        }
        WM_ENTERSIZEMOVE => {
            DRAGGING.store(true, Ordering::Relaxed);
            LRESULT(0)
        }
        WM_EXITSIZEMOVE => {
            DRAGGING.store(false, Ordering::Relaxed);
            app::post(app::WM_APP_OVERLAY_MOVED, 0);
            LRESULT(0)
        }
        // The card acts as a title bar in move mode; never let a double-click maximize it.
        WM_NCLBUTTONDBLCLK => LRESULT(0),
        // We size ourselves per monitor; ignore Windows' suggested rectangle.
        WM_DPICHANGED => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
