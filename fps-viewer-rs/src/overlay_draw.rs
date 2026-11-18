//! The overlay card — Minimal, Compact and Detailed styles, the frame-time graph and the
//! move-mode outline. Shared by the real overlay window and the settings preview, so both
//! always look identical. Sizes are the original WPF design's DIPs.

use std::fmt::Write;

use crate::gfx::{argb, pt, rgb, Font, Gfx, Paint, Rect, Text, BOLD, NORMAL, SEMIBOLD};
use crate::settings::{OverlayStyle, Settings, THEMES};
use crate::stats::{FrameStats, GRAPH_POINTS};
use crate::sys::Load;

pub const GOOD: u32 = rgb(0x34D399);
pub const OK: u32 = rgb(0xFBBF24);
pub const BAD: u32 = rgb(0xFB7185);
pub const NEUTRAL: u32 = rgb(0xF5F7FA);
pub const IDLE: u32 = rgb(0x8A93A6);

const MUTED: u32 = 0x8FFF_FFFF;
const FAINT: u32 = 0x5CFF_FFFF;
const VALUE: u32 = 0xEDFF_FFFF;
const VALUE_COMPACT: u32 = 0xE6FF_FFFF;

/// Everything the overlay renders.
pub struct OverlayModel {
    pub fps_text: String,
    /// Whole-PC CPU busy %.
    pub cpu_text: String,
    /// Whole-PC memory in use, GB.
    pub ram_text: String,
    /// The muted tail after the RAM figure — "/ 16 GB". Rebuilt only when the total changes.
    pub ram_unit: String,
    ram_total: i64,
    pub app_name: String,
    pub fps_color: u32,
    pub graph: [f32; GRAPH_POINTS],
    pub graph_len: usize,
    pub target_ms: f64,
    pub active: bool,
    pub source: &'static str,

    pub style: OverlayStyle,
    pub scale: f32,
    pub opacity: f32,
    pub show_app_name: bool,
    pub show_graph: bool,
    pub edit_mode: bool,
    pub theme: usize,
}

impl OverlayModel {
    pub fn new(app_name: &str) -> Self {
        OverlayModel {
            fps_text: "--".into(),
            cpu_text: "--".into(),
            ram_text: "--".into(),
            ram_unit: "GB".into(),
            ram_total: -1,
            app_name: app_name.into(),
            fps_color: IDLE,
            graph: [0.0; GRAPH_POINTS],
            graph_len: 0,
            target_ms: 1000.0 / 60.0,
            active: false,
            source: "",
            style: OverlayStyle::Detailed,
            scale: 1.0,
            opacity: 0.92,
            show_app_name: true,
            show_graph: true,
            edit_mode: false,
            theme: 0,
        }
    }

    pub fn apply_appearance(&mut self, s: &Settings, low_power: bool) {
        self.style = s.style;
        self.scale = s.scale as f32;
        self.opacity = s.overlay_opacity as f32;
        self.show_app_name = s.show_app_name;
        self.show_graph = s.show_graph && !low_power;
        self.theme = s.theme;
    }

    pub fn apply_stats(&mut self, st: &FrameStats, target_fps: f64, color_by_performance: bool) {
        let target = if target_fps > 0.0 { target_fps } else { 60.0 };
        self.target_ms = 1000.0 / target;

        if !st.active {
            self.active = false;
            self.fps_text.clear();
            self.fps_text.push_str("--");
            self.fps_color = IDLE;
            self.graph_len = 0;
            self.source = "";
            return;
        }

        self.active = true;
        set_int(&mut self.fps_text, st.fps);
        self.fps_color = if !color_by_performance {
            NEUTRAL
        } else {
            let ratio = st.fps / target;
            if ratio >= 0.9 {
                GOOD
            } else if ratio >= 0.5 {
                OK
            } else {
                BAD
            }
        };
        self.graph = st.graph;
        self.graph_len = st.graph_len;
        self.source = if st.from_kernel { "VK/GL" } else { "DX" };
    }

