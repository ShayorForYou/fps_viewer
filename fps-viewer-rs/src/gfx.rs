//! Direct2D / DirectWrite plumbing shared by the overlay and the settings window: fonts,
//! cached text formats, per-render-target brush caches and small drawing helpers.
//! Colors are `0xAARRGGBB`; all coordinates are DIPs.

use std::cell::RefCell;
use std::mem::ManuallyDrop;

use windows::core::{Interface, Result, BOOL, PCWSTR};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::util::wide;

// ───────────────────────── colors & geometry ─────────────────────────

/// `0xAARRGGBB` → Direct2D color (straight alpha).
#[inline]
pub fn color(c: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((c >> 16) & 0xFF) as f32 / 255.0,
        g: ((c >> 8) & 0xFF) as f32 / 255.0,
        b: (c & 0xFF) as f32 / 255.0,
        a: ((c >> 24) & 0xFF) as f32 / 255.0,
    }
}

/// Opaque color from `0xRRGGBB`.
#[inline]
pub const fn rgb(c: u32) -> u32 {
    0xFF00_0000 | (c & 0x00FF_FFFF)
}

/// `0xRRGGBB` with the given alpha.
#[inline]
pub const fn argb(alpha: u8, c: u32) -> u32 {
    ((alpha as u32) << 24) | (c & 0x00FF_FFFF)
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.right() && py < self.bottom()
    }
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }
    pub fn offset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    pub fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.right(), bottom: self.bottom() }
    }
}

#[inline]
pub fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

pub fn translate(x: f32, y: f32) -> Matrix3x2 {
    Matrix3x2::translation(x, y)
}

/// Scale by `s`, then translate by `(x, y)`.
pub fn scale_translate(s: f32, x: f32, y: f32) -> Matrix3x2 {
    Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: x, M32: y }
}

// ───────────────────────── fonts & text ─────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Font {
    /// Segoe UI Variable Text → Segoe UI.
    Ui,
    /// Segoe UI Variable Display → Segoe UI.
    Display,
    /// Bahnschrift (tabular figures) → Segoe UI.
    Number,
}

pub const NORMAL: u16 = 400;
pub const SEMIBOLD: u16 = 600;
pub const BOLD: u16 = 700;

/// A measured DirectWrite layout.
#[derive(Clone)]
pub struct Text {
    pub layout: IDWriteTextLayout,
    pub w: f32,
    pub h: f32,
}

impl Text {
    fn measure(layout: IDWriteTextLayout) -> Text {
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.GetMetrics(&mut m);
        }
        Text { layout, w: m.widthIncludingTrailingWhitespace, h: m.height }
    }

    /// Re-measures after range formatting changes.
    pub fn remeasure(self) -> Text {
        Text::measure(self.layout)
    }

    /// Restyles characters `[start, start+len)` (UTF-16 units) with another size and/or weight.
    pub fn style_range(&self, start: u32, len: u32, size: Option<f32>, weight: Option<u16>) {
        let range = DWRITE_TEXT_RANGE { startPosition: start, length: len };
        unsafe {
            if let Some(s) = size {
                let _ = self.layout.SetFontSize(s, range);
            }
            if let Some(w) = weight {
                let _ = self.layout.SetFontWeight(DWRITE_FONT_WEIGHT(w as i32), range);
            }
        }
    }

    /// Distance from the layout's top to its first line's baseline — for lining up text of
    /// different sizes on one baseline.
    pub fn baseline(&self) -> f32 {
        let mut lines = [DWRITE_LINE_METRICS::default(); 1];
        let mut n = 0u32;
        unsafe {
            let _ = self.layout.GetLineMetrics(Some(&mut lines), &mut n);
        }
        if n > 0 {
            lines[0].baseline
        } else {
            self.h * 0.78
        }
    }

    /// Colors characters `[start, start+len)` with `brush` (a drawing effect).
    pub fn color_range(&self, start: u32, len: u32, brush: &ID2D1SolidColorBrush) {
        let range = DWRITE_TEXT_RANGE { startPosition: start, length: len };
        unsafe {
            let _ = self.layout.SetDrawingEffect(brush, range);
        }
    }
}

