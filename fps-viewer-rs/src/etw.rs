//! Listens to the same ETW "present" events PresentMon uses, in-process and filtered in the
//! kernel to just the handful of event IDs we care about:
//!   • Microsoft-Windows-DXGI     Present_Start / PresentMPO_Start  (D3D10/11/12)
//!   • Microsoft-Windows-D3D9     Present_Start                     (D3D9)
//!   • Microsoft-Windows-DxgKrnl  Present_Info                      (fallback: Vulkan / OpenGL)
//! Only events from the focused process are recorded; everything else is rejected with one
//! integer compare in an allocation-free callback.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use windows::core::{GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Diagnostics::Etw::*;
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};

use crate::util::wide;

const SESSION_NAME: &str = "FpsViewer-Presents";

const DXGI_PROVIDER: GUID = GUID::from_u128(0xCA11C036_0102_4A2D_A6AD_F03CFED5D3C9);
const D3D9_PROVIDER: GUID = GUID::from_u128(0x783ACA0A_790E_4D7F_8451_AA850511C6B9);
const DXGKRNL_PROVIDER: GUID = GUID::from_u128(0x802EC45A_1E99_4B83_9920_87C98277BA9D);

const DXGI_PRESENT_START: u16 = 42;
const DXGI_PRESENT_MPO_START: u16 = 55;
const D3D9_PRESENT_START: u16 = 1;
const DXGKRNL_PRESENT_INFO: u16 = 184;

const WNODE_FLAG_TRACED_GUID: u32 = 0x0002_0000;
const EVENT_TRACE_REAL_TIME_MODE: u32 = 0x0000_0100;
const PROCESS_TRACE_MODE_REAL_TIME: u32 = 0x0000_0100;
const PROCESS_TRACE_MODE_RAW_TIMESTAMP: u32 = 0x0000_1000;
const PROCESS_TRACE_MODE_EVENT_RECORD: u32 = 0x1000_0000;
const EVENT_CONTROL_CODE_ENABLE_PROVIDER: u32 = 1;
const EVENT_FILTER_TYPE_EVENT_ID: u32 = 0x8000_0200;
const ENABLE_TRACE_PARAMETERS_VERSION_2: u32 = 2;
const TRACE_LEVEL_VERBOSE: u8 = 5;
const EVENT_HEADER_FLAG_32_BIT_HEADER: u16 = 0x0020;
const INVALID_PROCESSTRACE_HANDLE: u64 = u64::MAX;

/// ~10 s of history at 800 fps.
pub const CAPACITY: usize = 8192;

#[derive(Clone, Copy, Default)]
pub struct PresentSample {
    /// QPC ticks.
    pub timestamp: i64,
    /// Swap-chain identity (0 when unknown).
    pub swap_chain: u64,
}

struct Ring {
    buf: Box<[PresentSample]>,
    head: usize,
    count: usize,
}

impl Ring {
    fn new() -> Self {
        Ring { buf: vec![PresentSample::default(); CAPACITY].into_boxed_slice(), head: 0, count: 0 }
    }

    #[inline(always)]
    fn push(&mut self, s: PresentSample) {
        self.buf[self.head] = s;
        self.head = (self.head + 1) % CAPACITY;
        if self.count < CAPACITY {
            self.count += 1;
        }
    }

    fn clear(&mut self) {
        self.head = 0;
        self.count = 0;
    }

    fn copy_since(&self, since: i64, dest: &mut [PresentSample]) -> usize {
        let mut n = 0;
        let start = (self.head + CAPACITY - self.count) % CAPACITY;
        for i in 0..self.count {
            if n == dest.len() {
                break;
            }
            let s = self.buf[(start + i) % CAPACITY];
            if s.timestamp >= since {
                dest[n] = s;
                n += 1;
            }
        }
        n
    }
}

struct Rings {
    runtime: Ring,
    kernel: Ring,
    last_runtime_ts: i64,
}

static RINGS: Mutex<Option<Rings>> = Mutex::new(None);
/// PID whose presents we record; `u32::MAX` = nothing (never matches a real process).
static TARGET_PID: AtomicU32 = AtomicU32::new(u32::MAX);
static RUNNING: AtomicBool = AtomicBool::new(false);

