//! Thin wrappers over the Windows APIs the app needs: process names, monitors, power,
//! "Efficiency mode", working-set trimming and the start-with-Windows task.

use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use windows::core::{w, PCSTR, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromWindow, DEVMODEW, ENUM_CURRENT_SETTINGS, MONITORINFO,
    MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetSystemTimes, OpenProcess, ProcessPowerThrottling, QueryFullProcessImageNameW, SetPriorityClass,
    SetProcessInformation, BELOW_NORMAL_PRIORITY_CLASS, PROCESS_NAME_WIN32, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, GetWindowThreadProcessId};

use crate::util::{from_wide, log, wide};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ───────────────────────── processes ─────────────────────────

pub fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

pub fn process_image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..size as usize]))
    }
}

/// File name without directory or extension (`C:\Games\foo.exe` → `foo`).
pub fn file_stem(path: &str) -> &str {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// The product name from the executable's version resource, falling back to the file name.
pub fn friendly_name(path: Option<&str>, exe: &str) -> String {
    if let Some(path) = path {
        for key in ["ProductName", "FileDescription"] {
            if let Some(v) = version_string(path, key) {
                let c = v.trim();
                let len = c.chars().count();
                if len > 1
                    && len <= 40
                    && !contains_ci(c, "Unreal Engine")
                    && !contains_ci(c, "Operating System")
                    && !c.eq_ignore_ascii_case("Launcher")
                {
                    return c.to_string();
                }
            }
        }
    }
    if exe.is_empty() { "Unknown app".into() } else { exe.into() }
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}

fn version_string(path: &str, key: &str) -> Option<String> {
    unsafe {
        let wpath = wide(path);
        let size = GetFileVersionInfoSizeW(PCWSTR(wpath.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wpath.as_ptr()), None, size, data.as_mut_ptr() as *mut c_void).ok()?;

        // First listed language/code page, then US English as a fallback.
        let mut langs: Vec<String> = Vec::with_capacity(2);
        let mut ptr: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        if VerQueryValueW(data.as_ptr() as *const c_void, w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len)
            .as_bool()
            && len >= 4
        {
            let pair = ptr as *const u16;
            langs.push(format!("{:04x}{:04x}", pair.read_unaligned(), pair.add(1).read_unaligned()));
        }
        langs.push("040904b0".into());

        for lang in langs {
            let q = wide(&format!("\\StringFileInfo\\{lang}\\{key}"));
            let mut p: *mut c_void = std::ptr::null_mut();
            let mut n = 0u32;
            if VerQueryValueW(data.as_ptr() as *const c_void, PCWSTR(q.as_ptr()), &mut p, &mut n).as_bool() && n > 0 {
                let s = from_wide(std::slice::from_raw_parts(p as *const u16, n as usize));
                if !s.trim().is_empty() {
                    return Some(s);
                }
            }
        }
        None
    }
}

// ───────────────────────── monitors ─────────────────────────

pub struct MonitorInfo {
    /// Full monitor rectangle in physical pixels.
    pub rect: RECT,
    pub dpi: u32,
    device: [u16; 32],
}

pub fn monitor_of(hwnd: HWND) -> Option<MonitorInfo> {
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(mon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            return None;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_err() || dx == 0 {
            dx = 96;
        }
        Some(MonitorInfo { rect: mi.monitorInfo.rcMonitor, dpi: dx, device: mi.szDevice })
    }
}

/// Refresh rate (Hz) of the monitor hosting `hwnd`, or 0 if unknown.
pub fn refresh_rate(hwnd: HWND) -> u32 {
    let Some(m) = monitor_of(hwnd) else { return 0 };
    unsafe {
        let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        if EnumDisplaySettingsW(PCWSTR(m.device.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool() {
            dm.dmDisplayFrequency
        } else {
            0
        }
    }
}

/// True when `hwnd` covers its whole monitor (borderless or exclusive fullscreen).
pub fn is_fullscreen(hwnd: HWND) -> bool {
    if hwnd.is_invalid() {
        return false;
    }
    let mut r = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut r) }.is_err() {
        return false;
    }
    let Some(m) = monitor_of(hwnd) else { return false };
    let m = m.rect;
    r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
}

// ───────────────────────── power ─────────────────────────

pub fn on_battery() -> bool {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).is_ok() && s.ACLineStatus == 0 }
}

/// Below-normal priority plus EcoQoS ("Efficiency mode"): games win every scheduling decision
/// and our few wake-ups run on efficiency cores at low clocks.
pub fn apply_power_saver() {
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        };
        let _ = SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

/// Returns idle pages to Windows (e.g. after the settings window closes).
pub fn trim_working_set() {
    unsafe {
        let _ = K32EmptyWorkingSet(GetCurrentProcess());
    }
}

