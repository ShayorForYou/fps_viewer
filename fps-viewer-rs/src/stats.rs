//! Turns raw present timestamps into human-friendly numbers. Runs on the UI thread a few
//! times per second with buffers allocated once at startup — zero allocations per update.

use crate::etw::{self, PresentSample, CAPACITY};
use crate::util::{qpc_freq, qpc_now, Rng};

pub const GRAPH_POINTS: usize = 90;
pub const GRAPH_SECONDS: f64 = 4.0;
const HISTORY_SECONDS: f64 = 10.0;

#[derive(Clone, Copy)]
pub struct FrameStats {
    pub active: bool,
    pub fps: f64,
    /// Vulkan / OpenGL fallback path.
    pub from_kernel: bool,
    /// Frame times (ms), oldest → newest; only the first `graph_len` entries are valid.
    pub graph: [f32; GRAPH_POINTS],
    pub graph_len: usize,
}

impl FrameStats {
    pub const INACTIVE: FrameStats = FrameStats {
        active: false,
        fps: 0.0,
        from_kernel: false,
        graph: [0.0; GRAPH_POINTS],
        graph_len: 0,
    };
}

pub struct Engine {
    raw: Box<[PresentSample]>,
    ts: Box<[i64]>,
    /// (swap chain, count) — apps rarely own more than a handful.
    per_swap_chain: Vec<(u64, u32)>,
}

impl Engine {
    pub fn new() -> Self {
        Engine {
            raw: vec![PresentSample::default(); CAPACITY].into_boxed_slice(),
            ts: vec![0; CAPACITY].into_boxed_slice(),
            per_swap_chain: Vec::with_capacity(16),
        }
    }

    pub fn compute(&mut self, want_graph: bool) -> FrameStats {
        let now = qpc_now();
        let freq = qpc_freq();
        let since = now - (HISTORY_SECONDS * freq as f64) as i64;
        let (n, from_kernel) = etw::copy_since(since, now, freq, &mut self.raw);
        if n < 2 {
            return FrameStats::INACTIVE;
        }

        // Some apps own several swap chains (launchers, in-game overlays). Track the busiest one.
        self.per_swap_chain.clear();
        let (mut best, mut best_count) = (0u64, 0u32);
        for s in &self.raw[..n] {
            let c = match self.per_swap_chain.iter_mut().find(|(sc, _)| *sc == s.swap_chain) {
                Some(e) => {
                    e.1 += 1;
                    e.1
                }
                None => {
                    if self.per_swap_chain.len() < self.per_swap_chain.capacity() {
                        self.per_swap_chain.push((s.swap_chain, 1));
                    }
                    1
                }
            };
            if c > best_count {
                best_count = c;
                best = s.swap_chain;
            }
        }

        let mut m = 0;
        for s in &self.raw[..n] {
            if s.swap_chain == best {
                self.ts[m] = s.timestamp;
                m += 1;
            }
        }
        if m < 2 {
            return FrameStats::INACTIVE;
        }

        let ts = &mut self.ts[..m];
        ts.sort_unstable(); // ETW real-time delivery is only ordered per CPU buffer

        let newest = ts[m - 1];
        if now - newest > 2 * freq {
            return FrameStats::INACTIVE; // the app stopped rendering
        }

        let mut s = FrameStats { active: true, from_kernel, ..FrameStats::INACTIVE };

        // FPS: frames presented in the last second (ending at the newest present).
        let window_start = newest - freq;
        let mut i0 = m - 1;
        while i0 > 0 && ts[i0 - 1] >= window_start {
            i0 -= 1;
        }
        let frames = (m - 1 - i0) as f64;
        let span = (newest - ts[i0]) as f64 / freq as f64;
        s.fps = if frames > 0.0 && span > 0.0 {
            frames / span
        } else {
            freq as f64 / (newest - ts[m - 2]).max(1) as f64
        };

        let to_ms = 1000.0 / freq as f64;
        if want_graph && m > 2 {
            build_graph(ts, newest, freq, to_ms, &mut s.graph);
            s.graph_len = GRAPH_POINTS;
        }
        s
    }
}

/// Buckets the last few seconds of frame times, keeping each bucket's worst frame so
/// stutters stay visible.
fn build_graph(ts: &[i64], newest: i64, freq: i64, to_ms: f64, graph: &mut [f32; GRAPH_POINTS]) {
    graph.fill(0.0);
    let start = newest - (GRAPH_SECONDS * freq as f64) as i64;
    let bucket_ticks = GRAPH_SECONDS * freq as f64 / GRAPH_POINTS as f64;

    for i in 1..ts.len() {
        if ts[i] < start {
            continue;
        }
        let dt = (ts[i] - ts[i - 1]) as f64 * to_ms;
        if dt <= 0.0 || dt >= 1000.0 {
            continue;
        }
        let b = (((ts[i] - start) as f64 / bucket_ticks) as usize).min(GRAPH_POINTS - 1);
        if dt as f32 > graph[b] {
            graph[b] = dt as f32;
        }
    }

    // Fill empty buckets (not enough history yet) with their nearest neighbour.
    let mut last = graph.iter().copied().find(|&v| v > 0.0).unwrap_or(0.0);
    for v in graph.iter_mut() {
        if *v <= 0.0 {
            *v = last;
        } else {
            last = *v;
        }
    }
}

/// Plausible fake numbers so the settings preview looks alive when no game is running.
pub struct DemoFeed {
    rng: Rng,
    history: [f32; GRAPH_POINTS],
    phase: f64,
}

impl DemoFeed {
    pub fn new() -> Self {
        DemoFeed { rng: Rng::new(7), history: [6.9; GRAPH_POINTS], phase: 0.0 }
    }

    pub fn next(&mut self, target_fps: f64) -> FrameStats {
        let target = if target_fps > 0.0 { target_fps } else { 144.0 };
        self.phase += 0.35;
        let fps = target * (0.93 + 0.05 * self.phase.sin()) + self.rng.next_f64() * 3.0;

        self.history.copy_within(3.., 0);
        for i in GRAPH_POINTS - 3..GRAPH_POINTS {
            let mut ft = 1000.0 / fps * (0.92 + self.rng.next_f64() * 0.16);
            if self.rng.next_f64() < 0.025 {
                ft *= 2.2; // the occasional stutter
            }
            self.history[i] = ft as f32;
        }

        FrameStats {
            active: true,
            fps,
            from_kernel: false,
            graph: self.history,
            graph_len: GRAPH_POINTS,
        }
    }
}