    /// CPU and memory are system-wide, so they're shown whether or not a game is measured.
    pub fn apply_system(&mut self, load: &Load) {
        if load.cpu >= 0.0 {
            set_int(&mut self.cpu_text, load.cpu);
        } else {
            self.cpu_text.clear();
            self.cpu_text.push_str("--");
        }
        self.ram_text.clear();
        if load.ram_used_gb >= 0.0 {
            let _ = write!(self.ram_text, "{:.1}", load.ram_used_gb);
        } else {
            self.ram_text.push_str("--");
        }
        // Whole GB, the way RAM is sold ("16", not "15.8"). Practically never changes.
        let total = if load.ram_total_gb > 0.0 { load.ram_total_gb.round().max(1.0) as i64 } else { -1 };
        if total != self.ram_total {
            self.ram_total = total;
            self.ram_unit.clear();
            if total > 0 {
                let _ = write!(self.ram_unit, "/\u{2009}{total}\u{2009}GB");
            } else {
                self.ram_unit.push_str("GB");
            }
        }
    }

    fn accent(&self) -> (u32, u32) {
        let t = &THEMES[self.theme.min(THEMES.len() - 1)];
        (rgb(t.from), rgb(t.to))
    }
}

fn set_int(s: &mut String, v: f64) {
    s.clear();
    let _ = write!(s, "{:.0}", v.round());
}

/// Lays out the card with its top-left at (0, 0) (the move-mode outline extends 4 DIPs beyond
/// it). Draws it when `paint` is given. Returns the card size in DIPs — never including the
/// move-mode hint, so the card's position doesn't depend on whether move mode is on.
pub fn render(g: &Gfx, mut paint: Option<&mut Paint>, m: &OverlayModel) -> (f32, f32) {
    let (w, h, radius) = match m.style {
        OverlayStyle::Detailed => detailed(g, paint.as_deref_mut(), m),
        OverlayStyle::Compact => compact(g, paint.as_deref_mut(), m),
        OverlayStyle::Minimal => minimal(g, paint.as_deref_mut(), m),
    };
    if m.edit_mode {
        if let Some(p) = paint {
            // Move mode: dashed accent outline.
            let (a, b) = m.accent();
            let outline = Rect::new(-4.0, -4.0, w + 8.0, h + 8.0);
            let brush = p.linear(&[(0.0, a), (1.0, b)], outline.x, 0.0, outline.right(), 0.0);
            p.stroke_round_with(outline.inset(0.75), (radius + 4.0).min(16.0), &brush, 1.5, Some(&g.dash_edit));
        }
    }
    (w, h)
}

/// The move-mode hint pill with its top-left at (0, 0). Draws it when `paint` is given and
/// returns its size in DIPs. The overlay window places it beside the card wherever it fits.
pub fn hint(g: &Gfx, paint: Option<&mut Paint>) -> (f32, f32) {
    let text = g.text("Drag to move  ·  Alt+Shift+M to lock", Font::Ui, 11.0, NORMAL);
    text.style_range(0, 12, None, Some(SEMIBOLD));
    let text = text.remeasure();
    let pill = Rect::new(0.0, 0.0, text.w + 20.0, text.h + 10.0);
    if let Some(p) = paint {
        let muted = p.effect(MUTED);
        text.color_range(12, 100, &muted);
        p.fill_round(pill, 8.0, 0xE610_1217);
        p.text(&text, pill.x + 10.0, pill.y + 5.0, 0xD9FF_FFFF);
    }
    (pill.w, pill.h)
}

fn card(p: &mut Paint, w: f32, h: f32, radius: f32) {
    let r = Rect::new(0.0, 0.0, w, h);
    let bg = p.linear(&[(0.0, 0xEB15_171D), (1.0, 0xE60C_0D12)], 0.0, 0.0, 0.0, h);
    p.fill_round_with(r, radius, &bg);
    let stroke = p.linear(&[(0.0, 0x38FF_FFFF), (0.5, 0x14FF_FFFF), (1.0, 0x0AFF_FFFF)], 0.0, 0.0, 0.0, h);
    p.stroke_round_with(r.inset(0.5), radius - 0.5, &stroke, 1.0, None);
}