struct Session {
    trace: u64,
    thread: Option<std::thread::JoinHandle<()>>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

pub fn is_running() -> bool {
    RUNNING.load(Ordering::Acquire)
}

/// Starts the real-time trace session. Requires administrator rights.
pub fn start(include_kernel_fallback: bool) -> Result<(), String> {
    if is_running() {
        return Ok(());
    }
    {
        let mut r = RINGS.lock().unwrap();
        if r.is_none() {
            *r = Some(Rings { runtime: Ring::new(), kernel: Ring::new(), last_runtime_ts: 0 });
        }
    }

    let name = wide(SESSION_NAME);
    let mut session = CONTROLTRACE_HANDLE::default();
    let mut rc = start_session(&name, &mut session);
    if rc == ERROR_ALREADY_EXISTS {
        // A previous instance crashed and left the session behind — reclaim it.
        stop_session_by_name(&name);
        rc = start_session(&name, &mut session);
    }
    if rc != ERROR_SUCCESS {
        return Err(if rc == ERROR_ACCESS_DENIED {
            "Administrator rights are required to read frame timings.".into()
        } else {
            format!("Could not start the frame-timing session (error {}).", rc.0)
        });
    }

    let mut any = enable_provider(session, &DXGI_PROVIDER, u64::MAX, &[DXGI_PRESENT_START, DXGI_PRESENT_MPO_START]);
    any |= enable_provider(session, &D3D9_PROVIDER, u64::MAX, &[D3D9_PRESENT_START]);
    if include_kernel_fallback {
        // Base | Microsoft-Windows-DxgKrnl/Performance — the minimum needed for Present_Info.
        enable_provider(session, &DXGKRNL_PROVIDER, 0x8000_0000_0000_0001, &[DXGKRNL_PRESENT_INFO]);
    }

    if !any {
        stop_session_by_name(&name);
        return Err("Graphics event providers are unavailable on this system.".into());
    }

    let trace = open_consumer(&name);
    if trace == INVALID_PROCESSTRACE_HANDLE {
        let err = windows::core::Error::from_thread();
        stop_session_by_name(&name);
        return Err(format!("Could not open the frame-timing consumer (error {:#x}).", err.code().0));
    }

    let thread = std::thread::Builder::new()
        .name("ETW present consumer".into())
        .stack_size(64 * 1024)
        .spawn(move || unsafe {
            // Relative to our below-normal process: keeps up with event bursts under load.
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
            let handles = [PROCESSTRACE_HANDLE { Value: trace }];
            let _ = ProcessTrace(&handles, None, None); // blocks until the session stops
        })
        .ok();

    *SESSION.lock().unwrap() = Some(Session { trace, thread });
    RUNNING.store(true, Ordering::Release);
    Ok(())
}

pub fn stop() {
    let Some(mut s) = SESSION.lock().unwrap().take() else { return };
    RUNNING.store(false, Ordering::Release);
    stop_session_by_name(&wide(SESSION_NAME));
    unsafe {
        let _ = CloseTrace(PROCESSTRACE_HANDLE { Value: s.trace });
    }
    if let Some(t) = s.thread.take() {
        let _ = t.join(); // ProcessTrace returns promptly once the session is stopped
    }
}

/// Switches which process we record presents for and clears history.
pub fn set_target(pid: u32) {
    let mut guard = RINGS.lock().unwrap();
    TARGET_PID.store(if pid > 0 { pid } else { u32::MAX }, Ordering::Release);
    if let Some(r) = guard.as_mut() {
        r.runtime.clear();
        r.kernel.clear();
        r.last_runtime_ts = 0;
    }
}

/// Copies samples newer than `since` for the current target into `dest`. Prefers runtime
/// (DXGI/D3D9) events; falls back to kernel events only when the app has produced no runtime
/// presents for 2 seconds. Returns `(count, from_kernel)`.
pub fn copy_since(since: i64, now: i64, freq: i64, dest: &mut [PresentSample]) -> (usize, bool) {
    let guard = RINGS.lock().unwrap();
    let Some(r) = guard.as_ref() else { return (0, false) };
    let runtime_fresh = r.runtime.count > 0 && now - r.last_runtime_ts < 2 * freq;
    let from_kernel = !runtime_fresh && r.kernel.count > 0;
    let n = if from_kernel { r.kernel.copy_since(since, dest) } else { r.runtime.copy_since(since, dest) };
    (n, from_kernel)
}

// ───────────────────────── session plumbing ─────────────────────────

/// EVENT_TRACE_PROPERTIES followed by room for the logger name, 8-byte aligned.
fn alloc_props() -> Vec<u64> {
    let size = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() + 1024 * 2;
    let mut buf = vec![0u64; size.div_ceil(8)];
    let p = buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
    unsafe {
        (*p).Wnode.BufferSize = size as u32;
        (*p).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        (*p).Wnode.ClientContext = 1; // QPC timestamps — same clock as QueryPerformanceCounter
        (*p).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        (*p).BufferSize = 4; // KB — small buffers = timely delivery, tiny footprint
        (*p).MinimumBuffers = 4;
        (*p).MaximumBuffers = 64;
        (*p).FlushTimer = 1; // seconds
        (*p).LoggerNameOffset = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
    }
    buf
}

fn start_session(name: &[u16], handle: &mut CONTROLTRACE_HANDLE) -> WIN32_ERROR {
    let mut props = alloc_props();
    unsafe { StartTraceW(handle, PCWSTR(name.as_ptr()), props.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES) }
}

fn stop_session_by_name(name: &[u16]) {
    let mut props = alloc_props();
    unsafe {
        let _ = ControlTraceW(
            CONTROLTRACE_HANDLE::default(),
            PCWSTR(name.as_ptr()),
            props.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES,
            EVENT_TRACE_CONTROL_STOP,
        );
    }
}

fn enable_provider(session: CONTROLTRACE_HANDLE, provider: &GUID, keywords: u64, event_ids: &[u16]) -> bool {
    // EVENT_FILTER_EVENT_ID { BOOLEAN FilterIn; UCHAR Reserved; USHORT Count; USHORT Events[]; }
    let mut filter = [0u16; 8];
    filter[0] = 1; // FilterIn = TRUE, Reserved = 0
    filter[1] = event_ids.len() as u16;
    filter[2..2 + event_ids.len()].copy_from_slice(event_ids);
    let mut desc = EVENT_FILTER_DESCRIPTOR {
        Ptr: filter.as_ptr() as u64,
        Size: (4 + 2 * event_ids.len()) as u32,
        Type: EVENT_FILTER_TYPE_EVENT_ID,
    };
    let params = ENABLE_TRACE_PARAMETERS {
        Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
        EnableFilterDesc: &mut desc,
        FilterDescCount: 1,
        ..Default::default()
    };
    let rc = unsafe {
        EnableTraceEx2(
            session,
            provider,
            EVENT_CONTROL_CODE_ENABLE_PROVIDER,
            TRACE_LEVEL_VERBOSE,
            keywords,
            0,
            0,
            Some(&params),
        )
    };
    rc == ERROR_SUCCESS
}

fn open_consumer(name: &[u16]) -> u64 {
    let mut name = name.to_vec();
    let mut logfile = EVENT_TRACE_LOGFILEW::default();
    logfile.LoggerName = PWSTR(name.as_mut_ptr());
    // RAW_TIMESTAMP keeps event times in QPC ticks (otherwise ETW converts them to FILETIME,
    // which can't be compared with QueryPerformanceCounter).
    logfile.Anonymous1.ProcessTraceMode =
        PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
    logfile.Anonymous2.EventRecordCallback = Some(on_event);
    unsafe { OpenTraceW(&mut logfile).Value }
}

// ───────────────────────── hot path ─────────────────────────

unsafe extern "system" fn on_event(e: *mut EVENT_RECORD) {
    // SAFETY: ETW passes a valid record for the duration of the callback.
    let e = unsafe { &*e };
    let h = &e.EventHeader;
    if h.ProcessId != TARGET_PID.load(Ordering::Relaxed) {
        return;
    }

    let id = h.EventDescriptor.Id;
    let is_runtime = ((id == DXGI_PRESENT_START || id == DXGI_PRESENT_MPO_START) && h.ProviderId == DXGI_PROVIDER)
        || (id == D3D9_PRESENT_START && h.ProviderId == D3D9_PROVIDER);
    let is_kernel = !is_runtime && id == DXGKRNL_PRESENT_INFO && h.ProviderId == DXGKRNL_PROVIDER;
    if !is_runtime && !is_kernel {
        return;
    }

    let mut swap_chain = 0u64;
    if is_runtime && !e.UserData.is_null() {
        // First payload field of both events is the swap-chain pointer.
        // SAFETY: lengths are checked; reads are unaligned-safe.
        unsafe {
            if h.Flags & EVENT_HEADER_FLAG_32_BIT_HEADER == 0 {
                if e.UserDataLength >= 8 {
                    swap_chain = (e.UserData as *const u64).read_unaligned();
                }
            } else if e.UserDataLength >= 4 {
                swap_chain = (e.UserData as *const u32).read_unaligned() as u64;
            }
        }
    }

    let sample = PresentSample { timestamp: h.TimeStamp, swap_chain };
    let mut guard = RINGS.lock().unwrap();
    if let Some(r) = guard.as_mut() {
        if is_runtime {
            r.runtime.push(sample);
            if sample.timestamp > r.last_runtime_ts {
                r.last_runtime_ts = sample.timestamp;
            }
        } else {
            r.kernel.push(sample);
        }
    }
}
