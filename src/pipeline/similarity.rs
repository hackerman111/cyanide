use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gstreamer as gst;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::*;

pub const GRID_W: usize = 32;
pub const GRID_H: usize = 18;
pub const GRID_CELLS: usize = GRID_W * GRID_H;
const SAMPLE_INTERVAL_MS: u64 = 200;
const EMA_ALPHA: f32 = 0.35;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameSide {
    Loop,
    Live,
}

#[derive(Default)]
struct MatcherState {
    loop_grid: Option<[u8; GRID_CELLS]>,
    live_grid: Option<[u8; GRID_CELLS]>,
    smoothed: f32,
}

/// Computes brightness-invariant similarity (ZNCC) between downsampled Loop and Live frames.
pub struct FrameMatcher {
    state: Mutex<MatcherState>,
    enabled: AtomicBool,
    frozen: AtomicBool,
    last_loop_ms: AtomicU64,
    last_live_ms: AtomicU64,
    score_bits: AtomicU32,
}

impl Default for FrameMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameMatcher {
    pub fn new() -> Self {
        Self::with_enabled(false)
    }

    pub fn with_enabled(enabled: bool) -> Self {
        Self {
            state: Mutex::new(MatcherState::default()),
            enabled: AtomicBool::new(enabled),
            frozen: AtomicBool::new(false),
            last_loop_ms: AtomicU64::new(0),
            last_live_ms: AtomicU64::new(0),
            score_bits: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        if !enabled {
            self.last_loop_ms.store(0, Ordering::Relaxed);
            self.last_live_ms.store(0, Ordering::Relaxed);
            self.reset();
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub fn set_frozen(&self, frozen: bool) {
        self.frozen.store(frozen, Ordering::Release);
    }

    pub fn score(&self) -> f32 {
        f32::from_bits(self.score_bits.load(Ordering::Relaxed))
    }

    pub fn reset(&self) {
        if let Ok(mut st) = self.state.lock() {
            st.loop_grid = None;
            st.live_grid = None;
            st.smoothed = 0.0;
        }
        self.score_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
    }

    /// Samples a GStreamer video buffer at most once per `SAMPLE_INTERVAL_MS` per side.
    pub fn on_buffer(&self, side: FrameSide, info: &gst_video::VideoInfo, buf: &gst::BufferRef) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        if side == FrameSide::Loop && self.frozen.load(Ordering::Acquire) {
            return;
        }
        let now = now_ms();
        let last = match side {
            FrameSide::Loop => &self.last_loop_ms,
            FrameSide::Live => &self.last_live_ms,
        };
        let prev = last.load(Ordering::Relaxed);
        if now.saturating_sub(prev) < SAMPLE_INTERVAL_MS {
            return;
        }
        if last
            .compare_exchange(prev, now, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let Ok(frame) = gst_video::VideoFrameRef::from_buffer_ref_readable(buf, info) else {
            return;
        };
        let mut grid = [0u8; GRID_CELLS];
        if !extract_luma_grid(&frame, &mut grid) {
            return;
        }
        self.push_grid(side, grid);
    }

    pub fn push_grid(&self, side: FrameSide, grid: [u8; GRID_CELLS]) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        if side == FrameSide::Loop && self.frozen.load(Ordering::Acquire) {
            return;
        }
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        match side {
            FrameSide::Loop => st.loop_grid = Some(grid),
            FrameSide::Live => st.live_grid = Some(grid),
        }
        let Some(ref a) = st.loop_grid else {
            return;
        };
        let Some(ref b) = st.live_grid else {
            return;
        };
        let raw = zncc_similarity(a, b);
        st.smoothed = if st.smoothed == 0.0 {
            raw
        } else {
            st.smoothed * (1.0 - EMA_ALPHA) + raw * EMA_ALPHA
        };
        self.score_bits
            .store(st.smoothed.to_bits(), Ordering::Relaxed);
    }
}

/// Zero-Mean Normalized Cross-Correlation mapped to `[0.0, 1.0]`.
/// Flat/black frames return `0.0` so fallback black screens never falsely match.
pub fn zncc_similarity(a: &[u8; GRID_CELLS], b: &[u8; GRID_CELLS]) -> f32 {
    let n = GRID_CELLS as f64;
    let sum_a: f64 = a.iter().map(|&v| v as f64).sum();
    let sum_b: f64 = b.iter().map(|&v| v as f64).sum();
    let mean_a = sum_a / n;
    let mean_b = sum_b / n;

    let mut cov = 0.0f64;
    let mut var_a = 0.0f64;
    let mut var_b = 0.0f64;
    for (&va, &vb) in a.iter().zip(b.iter()) {
        let da = va as f64 - mean_a;
        let db = vb as f64 - mean_b;
        cov += da * db;
        var_a += da * da;
        var_b += db * db;
    }

    // Require minimum variance (~2 luma levels std-dev) so uniform/black frames score 0.
    let min_var = 4.0 * n;
    if var_a < min_var || var_b < min_var {
        return 0.0;
    }
    let denom = (var_a * var_b).sqrt();
    if denom <= f64::EPSILON {
        return 0.0;
    }
    (cov / denom).clamp(0.0, 1.0) as f32
}

fn extract_luma_grid(
    frame: &gst_video::VideoFrameRef<&gst::BufferRef>,
    out: &mut [u8; GRID_CELLS],
) -> bool {
    let w = frame.width() as usize;
    let h = frame.height() as usize;
    if w < 2 || h < 2 {
        return false;
    }
    let stride = frame.plane_stride().first().copied().unwrap_or(0);
    if stride <= 0 {
        return false;
    }
    let stride = stride as usize;
    let Ok(data) = frame.plane_data(0) else {
        return false;
    };

    use gst_video::VideoFormat;
    match frame.format() {
        VideoFormat::I420
        | VideoFormat::Yv12
        | VideoFormat::Nv12
        | VideoFormat::Nv21
        | VideoFormat::Y42b
        | VideoFormat::Y444
        | VideoFormat::Gray8 => sample_grid(w, h, stride, data, out, |row, x| {
            row.get(x).copied().unwrap_or(0)
        }),
        VideoFormat::Yuy2 | VideoFormat::Yvyu => sample_grid(w, h, stride, data, out, |row, x| {
            row.get(x * 2).copied().unwrap_or(0)
        }),
        VideoFormat::Uyvy => sample_grid(w, h, stride, data, out, |row, x| {
            row.get(x * 2 + 1).copied().unwrap_or(0)
        }),
        VideoFormat::Rgba | VideoFormat::Rgbx => sample_grid(w, h, stride, data, out, |row, x| {
            let i = x * 4;
            if i + 2 >= row.len() {
                return 0;
            }
            rgb_luma(row[i], row[i + 1], row[i + 2])
        }),
        VideoFormat::Bgra | VideoFormat::Bgrx => sample_grid(w, h, stride, data, out, |row, x| {
            let i = x * 4;
            if i + 2 >= row.len() {
                return 0;
            }
            rgb_luma(row[i + 2], row[i + 1], row[i])
        }),
        VideoFormat::Rgb => sample_grid(w, h, stride, data, out, |row, x| {
            let i = x * 3;
            if i + 2 >= row.len() {
                return 0;
            }
            rgb_luma(row[i], row[i + 1], row[i + 2])
        }),
        VideoFormat::Bgr => sample_grid(w, h, stride, data, out, |row, x| {
            let i = x * 3;
            if i + 2 >= row.len() {
                return 0;
            }
            rgb_luma(row[i + 2], row[i + 1], row[i])
        }),
        _ => return false,
    }
    true
}

fn sample_grid(
    w: usize,
    h: usize,
    stride: usize,
    data: &[u8],
    out: &mut [u8; GRID_CELLS],
    luma_at: impl Fn(&[u8], usize) -> u8,
) {
    // 2x2 neighborhood average per cell reduces webcam sensor noise cheaply.
    for gy in 0..GRID_H {
        let y0 = ((gy * 2 + 1) * h) / (GRID_H * 2);
        let y1 = (y0 + 1).min(h - 1);
        let row0_start = y0 * stride;
        let row1_start = y1 * stride;
        let Some(row0) = data.get(row0_start..row0_start.saturating_add(stride).min(data.len()))
        else {
            continue;
        };
        let Some(row1) = data.get(row1_start..row1_start.saturating_add(stride).min(data.len()))
        else {
            continue;
        };
        let dst_row = gy * GRID_W;
        for gx in 0..GRID_W {
            let x0 = ((gx * 2 + 1) * w) / (GRID_W * 2);
            let x1 = (x0 + 1).min(w - 1);
            let sum = luma_at(row0, x0) as u16
                + luma_at(row0, x1) as u16
                + luma_at(row1, x0) as u16
                + luma_at(row1, x1) as u16;
            out[dst_row + gx] = (sum >> 2) as u8;
        }
    }
}

#[inline]
fn rgb_luma(r: u8, g: u8, b: u8) -> u8 {
    ((r as u16 + ((g as u16) << 1) + b as u16) >> 2) as u8
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_portrait(shift_x: isize, brightness_offset: i16) -> [u8; GRID_CELLS] {
        let mut grid = [0u8; GRID_CELLS];
        for y in 0..GRID_H {
            for x in 0..GRID_W {
                let cx = x as isize - (GRID_W as isize / 2 + shift_x);
                let cy = y as isize - (GRID_H as isize / 2);
                let dist2 = cx * cx + cy * cy * 2;
                let base = if dist2 < 28 { 190 } else { 40 + (y as i16 * 3) };
                grid[y * GRID_W + x] = (base + brightness_offset).clamp(0, 255) as u8;
            }
        }
        grid
    }

    #[test]
    fn zncc_matches_exposure_shifted_and_rejects_flat_or_shifted() {
        let reference = synthetic_portrait(0, 0);
        let brighter = synthetic_portrait(0, 35);
        let shifted = synthetic_portrait(9, 0);
        let black = [0u8; GRID_CELLS];

        assert!(zncc_similarity(&reference, &reference) > 0.999);
        assert!(zncc_similarity(&reference, &brighter) > 0.99);
        assert!(zncc_similarity(&reference, &shifted) < 0.45);
        assert_eq!(zncc_similarity(&black, &black), 0.0);
        assert_eq!(zncc_similarity(&reference, &black), 0.0);
    }

    #[test]
    fn frame_matcher_respects_frozen_reference() {
        let matcher = FrameMatcher::with_enabled(true);
        let a = synthetic_portrait(0, 0);
        let b = synthetic_portrait(10, 0);

        matcher.push_grid(FrameSide::Loop, a);
        matcher.push_grid(FrameSide::Live, a);
        assert!(matcher.score() > 0.95);

        matcher.set_frozen(true);
        // Updating loop while frozen must be ignored so live still matches frozen `a`.
        matcher.push_grid(FrameSide::Loop, b);
        matcher.push_grid(FrameSide::Live, a);
        assert!(matcher.score() > 0.95);
    }

    #[test]
    fn frame_matcher_disabled_by_default_and_skips_updates() {
        let matcher = FrameMatcher::new();
        assert!(!matcher.is_enabled());
        let a = synthetic_portrait(0, 0);

        matcher.push_grid(FrameSide::Loop, a);
        matcher.push_grid(FrameSide::Live, a);
        assert_eq!(matcher.score(), 0.0);

        matcher.set_enabled(true);
        assert!(matcher.is_enabled());
        matcher.push_grid(FrameSide::Loop, a);
        matcher.push_grid(FrameSide::Live, a);
        assert!(matcher.score() > 0.95);

        matcher.set_enabled(false);
        assert!(!matcher.is_enabled());
        assert_eq!(matcher.score(), 0.0);
        matcher.push_grid(FrameSide::Loop, a);
        matcher.push_grid(FrameSide::Live, a);
        assert_eq!(matcher.score(), 0.0);
    }
}