fn accent_dot(p: &mut Paint, m: &OverlayModel, cx: f32, cy: f32, radius: f32) {
    let (a, b) = m.accent();
    let brush = p.linear(&[(0.0, a), (1.0, b)], cx - radius, cy, cx + radius, cy);
    p.fill_circle_with(cx, cy, radius, &brush);
}

// ───────────────────────── one visual language ─────────────────────────

/// One reading, drawn the same way everywhere: a semibold number (with an optional smaller,
/// muted unit) and a small bold label on the same baseline — "60 FPS", "34% CPU", "9.1 / 16 GB RAM".
struct Metric {
    value: Text,
    label: Text,
    /// Where the unit starts and how long it is (UTF-16 units).
    unit_at: Option<(u32, u32)>,
    gap: f32,
    /// Width floor so the card doesn't twitch as digits change (0 = none).
    min_w: f32,
}

impl Metric {
    /// `reserve` is a sample value ("888") whose width the metric never shrinks below.
    fn new(g: &Gfx, value: &str, unit: &str, label: &str, size: f32, label_size: f32, reserve: &str) -> Metric {
        let gap = (size * 0.15).max(4.0).round();
        let label = g.text(label, Font::Ui, label_size, BOLD);
        let (value, unit_at) = Self::number(g, value, unit, size);
        let min_w = if reserve.is_empty() { 0.0 } else { Self::number(g, reserve, unit, size).0.w + gap + label.w };
        Metric { value, label, unit_at, gap, min_w }
    }

    fn number(g: &Gfx, value: &str, unit: &str, size: f32) -> (Text, Option<(u32, u32)>) {
        if unit.is_empty() || value == "--" {
            return (g.text(value, Font::Number, size, SEMIBOLD), None);
        }
        let sep = if unit == "%" { "" } else { "\u{2009}" }; // thin space before "GB" / "/ 16 GB"
        let at = value.encode_utf16().count() as u32;
        let t = g.text(&format!("{value}{sep}{unit}"), Font::Number, size, SEMIBOLD);
        let len = (sep.encode_utf16().count() + unit.encode_utf16().count()) as u32;
        t.style_range(at, len, Some((size * 0.62).round()), None);
        (t.remeasure(), Some((at, len)))
    }

    fn w(&self) -> f32 {
        (self.value.w + self.gap + self.label.w).max(self.min_w).ceil()
    }
    /// Top of the metric → its baseline.
    fn ascent(&self) -> f32 {
        self.value.baseline().max(self.label.baseline())
    }
    /// Baseline → bottom of the metric.
    fn descent(&self) -> f32 {
        (self.value.h - self.value.baseline()).max(self.label.h - self.label.baseline())
    }

    /// Draws with the shared baseline at `base`.
    fn draw(&self, p: &mut Paint, x: f32, base: f32, color: u32) {
        if let Some((at, len)) = self.unit_at {
            let muted = p.effect(MUTED);
            self.value.color_range(at, len, &muted);
        }
        p.text(&self.value, x, base - self.value.baseline(), color);
        p.text(&self.label, x + self.value.w + self.gap, base - self.label.baseline(), FAINT);
    }
}

// ───────────────────────── Detailed ─────────────────────────

