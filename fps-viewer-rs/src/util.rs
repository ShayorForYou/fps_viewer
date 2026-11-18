//! Small helpers shared by every module: UTF-16 strings, the QPC clock and the log file.

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::WindowsAndMessaging::{
    LoadCursorW, LoadImageW, RegisterClassExW, CS_HREDRAW, CS_VREDRAW, HICON, IDC_ARROW, IMAGE_ICON,
    LR_DEFAULTCOLOR, LR_DEFAULTSIZE, WNDCLASSEXW, WNDPROC,
};

/// This executable's module handle.
pub fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).map(|h| HINSTANCE(h.0)).unwrap_or_default() }
}

/// The app icon (resource #1) at the given pixel size (0 = system default size).
pub fn app_icon(size: i32) -> Option<HICON> {
    unsafe {
        let flags = if size == 0 { LR_DEFAULTCOLOR | LR_DEFAULTSIZE } else { LR_DEFAULTCOLOR };
        LoadImageW(Some(hinstance()), PCWSTR(1 as *const u16), IMAGE_ICON, size, size, flags)
            .ok()
            .map(|h| HICON(h.0))
    }
}

/// Registers a window class with an arrow cursor and the app icon.
pub fn register_class(name: PCWSTR, proc: WNDPROC, redraw_on_resize: bool) {
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: if redraw_on_resize { CS_HREDRAW | CS_VREDRAW } else { Default::default() },
            lpfnWndProc: proc,
            hInstance: hinstance(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: app_icon(0).unwrap_or_default(),
            lpszClassName: name,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

/// NUL-terminated UTF-16 copy of `s`, for Win32 `PCWSTR` parameters.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Copies `s` into a fixed UTF-16 buffer (truncating, always NUL-terminated).
pub fn copy_wide(dst: &mut [u16], s: &str) {
    let max = dst.len().saturating_sub(1);
    let mut n = 0;
    for c in s.encode_utf16().take(max) {
        dst[n] = c;
        n += 1;
    }
    dst[n] = 0;
}

/// Converts a NUL-terminated (or full) UTF-16 buffer to a `String`.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Current QPC timestamp — the same clock ETW uses when `ClientContext = 1`.
#[inline]
pub fn qpc_now() -> i64 {
    let mut t = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut t);
    }
    t
}

/// QPC ticks per second (constant for the life of the system).
pub fn qpc_freq() -> i64 {
    static FREQ: OnceLock<i64> = OnceLock::new();
    *FREQ.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut f);
        }
        f.max(1)
    })
}

/// `%AppData%\FpsViewer` — settings and the diagnostics log live here.
pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("FpsViewer")
}

/// Appends a timestamped line to `log.txt`. Never fails loudly: logging must not take the app down.
pub fn log(message: &str) {
    let dir = app_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("log.txt");
    if std::fs::metadata(&path).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    let t = unsafe { GetLocalTime() };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}  {}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, message
        );
    }
}

/// Logs which step failed, then passes the result through (`ctx(call(), "CreateX")?`).
pub fn ctx<T>(r: windows::core::Result<T>, what: &str) -> windows::core::Result<T> {
    if let Err(e) = &r {
        log(&format!("{what} failed: {e}"));
    }
    r
}

/// Tiny xorshift PRNG for the settings preview's demo numbers (no `rand` dependency).
pub struct Rng(u64);

impl Rng {
    pub const fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }
}