/// Device-independent graphics resources, created once per process.
pub struct Gfx {
    pub d2d: ID2D1Factory,
    dw: IDWriteFactory,
    families: [Vec<u16>; 3],
    formats: RefCell<Vec<(Font, u32, u16, IDWriteTextFormat)>>,
    tnum: IDWriteTypography,
    /// 1px "2 on / 3 off" — the graph's target line.
    pub dash_target: ID2D1StrokeStyle,
    /// "3 on / 2 off" (× stroke width) — the move-mode outline.
    pub dash_edit: ID2D1StrokeStyle,
    /// Round caps and joins — the graph line.
    pub round: ID2D1StrokeStyle,
}

impl Gfx {
    pub fn new() -> Result<Gfx> {
        unsafe {
            let d2d: ID2D1Factory = crate::util::ctx(D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None), "D2D1CreateFactory")?;
            let dw: IDWriteFactory = crate::util::ctx(DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED), "DWriteCreateFactory")?;

            let mut collection: Option<IDWriteFontCollection> = None;
            crate::util::ctx(dw.GetSystemFontCollection(&mut collection, false), "GetSystemFontCollection")?;
            let pick = |names: &[&str]| -> Vec<u16> {
                for n in names {
                    let w = wide(n);
                    let (mut index, mut exists) = (0u32, BOOL(0));
                    if let Some(c) = &collection {
                        if c.FindFamilyName(PCWSTR(w.as_ptr()), &mut index, &mut exists).is_ok() && exists.as_bool() {
                            return w;
                        }
                    }
                }
                wide("Segoe UI")
            };
            let families = [
                pick(&["Segoe UI Variable Text", "Segoe UI"]),
                pick(&["Segoe UI Variable Display", "Segoe UI"]),
                pick(&["Bahnschrift", "Segoe UI"]),
            ];