fn detailed(g: &Gfx, mut paint: Option<&mut Paint>, m: &OverlayModel) -> (f32, f32, f32) {
    const W: f32 = 214.0;
    const PAD_L: f32 = 14.0;
    const INNER: f32 = W - 28.0;
    let mut y = 11.0;

    let app_row = if m.show_app_name {
        let label = g.text(m.source, Font::Ui, 9.0, SEMIBOLD);
        let label_space = if m.source.is_empty() { 0.0 } else { label.w + 8.0 };
        let name = g.text_trimmed(&m.app_name, Font::Ui, 11.0, SEMIBOLD, INNER - 13.0 - label_space);
        let row_h = name.h.max(label.h);
        let row_y = y;
        y += row_h + 2.0;
        Some((label, name, row_y, row_h))
    } else {
        None
    };

    // Hero: FPS.
    let fps = Metric::new(g, &m.fps_text, "", "FPS", 40.0, 11.0, "");
    let fps_base = y + fps.ascent();
    y += fps.ascent() + fps.descent();

    // Hairline, then CPU and RAM side by side — the same shape as FPS, one size down.
    y += 6.0;
    let rule_y = y;
    y += 9.0;
    let cpu = Metric::new(g, &m.cpu_text, "%", "CPU", 17.0, 9.5, "");
    let ram = Metric::new(g, &m.ram_text, &m.ram_unit, "RAM", 17.0, 9.5, "");
    let ascent = cpu.ascent().max(ram.ascent());
    let sys_base = y + ascent;
    y += ascent + cpu.descent().max(ram.descent());

    // Graph, with a one-line legend: time span · dashed target line · red spike dots.
    let graph = if m.show_graph {
        y += 12.0;
        let span = g.text("LAST 4 SECONDS", Font::Ui, 8.5, SEMIBOLD);
        let target = g.text("TARGET", Font::Ui, 8.5, SEMIBOLD);
        let spike = g.text("SPIKE", Font::Ui, 8.5, SEMIBOLD);
        let legend_y = y;
        y += span.h + 4.0;
        let r = Rect::new(PAD_L, y, INNER, 34.0);
        y += 34.0;
        Some((r, legend_y, span, target, spike))
    } else {
        None
    };
    let h = y + 12.0;

    if let Some(p) = paint.as_deref_mut() {
        card(p, W, h, 16.0);

        if let Some((label, name, ry, rh)) = &app_row {
            accent_dot(p, m, PAD_L + 3.0, ry + rh / 2.0 + 0.5, 3.0);
            p.text(name, PAD_L + 13.0, ry + (rh - name.h) / 2.0, MUTED);
            if !m.source.is_empty() {
                p.text(label, W - PAD_L - label.w, ry + (rh - label.h) / 2.0, FAINT);
            }
        }

        fps.draw(p, PAD_L, fps_base, m.fps_color);
        p.fill_rect(Rect::new(PAD_L, rule_y, INNER, 1.0), 0x14FF_FFFF);
        cpu.draw(p, PAD_L, sys_base, VALUE);
        // Right-aligned so "12.3 / 32 GB RAM" always fits, flush with the card's right edge.
        ram.draw(p, W - PAD_L - ram.w(), sys_base, VALUE);

        if let Some((r, ly, span, target, spike)) = &graph {
            p.text(span, PAD_L, *ly, FAINT);
            // Right-aligned legend: "╌ TARGET   ● SPIKE"
            let mut x = W - PAD_L - spike.w;
            p.text(spike, x, *ly, FAINT);
            x -= 4.0 + 2.0;
            p.fill_circle(x, ly + spike.h / 2.0, 2.0, BAD);
            x -= 2.0 + 10.0 + target.w;
            p.text(target, x, *ly, FAINT);
            x -= 4.0 + 10.0;
            let my = (ly + target.h / 2.0).round() + 0.5;
            p.line(x, my, x + 10.0, my, 0x66FF_FFFF, 1.0, Some(&g.dash_target));

            let (_, accent) = m.accent();
            draw_graph(g, p, *r, &m.graph[..m.graph_len], m.target_ms as f32, accent);
        }
    }
    (W, h, 16.0)
}

// ───────────────────────── Compact ─────────────────────────

/// One line, one baseline: ● 60 FPS │ 34% CPU │ 9.1 / 16 GB RAM
fn compact(g: &Gfx, paint: Option<&mut Paint>, m: &OverlayModel) -> (f32, f32, f32) {
    const PAD_X: f32 = 12.0;
    const PAD_Y: f32 = 7.0;
    const SEP: f32 = 12.0; // space on each side of a divider
    let metrics = [
        Metric::new(g, &m.fps_text, "", "FPS", 21.0, 9.5, "888"),
        Metric::new(g, &m.cpu_text, "%", "CPU", 15.0, 8.5, "88%"),
        Metric::new(g, &m.ram_text, &m.ram_unit, "RAM", 15.0, 8.5, "88.8"),
    ];
    let ascent = metrics.iter().map(Metric::ascent).fold(0.0, f32::max);
    let descent = metrics.iter().map(Metric::descent).fold(0.0, f32::max);
    let dot = 6.0 + 9.0;
    let w = PAD_X + dot + metrics.iter().map(Metric::w).sum::<f32>() + 2.0 * (2.0 * SEP + 1.0) + PAD_X + 2.0;
    let h = PAD_Y + ascent + descent + PAD_Y;

    if let Some(p) = paint {
        card(p, w, h, 12.0);
        let base = PAD_Y + ascent;
        let mid = base - 21.0 * 0.36; // optical middle of the FPS digits
        let mut x = PAD_X;
        accent_dot(p, m, x + 3.0, mid, 3.0);
        x += dot;
        for (i, metric) in metrics.iter().enumerate() {
            if i > 0 {
                x += SEP;
                p.fill_rect(Rect::new(x, (mid - 8.0).round(), 1.0, 16.0), 0x26FF_FFFF);
                x += 1.0 + SEP;
            }
            metric.draw(p, x, base, if i == 0 { m.fps_color } else { VALUE_COMPACT });
            x += metric.w();
        }
    }
    (w, h, 12.0)
}

