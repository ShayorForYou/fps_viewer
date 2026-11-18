//! User preferences, their JSON file (compatible with the C# version's `settings.json`)
//! and the accent themes.

use crate::util::{app_dir, log};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OverlayStyle {
    Minimal,
    Compact,
    Detailed,
}

impl OverlayStyle {
    pub const ALL: [OverlayStyle; 3] = [OverlayStyle::Minimal, OverlayStyle::Compact, OverlayStyle::Detailed];

    pub fn name(self) -> &'static str {
        match self {
            OverlayStyle::Minimal => "Minimal",
            OverlayStyle::Compact => "Compact",
            OverlayStyle::Detailed => "Detailed",
        }
    }

    pub fn next(self) -> Self {
        match self {
            OverlayStyle::Minimal => OverlayStyle::Compact,
            OverlayStyle::Compact => OverlayStyle::Detailed,
            OverlayStyle::Detailed => OverlayStyle::Minimal,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OverlayPosition {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
    Custom,
}

impl OverlayPosition {
    /// The six anchors, in the order the settings preview lays out its hotspots.
    pub const ANCHORS: [OverlayPosition; 6] = [
        OverlayPosition::TopLeft,
        OverlayPosition::TopCenter,
        OverlayPosition::TopRight,
        OverlayPosition::BottomLeft,
        OverlayPosition::BottomCenter,
        OverlayPosition::BottomRight,
    ];

    pub fn name(self) -> &'static str {
        match self {
            OverlayPosition::TopLeft => "TopLeft",
            OverlayPosition::TopCenter => "TopCenter",
            OverlayPosition::TopRight => "TopRight",
            OverlayPosition::BottomLeft => "BottomLeft",
            OverlayPosition::BottomCenter => "BottomCenter",
            OverlayPosition::BottomRight => "BottomRight",
            OverlayPosition::Custom => "Custom",
        }
    }
}

/// An accent palette: a two-stop gradient (colors are 0xRRGGBB).
pub struct Theme {
    pub name: &'static str,
    pub from: u32,
    pub to: u32,
    /// End color of the settings swatch (Mono's swatch is darker than its overlay accent).
    pub swatch_to: u32,
}

pub const THEMES: [Theme; 5] = [
    Theme { name: "Aurora", from: 0x8B5CF6, to: 0x22D3EE, swatch_to: 0x22D3EE },
    Theme { name: "Mint", from: 0x10B981, to: 0xA7F3D0, swatch_to: 0xA7F3D0 },
    Theme { name: "Sunset", from: 0xFB7185, to: 0xFDBA74, swatch_to: 0xFDBA74 },
    Theme { name: "Ocean", from: 0x3B82F6, to: 0x67E8F9, swatch_to: 0x67E8F9 },
    Theme { name: "Mono", from: 0xE5E7EB, to: 0x9CA3AF, swatch_to: 0x6B7280 },
];

fn theme_index(name: &str) -> Option<usize> {
    THEMES.iter().position(|t| t.name.eq_ignore_ascii_case(name))
}

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    pub overlay_enabled: bool,
    pub style: OverlayStyle,
    pub position: OverlayPosition,
    pub custom_x: f64,
    pub custom_y: f64,
    pub scale: f64,
    pub overlay_opacity: f64,
    /// Index into [`THEMES`].
    pub theme: usize,
    pub show_app_name: bool,
    pub show_graph: bool,
    pub color_by_performance: bool,
    /// 0 = match the display's refresh rate.
    pub target_fps: u32,
    pub update_hz: u32,
    pub auto_hide: bool,
    pub fullscreen_only: bool,
    pub battery_saver: bool,
    pub hide_from_capture: bool,
    pub vulkan_opengl_support: bool,
    pub start_with_windows: bool,
    pub first_run: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            overlay_enabled: true,
            style: OverlayStyle::Detailed,
            position: OverlayPosition::TopLeft,
            custom_x: 0.02,
            custom_y: 0.03,
            scale: 1.0,
            overlay_opacity: 0.92,
            theme: 0,
            show_app_name: true,
            show_graph: true,
            color_by_performance: true,
            target_fps: 0,
            update_hz: 2,
            auto_hide: true,
            fullscreen_only: false,
            battery_saver: true,
            hide_from_capture: false,
            vulkan_opengl_support: true,
            start_with_windows: false,
            first_run: true,
        }
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