/// Dark context menus (Windows 10 1903+), via uxtheme's unnamed `SetPreferredAppMode` /
/// `FlushMenuThemes` exports. Silently does nothing where they don't exist.
pub fn enable_dark_menus() {
    unsafe {
        let Ok(ux) = LoadLibraryExW(w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) else { return };
        // Ordinal 135: SetPreferredAppMode(PreferredAppMode) — 2 = ForceDark.
        if let Some(f) = GetProcAddress(ux, PCSTR(135usize as *const u8)) {
            let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set(2);
        }
        // Ordinal 136: FlushMenuThemes().
        if let Some(f) = GetProcAddress(ux, PCSTR(136usize as *const u8)) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

// ───────────────────────── start with Windows ─────────────────────────

/// Because the app needs elevation, a Run-key entry would be blocked by UAC at sign-in;
/// a scheduled task with highest privileges launches silently instead.
const TASK_NAME: &str = "FPS Viewer";

pub fn startup_is_enabled() -> bool {
    run_schtasks(&["/Query", "/TN", TASK_NAME]) == Some(0)
}

pub fn startup_set_enabled(enabled: bool) -> bool {
    if !enabled {
        return run_schtasks(&["/Delete", "/F", "/TN", TASK_NAME]) == Some(0) || !startup_is_enabled();
    }

    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(d), Ok(u)) => format!("{d}\\{u}"),
        (_, Ok(u)) => u,
        _ => String::new(),
    };
    let (user, exe) = (xml_escape(&user), xml_escape(&exe));
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Starts the FPS Viewer overlay when you sign in.</Description></RegistrationInfo>
  <Triggers>
    <LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId><Delay>PT10S</Delay></LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec><Command>{exe}</Command><Arguments>--background</Arguments></Exec>
  </Actions>
</Task>
"#
    );

    let tmp = std::env::temp_dir().join(format!("fpsviewer-task-{}.xml", std::process::id()));
    let mut bytes = Vec::with_capacity(xml.len() * 2 + 2);
    bytes.extend_from_slice(&[0xFF, 0xFE]); // UTF-16 LE BOM
    for u in xml.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    if let Err(e) = std::fs::write(&tmp, bytes) {
        log(&format!("Could not write the startup task file: {e}"));
        return false;
    }
    let tmp_str = tmp.display().to_string();
    let ok = run_schtasks(&["/Create", "/F", "/TN", TASK_NAME, "/XML", &tmp_str]) == Some(0);
    let _ = std::fs::remove_file(&tmp);
    ok
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Runs `schtasks.exe` hidden, waiting at most 5 seconds. Returns its exit code.
fn run_schtasks(args: &[&str]) -> Option<i32> {
    let mut child = match Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log(&format!("schtasks failed: {e}"));
            return None;
        }
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
}

// ───────────────────────── CPU & memory load ─────────────────────────

/// Whole-PC load (all apps, not just the game).
#[derive(Clone, Copy)]
pub struct Load {
    /// CPU busy %, averaged over all cores since the previous sample; negative = unknown.
    pub cpu: f64,
    /// Physical memory in use, GB (1024³ bytes, as Task Manager shows it).
    pub ram_used_gb: f64,
    /// Physical memory installed (usable by Windows), GB; negative = unknown.
    pub ram_total_gb: f64,
}

impl Load {
    pub const UNKNOWN: Load = Load { cpu: -1.0, ram_used_gb: -1.0, ram_total_gb: -1.0 };
}

/// Samples [`Load`] — two cheap system calls, no allocations.
pub struct LoadSampler {
    idle: u64,
    total: u64,
    last: Load,
}

impl LoadSampler {
    pub fn new() -> Self {
        let mut s = LoadSampler { idle: 0, total: 0, last: Load::UNKNOWN };
        s.sample(); // primes the CPU counters; the first real reading comes next tick
        s
    }

    pub fn sample(&mut self) -> Load {
        let ticks = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_ok() {
            // Kernel time includes idle time.
            let (i, t) = (ticks(idle), ticks(kernel) + ticks(user));
            let (di, dt) = (i.wrapping_sub(self.idle), t.wrapping_sub(self.total));
            if self.total != 0 && dt > 0 && di <= dt {
                self.last.cpu = 100.0 * (1.0 - di as f64 / dt as f64);
            }
            self.idle = i;
            self.total = t;
        }

        let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        if unsafe { GlobalMemoryStatusEx(&mut m) }.is_ok() {
            const GB: f64 = (1u64 << 30) as f64;
            self.last.ram_used_gb = m.ullTotalPhys.saturating_sub(m.ullAvailPhys) as f64 / GB;
            self.last.ram_total_gb = m.ullTotalPhys as f64 / GB;
        }
        self.last
    }
}