            let tnum = crate::util::ctx(dw.CreateTypography(), "CreateTypography")?;
            crate::util::ctx(tnum.AddFontFeature(DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, parameter: 1 }), "AddFontFeature")?;

            let stroke = |cap: D2D1_CAP_STYLE, join: D2D1_LINE_JOIN, dashes: Option<&[f32]>| {
                let props = D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: cap,
                    endCap: cap,
                    dashCap: D2D1_CAP_STYLE_FLAT,
                    lineJoin: join,
                    miterLimit: 10.0,
                    dashStyle: if dashes.is_some() { D2D1_DASH_STYLE_CUSTOM } else { D2D1_DASH_STYLE_SOLID },
                    dashOffset: 0.0,
                };
                d2d.CreateStrokeStyle(&props, dashes)
            };
            let dash_target = stroke(D2D1_CAP_STYLE_FLAT, D2D1_LINE_JOIN_MITER, Some(&[2.0, 3.0])).inspect_err(|e| crate::util::log(&format!("CreateStrokeStyle failed: {e}")))?;
            let dash_edit = stroke(D2D1_CAP_STYLE_FLAT, D2D1_LINE_JOIN_MITER, Some(&[3.0, 2.0]))?;
            let round = stroke(D2D1_CAP_STYLE_ROUND, D2D1_LINE_JOIN_ROUND, None)?;

            Ok(Gfx { d2d, dw, families, formats: RefCell::new(Vec::new()), tnum, dash_target, dash_edit, round })
        }
    }

    fn format(&self, font: Font, size: f32, weight: u16) -> IDWriteTextFormat {
        let key = (size * 100.0).round() as u32;
        let mut cache = self.formats.borrow_mut();
        if let Some((.., f)) = cache.iter().find(|(f, s, w, _)| *f == font && *s == key && *w == weight) {
            return f.clone();
        }
        let family = &self.families[font as usize];
        let f = unsafe {
            self.dw
                .CreateTextFormat(
                    PCWSTR(family.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT(weight as i32),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    windows::core::w!("en-us"),
                )
                .expect("CreateTextFormat")
        };
        unsafe {
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        }
        cache.push((font, key, weight, f.clone()));
        f
    }

    fn layout(&self, s: &str, font: Font, size: f32, weight: u16, max_w: f32) -> IDWriteTextLayout {
        let buf: Vec<u16> = s.encode_utf16().collect();
        let fmt = self.format(font, size, weight);
        let layout = unsafe { self.dw.CreateTextLayout(&buf, &fmt, max_w, 10_000.0).expect("CreateTextLayout") };
        if font == Font::Number {
            unsafe {
                let _ = layout.SetTypography(&self.tnum, DWRITE_TEXT_RANGE { startPosition: 0, length: buf.len() as u32 });
            }
        }
        layout
    }

    /// Single line, natural width.
    pub fn text(&self, s: &str, font: Font, size: f32, weight: u16) -> Text {
        Text::measure(self.layout(s, font, size, weight, 100_000.0))
    }

    /// Wrapped to `max_w`.
    pub fn text_wrapped(&self, s: &str, font: Font, size: f32, weight: u16, max_w: f32) -> Text {
        let layout = self.layout(s, font, size, weight, max_w.max(1.0));
        unsafe {
            let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
        }
        Text::measure(layout)
    }

    /// Single line, cut with "…" when wider than `max_w`.
    pub fn text_trimmed(&self, s: &str, font: Font, size: f32, weight: u16, max_w: f32) -> Text {
        let layout = self.layout(s, font, size, weight, max_w.max(1.0));
        unsafe {
            let trimming = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };
            if let Ok(sign) = self.dw.CreateEllipsisTrimmingSign(&layout) {
                let _ = layout.SetTrimming(&trimming, &sign);
            }
        }
        let mut t = Text::measure(layout);
        t.w = t.w.min(max_w);
        t
    }

    pub fn rounded_rect_geometry(&self, r: Rect, radius: f32) -> Option<ID2D1Geometry> {
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.d2d.CreateRoundedRectangleGeometry(&rr).ok().and_then(|g| g.cast().ok()) }
    }

    /// A polyline path. `filled` closes it into an area.
    pub fn path(&self, points: &[Vector2], filled: bool) -> Option<ID2D1Geometry> {
        if points.len() < 2 {
            return None;
        }
        unsafe {
            let geo = self.d2d.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            sink.BeginFigure(points[0], if filled { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
            sink.AddLines(&points[1..]);
            sink.EndFigure(if filled { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            sink.Close().ok()?;
            geo.cast().ok()
        }
    }
}

// ───────────────────────── painting ─────────────────────────

/// Device-dependent resources for one render target: a reusable solid brush plus small
/// caches of gradient and drawing-effect brushes, so steady-state drawing allocates little.
pub struct Paint {
    pub rt: ID2D1RenderTarget,
    solid: ID2D1SolidColorBrush,
    linear: Vec<(u64, ID2D1LinearGradientBrush)>,
    radial: Vec<(u64, ID2D1RadialGradientBrush)>,
    effects: Vec<(u32, ID2D1SolidColorBrush)>,
    layer: Option<ID2D1Layer>,
}

fn stops_key(stops: &[(f32, u32)]) -> u64 {
    // FNV-1a over the stop data.
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for (p, c) in stops {
        for b in p.to_bits().to_le_bytes().into_iter().chain(c.to_le_bytes()) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

impl Paint {
    pub fn new(rt: ID2D1RenderTarget) -> Result<Paint> {
        unsafe {
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let solid = crate::util::ctx(rt.CreateSolidColorBrush(&color(0xFFFF_FFFF), None), "CreateSolidColorBrush")?;
            Ok(Paint { rt, solid, linear: Vec::new(), radial: Vec::new(), effects: Vec::new(), layer: None })
        }
    }

    #[inline]
    fn solid(&self, c: u32) -> &ID2D1SolidColorBrush {
        unsafe { self.solid.SetColor(&color(c)) };
        &self.solid
    }

    pub fn set_transform(&self, m: &Matrix3x2) {
        unsafe { self.rt.SetTransform(m) };
    }

    pub fn clear(&self, c: u32) {
        unsafe { self.rt.Clear(Some(&color(c))) };
    }

    pub fn fill_rect(&self, r: Rect, c: u32) {
        unsafe { self.rt.FillRectangle(&r.d2d(), self.solid(c)) };
    }

    pub fn fill_round(&self, r: Rect, radius: f32, c: u32) {
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.rt.FillRoundedRectangle(&rr, self.solid(c)) };
    }

    pub fn fill_round_with(&self, r: Rect, radius: f32, brush: &ID2D1Brush) {
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.rt.FillRoundedRectangle(&rr, brush) };
    }

    /// Strokes centered on a rectangle inset by half the width, so the stroke stays inside `r`
    /// (like a WPF Border).
    pub fn stroke_round(&self, r: Rect, radius: f32, c: u32, width: f32) {
        let ri = (radius - width / 2.0).max(0.0);
        let rr = D2D1_ROUNDED_RECT { rect: r.inset(width / 2.0).d2d(), radiusX: ri, radiusY: ri };
        unsafe { self.rt.DrawRoundedRectangle(&rr, self.solid(c), width, None) };
    }

    pub fn stroke_round_with(&self, r: Rect, radius: f32, brush: &ID2D1Brush, width: f32, style: Option<&ID2D1StrokeStyle>) {
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.rt.DrawRoundedRectangle(&rr, brush, width, style) };
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, radius: f32, c: u32) {
        let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: radius, radiusY: radius };
        unsafe { self.rt.FillEllipse(&e, self.solid(c)) };
    }

    pub fn fill_circle_with(&self, cx: f32, cy: f32, radius: f32, brush: &ID2D1Brush) {
        let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: radius, radiusY: radius };
        unsafe { self.rt.FillEllipse(&e, brush) };
    }

    /// Stroke inside a circle of `radius` (like a WPF Ellipse with a stroke).
    pub fn stroke_circle(&self, cx: f32, cy: f32, radius: f32, c: u32, width: f32) {
        let r = (radius - width / 2.0).max(0.0);
        let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: r, radiusY: r };
        unsafe { self.rt.DrawEllipse(&e, self.solid(c), width, None) };
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: u32, width: f32, style: Option<&ID2D1StrokeStyle>) {
        unsafe { self.rt.DrawLine(pt(x0, y0), pt(x1, y1), self.solid(c), width, style) };
    }

    pub fn fill_geometry(&self, g: &ID2D1Geometry, c: u32) {
        unsafe { self.rt.FillGeometry(g, self.solid(c), None) };
    }

    pub fn fill_geometry_with(&self, g: &ID2D1Geometry, brush: &ID2D1Brush) {
        unsafe { self.rt.FillGeometry(g, brush, None) };
    }

    pub fn stroke_geometry(&self, g: &ID2D1Geometry, c: u32, width: f32, style: Option<&ID2D1StrokeStyle>) {
        unsafe { self.rt.DrawGeometry(g, self.solid(c), width, style) };
    }

    pub fn text(&self, t: &Text, x: f32, y: f32, c: u32) {
        unsafe { self.rt.DrawTextLayout(pt(x, y), &t.layout, self.solid(c), D2D1_DRAW_TEXT_OPTIONS_NONE) };
    }

    /// A brush for coloring a text range (must be distinct from the shared solid brush).
    pub fn effect(&mut self, c: u32) -> ID2D1SolidColorBrush {
        if let Some((_, b)) = self.effects.iter().find(|(k, _)| *k == c) {
            return b.clone();
        }
        let b = unsafe { self.rt.CreateSolidColorBrush(&color(c), None).expect("CreateSolidColorBrush") };
        self.effects.push((c, b.clone()));
        b
    }

    /// Linear gradient from `(x0, y0)` to `(x1, y1)`; stops are `(offset, 0xAARRGGBB)`.
    pub fn linear(&mut self, stops: &[(f32, u32)], x0: f32, y0: f32, x1: f32, y1: f32) -> ID2D1Brush {
        let key = stops_key(stops);
        let brush = match self.linear.iter().find(|(k, _)| *k == key) {
            Some((_, b)) => b.clone(),
            None => unsafe {
                let gs: Vec<D2D1_GRADIENT_STOP> =
                    stops.iter().map(|&(p, c)| D2D1_GRADIENT_STOP { position: p, color: color(c) }).collect();
                let coll = self.rt.CreateGradientStopCollection(&gs, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).expect("stops");
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: pt(x0, y0), endPoint: pt(x1, y1) };
                let b = self.rt.CreateLinearGradientBrush(&props, None, &coll).expect("linear brush");
                if self.linear.len() > 24 {
                    self.linear.remove(0);
                }
                self.linear.push((key, b.clone()));
                b
            },
        };
        unsafe {
            brush.SetStartPoint(pt(x0, y0));
            brush.SetEndPoint(pt(x1, y1));
        }
        brush.cast().expect("brush")
    }

    /// Radial gradient centered at `(cx, cy)`.
    pub fn radial(&mut self, stops: &[(f32, u32)], cx: f32, cy: f32, rx: f32, ry: f32) -> ID2D1Brush {
        let key = stops_key(stops);
        let brush = match self.radial.iter().find(|(k, _)| *k == key) {
            Some((_, b)) => b.clone(),
            None => unsafe {
                let gs: Vec<D2D1_GRADIENT_STOP> =
                    stops.iter().map(|&(p, c)| D2D1_GRADIENT_STOP { position: p, color: color(c) }).collect();
                let coll = self.rt.CreateGradientStopCollection(&gs, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).expect("stops");
                let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: pt(cx, cy),
                    gradientOriginOffset: pt(0.0, 0.0),
                    radiusX: rx,
                    radiusY: ry,
                };
                let b = self.rt.CreateRadialGradientBrush(&props, None, &coll).expect("radial brush");
                self.radial.push((key, b.clone()));
                b
            },
        };
        unsafe {
            brush.SetCenter(pt(cx, cy));
            brush.SetRadiusX(rx);
            brush.SetRadiusY(ry);
        }
        brush.cast().expect("brush")
    }

    /// Pushes a layer with an optional geometric clip and an opacity. Pair with [`Paint::pop`].
    pub fn push_layer(&mut self, mask: Option<&ID2D1Geometry>, opacity: f32) {
        unsafe {
            if self.layer.is_none() {
                self.layer = self.rt.CreateLayer(None).ok();
            }
            let params = D2D1_LAYER_PARAMETERS {
                contentBounds: D2D_RECT_F { left: -1e9, top: -1e9, right: 1e9, bottom: 1e9 },
                geometricMask: ManuallyDrop::new(mask.cloned()),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity,
                opacityBrush: ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS_NONE,
            };
            self.rt.PushLayer(&params, self.layer.as_ref());
            // The render target AddRef'd the mask; release our copy.
            drop(ManuallyDrop::into_inner(params.geometricMask));
        }
    }

    pub fn pop(&self) {
        unsafe { self.rt.PopLayer() };
    }

    pub fn push_clip(&self, r: Rect) {
        unsafe { self.rt.PushAxisAlignedClip(&r.d2d(), D2D1_ANTIALIAS_MODE_ALIASED) };
    }

    pub fn pop_clip(&self) {
        unsafe { self.rt.PopAxisAlignedClip() };
    }
}

// ───────────────────────── shared artwork ─────────────────────────

pub const ACCENT_FROM: u32 = 0xFF8B_5CF6;
pub const ACCENT_TO: u32 = 0xFF22_D3EE;

/// The app glyph: violet→cyan rounded tile with three rising white bars (32-unit grid).
pub fn draw_glyph(p: &mut Paint, x: f32, y: f32, size: f32) {
    let k = size / 32.0;
    let tile = Rect::new(x, y, size, size);
    let grad = p.linear(&[(0.0, ACCENT_FROM), (1.0, ACCENT_TO)], x, y, x + size, y + size);
    p.fill_round_with(tile, 8.5 * k, &grad);
    for (bx, by, bw, bh) in [(8.4, 16.6, 4.5, 7.7), (13.75, 12.2, 4.5, 12.1), (19.1, 7.7, 4.5, 16.6)] {
        p.fill_round(Rect::new(x + bx * k, y + by * k, bw * k, bh * k), 2.25 * k, 0xFFFF_FFFF);
    }
}