impl Settings {
    /// Clamps every value into its valid range (mirrors the C# property setters).
    pub fn normalize(&mut self) {
        let finite = |v: f64, d: f64| if v.is_finite() { v } else { d };
        self.custom_x = finite(self.custom_x, 0.02).clamp(0.0, 1.0);
        self.custom_y = finite(self.custom_y, 0.03).clamp(0.0, 1.0);
        self.scale = round2(finite(self.scale, 1.0)).clamp(0.6, 2.0);
        self.overlay_opacity = round2(finite(self.overlay_opacity, 0.92)).clamp(0.3, 1.0);
        if self.theme >= THEMES.len() {
            self.theme = 0;
        }
        self.update_hz = self.update_hz.clamp(1, 10);
    }

    pub fn reset_appearance(&mut self) {
        let d = Settings::default();
        self.style = d.style;
        self.position = d.position;
        self.custom_x = d.custom_x;
        self.custom_y = d.custom_y;
        self.scale = d.scale;
        self.overlay_opacity = d.overlay_opacity;
        self.theme = d.theme;
        self.show_app_name = d.show_app_name;
        self.show_graph = d.show_graph;
        self.color_by_performance = d.color_by_performance;
        self.target_fps = d.target_fps;
    }

    pub fn theme(&self) -> &'static Theme {
        &THEMES[self.theme.min(THEMES.len() - 1)]
    }
}

// ───────────────────────────── persistence ─────────────────────────────

fn file_path() -> std::path::PathBuf {
    app_dir().join("settings.json")
}

pub fn load() -> Settings {
    let mut s = Settings::default();
    match std::fs::read_to_string(file_path()) {
        Ok(text) => match json::parse_flat_object(&text) {
            Some(fields) => {
                for (key, value) in fields {
                    apply_field(&mut s, &key, value);
                }
            }
            None => log("Settings could not be read, using defaults: invalid JSON"),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log(&format!("Settings could not be read, using defaults: {e}")),
    }
    s.normalize();
    s
}

fn apply_field(s: &mut Settings, key: &str, v: json::Value) {
    use json::Value::*;
    match (key, v) {
        ("OverlayEnabled", Bool(b)) => s.overlay_enabled = b,
        ("Style", Str(v)) => {
            if let Some(st) = OverlayStyle::ALL.into_iter().find(|x| x.name().eq_ignore_ascii_case(&v)) {
                s.style = st;
            }
        }
        ("Position", Str(v)) => {
            let mut all = OverlayPosition::ANCHORS.into_iter().chain([OverlayPosition::Custom]);
            if let Some(p) = all.find(|x| x.name().eq_ignore_ascii_case(&v)) {
                s.position = p;
            }
        }
        ("CustomX", Num(n)) => s.custom_x = n,
        ("CustomY", Num(n)) => s.custom_y = n,
        ("Scale", Num(n)) => s.scale = n,
        ("OverlayOpacity", Num(n)) => s.overlay_opacity = n,
        ("Theme", Str(v)) => s.theme = theme_index(&v).unwrap_or(0),
        ("ShowAppName", Bool(b)) => s.show_app_name = b,
        ("ShowGraph", Bool(b)) => s.show_graph = b,
        ("ColorByPerformance", Bool(b)) => s.color_by_performance = b,
        ("TargetFps", Num(n)) => s.target_fps = n.clamp(0.0, 10_000.0) as u32,
        ("UpdateHz", Num(n)) => s.update_hz = n.clamp(1.0, 10.0) as u32,
        ("AutoHide", Bool(b)) => s.auto_hide = b,
        ("FullscreenOnly", Bool(b)) => s.fullscreen_only = b,
        ("BatterySaver", Bool(b)) => s.battery_saver = b,
        ("HideFromCapture", Bool(b)) => s.hide_from_capture = b,
        ("VulkanOpenGlSupport", Bool(b)) => s.vulkan_opengl_support = b,
        ("StartWithWindows", Bool(b)) => s.start_with_windows = b,
        ("FirstRun", Bool(b)) => s.first_run = b,
        _ => {}
    }
}

pub fn save(s: &Settings) {
    let text = format!(
        "{{\n  \"OverlayEnabled\": {},\n  \"Style\": \"{}\",\n  \"Position\": \"{}\",\n  \"CustomX\": {},\n  \
         \"CustomY\": {},\n  \"Scale\": {},\n  \"OverlayOpacity\": {},\n  \"Theme\": \"{}\",\n  \
         \"ShowAppName\": {},\n  \"ShowGraph\": {},\n  \"ColorByPerformance\": {},\n  \"TargetFps\": {},\n  \
         \"UpdateHz\": {},\n  \"AutoHide\": {},\n  \"FullscreenOnly\": {},\n  \"BatterySaver\": {},\n  \
         \"HideFromCapture\": {},\n  \"VulkanOpenGlSupport\": {},\n  \"StartWithWindows\": {},\n  \
         \"FirstRun\": {}\n}}\n",
        s.overlay_enabled,
        s.style.name(),
        s.position.name(),
        s.custom_x,
        s.custom_y,
        s.scale,
        s.overlay_opacity,
        s.theme().name,
        s.show_app_name,
        s.show_graph,
        s.color_by_performance,
        s.target_fps,
        s.update_hz,
        s.auto_hide,
        s.fullscreen_only,
        s.battery_saver,
        s.hide_from_capture,
        s.vulkan_opengl_support,
        s.start_with_windows,
        s.first_run,
    );

    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(app_dir())?;
        let path = file_path();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path) // atomic replace on NTFS
    })();
    if let Err(e) = result {
        log(&format!("Settings could not be saved: {e}"));
    }
}