// ───────────────────────── Minimal ─────────────────────────

fn minimal(g: &Gfx, paint: Option<&mut Paint>, m: &OverlayModel) -> (f32, f32, f32) {
    let fps = Metric::new(g, &m.fps_text, "", "FPS", 17.0, 8.5, "");
    let (ascent, descent) = (fps.ascent(), fps.descent());
    let w = 9.0 + 7.0 + 7.0 + fps.w() + 11.0;
    let h = 3.0 + ascent + descent + 3.0;
    if let Some(p) = paint {
        card(p, w, h, 10.0);
        let base = 3.0 + ascent;
        p.fill_circle(9.0 + 3.5, base - 17.0 * 0.36, 3.5, m.fps_color);
        fps.draw(p, 23.0, base, 0xFFFF_FFFF);
    }
    (w, h, 10.0)
}

// ───────────────────────── frame-time graph ─────────────────────────

/// Frame-time sparkline: gradient area, line, dashed target guide and red dots on stutters
/// (frames taking more than twice the target budget).
fn draw_graph(g: &Gfx, p: &mut Paint, r: Rect, s: &[f32], target_ms: f32, accent: u32) {
    if r.w <= 2.0 || r.h <= 2.0 {
        return;
    }
    p.line(r.x, r.bottom() - 0.5, r.right(), r.bottom() - 0.5, 0x1AFF_FFFF, 1.0, None);
    let n = s.len().min(GRAPH_POINTS);
    if n < 2 {
        return;
    }

    let target = if target_ms > 0.0 { target_ms } else { 16.7 };
    let peak = s[..n].iter().copied().fold(0.0f32, f32::max);
    let max = (target * 1.6).max(peak * 1.12);
    let y_of = |v: f32| r.y + r.h - 1.0 - (v / max).clamp(0.0, 1.0) * (r.h - 3.0);
    let step = r.w / (n - 1) as f32;

    // [baseline-left, line points…, baseline-right] — the area; the middle slice is the line.
    let mut pts = [pt(0.0, 0.0); GRAPH_POINTS + 2];
    pts[0] = pt(r.x, r.bottom());
    for i in 0..n {
        pts[i + 1] = pt(r.x + i as f32 * step, y_of(s[i]));
    }
    pts[n + 1] = pt(r.right(), r.bottom());

    if let Some(area) = g.path(&pts[..n + 2], true) {
        let fill = p.linear(&[(0.0, argb(0x55, accent)), (1.0, argb(0x00, accent))], 0.0, r.y, 0.0, r.bottom());
        p.fill_geometry_with(&area, &fill);
    }

    let ty = y_of(target);
    p.line(r.x, ty, r.right(), ty, 0x40FF_FFFF, 1.0, Some(&g.dash_target));

    if let Some(line) = g.path(&pts[1..n + 1], false) {
        p.stroke_geometry(&line, accent, 1.5, Some(&g.round));
    }

    for i in 0..n {
        let v = s[i];
        if v > target * 2.0 && (i == 0 || v >= s[i - 1]) && (i == n - 1 || v >= s[i + 1]) {
            p.fill_circle(r.x + i as f32 * step, y_of(v), 2.0, BAD);
        }
    }
}