/// A deliberately tiny JSON reader: the settings file is one flat object of strings,
/// numbers and booleans. Nested values are skipped, so newer/older files still load.
mod json {
    pub enum Value {
        Str(String),
        Num(f64),
        Bool(bool),
        Other,
    }

    struct P<'a> {
        b: &'a [u8],
        i: usize,
    }

    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
        }
        fn peek(&mut self) -> Option<u8> {
            self.ws();
            self.b.get(self.i).copied()
        }
        fn eat(&mut self, c: u8) -> Option<()> {
            (self.peek()? == c).then(|| self.i += 1)
        }
        fn string(&mut self) -> Option<String> {
            self.eat(b'"')?;
            let mut out = String::new();
            loop {
                let c = *self.b.get(self.i)?;
                self.i += 1;
                match c {
                    b'"' => return Some(out),
                    b'\\' => {
                        let e = *self.b.get(self.i)?;
                        self.i += 1;
                        match e {
                            b'n' => out.push('\n'),
                            b't' => out.push('\t'),
                            b'r' => out.push('\r'),
                            b'b' => out.push('\u{8}'),
                            b'f' => out.push('\u{c}'),
                            b'u' => {
                                let hex = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                                self.i += 4;
                                out.push(char::from_u32(u32::from_str_radix(hex, 16).ok()?).unwrap_or('\u{FFFD}'));
                            }
                            other => out.push(other as char),
                        }
                    }
                    _ => {
                        // Copy a run of plain UTF-8 bytes in one go.
                        let start = self.i - 1;
                        while self.i < self.b.len() && self.b[self.i] != b'"' && self.b[self.i] != b'\\' {
                            self.i += 1;
                        }
                        out.push_str(std::str::from_utf8(&self.b[start..self.i]).ok()?);
                    }
                }
            }
        }
        fn value(&mut self) -> Option<Value> {
            match self.peek()? {
                b'"' => self.string().map(Value::Str),
                b't' => self.word(b"true").map(|_| Value::Bool(true)),
                b'f' => self.word(b"false").map(|_| Value::Bool(false)),
                b'n' => self.word(b"null").map(|_| Value::Other),
                b'{' | b'[' => self.skip_nested().map(|_| Value::Other),
                _ => {
                    let start = self.i;
                    while self.i < self.b.len()
                        && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                    {
                        self.i += 1;
                    }
                    std::str::from_utf8(&self.b[start..self.i]).ok()?.parse().ok().map(Value::Num)
                }
            }
        }
        fn word(&mut self, w: &[u8]) -> Option<()> {
            (self.b.get(self.i..self.i + w.len())? == w).then(|| self.i += w.len())
        }
        fn skip_nested(&mut self) -> Option<()> {
            let mut depth = 0usize;
            loop {
                match self.peek()? {
                    b'"' => {
                        self.string()?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            self.i += 1;
                            return Some(());
                        }
                    }
                    _ => {}
                }
                self.i += 1;
            }
        }
    }

    pub fn parse_flat_object(text: &str) -> Option<Vec<(String, Value)>> {
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
        let mut p = P { b: text.as_bytes(), i: 0 };
        p.eat(b'{')?;
        let mut out = Vec::new();
        if p.eat(b'}').is_some() {
            return Some(out);
        }
        loop {
            let key = p.string()?;
            p.eat(b':')?;
            out.push((key, p.value()?));
            match p.peek()? {
                b',' => p.i += 1,
                b'}' => return Some(out),
                _ => return None,
            }
        }
    }
}
