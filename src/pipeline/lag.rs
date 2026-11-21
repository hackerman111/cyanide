use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gstreamer as gst;

use crate::config::LagConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LagDecision {
    PassThrough,
    StartFreeze,
    HoldFreeze,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScheduleState {
    Idle,
    Waiting { next_freeze_ms: u64 },
    PendingFreeze { duration_ms: u64 },
    Frozen { until_ms: u64 },
}

/// Deterministic-seedable random freeze scheduler.
pub struct LagSchedule {
    cfg: LagConfig,
    state: ScheduleState,
    rng: u64,
}

impl LagSchedule {
    pub fn new(cfg: LagConfig, seed: u64) -> Self {
        Self {
            cfg: cfg.sanitized(),
            state: ScheduleState::Idle,
            rng: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    pub fn is_active(&self) -> bool {
        self.cfg.enabled
            || matches!(
                self.state,
                ScheduleState::PendingFreeze { .. } | ScheduleState::Frozen { .. }
            )
    }

    pub fn is_frozen(&self) -> bool {
        matches!(self.state, ScheduleState::Frozen { .. })
    }

    /// Arms a pre-switch freeze of random length within `[min_duration_ms, max_duration_ms]`.
    /// Returns the chosen freeze duration in ms if `freeze_on_switch` is enabled.
    pub fn arm_switch_freeze(&mut self, now_ms: u64, already_held: bool) -> Option<u32> {
        if !self.cfg.freeze_on_switch {
            return None;
        }
        let dur = self.next_duration_ms();
        if already_held && matches!(self.state, ScheduleState::Frozen { .. }) {
            self.state = ScheduleState::Frozen {
                until_ms: now_ms.saturating_add(dur),
            };
        } else {
            self.state = ScheduleState::PendingFreeze { duration_ms: dur };
        }
        Some(dur as u32)
    }

    pub fn update_config(&mut self, cfg: LagConfig, now_ms: u64) {
        self.cfg = cfg.sanitized();
        if !self.cfg.enabled {
            match self.state {
                ScheduleState::Idle | ScheduleState::Waiting { .. } => {
                    self.state = ScheduleState::Idle;
                }
                ScheduleState::PendingFreeze { .. } => {
                    if !self.cfg.freeze_on_switch {
                        self.state = ScheduleState::Idle;
                    }
                }
                ScheduleState::Frozen { until_ms } => {
                    if !self.cfg.freeze_on_switch {
                        self.state = ScheduleState::Idle;
                    } else {
                        let max_until = now_ms.saturating_add(self.cfg.max_duration_ms as u64);
                        if until_ms > max_until {
                            self.state = ScheduleState::Frozen {
                                until_ms: max_until,
                            };
                        }
                    }
                }
            }
            return;
        }
        match self.state {
            ScheduleState::Idle => {
                let wait = self.next_interval_ms();
                self.state = ScheduleState::Waiting {
                    next_freeze_ms: now_ms.saturating_add(wait),
                };
            }
            ScheduleState::Waiting { next_freeze_ms } => {
                let max_wait = ((self.cfg.interval_ms as u64) * 16) / 10;
                if next_freeze_ms > now_ms.saturating_add(max_wait) {
                    let wait = self.next_interval_ms();
                    self.state = ScheduleState::Waiting {
                        next_freeze_ms: now_ms.saturating_add(wait),
                    };
                }
            }
            ScheduleState::PendingFreeze { .. } => {}
            ScheduleState::Frozen { until_ms } => {
                let max_until = now_ms.saturating_add(self.cfg.max_duration_ms as u64);
                if until_ms > max_until {
                    self.state = ScheduleState::Frozen {
                        until_ms: max_until,
                    };
                }
            }
        }
    }

    pub fn step(&mut self, now_ms: u64) -> LagDecision {
        match self.state {
            ScheduleState::Idle => {
                if !self.cfg.enabled {
                    return LagDecision::PassThrough;
                }
                let wait = self.next_interval_ms();
                self.state = ScheduleState::Waiting {
                    next_freeze_ms: now_ms.saturating_add(wait),
                };
                LagDecision::PassThrough
            }
            ScheduleState::PendingFreeze { duration_ms } => {
                self.state = ScheduleState::Frozen {
                    until_ms: now_ms.saturating_add(duration_ms),
                };
                LagDecision::StartFreeze
            }
            ScheduleState::Waiting { next_freeze_ms } => {
                if !self.cfg.enabled {
                    self.state = ScheduleState::Idle;
                    return LagDecision::PassThrough;
                }
                if now_ms >= next_freeze_ms {
                    let dur = self.next_duration_ms();
                    self.state = ScheduleState::Frozen {
                        until_ms: now_ms.saturating_add(dur),
                    };
                    LagDecision::StartFreeze
                } else {
                    LagDecision::PassThrough
                }
            }
            ScheduleState::Frozen { until_ms } => {
                if now_ms < until_ms {
                    LagDecision::HoldFreeze
                } else if self.cfg.enabled {
                    let wait = self.next_interval_ms();
                    self.state = ScheduleState::Waiting {
                        next_freeze_ms: now_ms.saturating_add(wait),
                    };
                    LagDecision::PassThrough
                } else {
                    self.state = ScheduleState::Idle;
                    LagDecision::PassThrough
                }
            }
        }
    }

    fn next_interval_ms(&mut self) -> u64 {
        let base = self.cfg.interval_ms as u64;
        let lo = (base * 4 / 10).max(200);
        let hi = (base * 16 / 10).max(lo);
        self.rand_range(lo, hi)
    }

    fn next_duration_ms(&mut self) -> u64 {
        self.rand_range(
            self.cfg.min_duration_ms as u64,
            self.cfg.max_duration_ms as u64,
        )
    }

    fn rand_range(&mut self, lo: u64, hi: u64) -> u64 {
        if lo >= hi {
            return lo;
        }
        let span = hi - lo + 1;
        lo + (self.next_u64() % span)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

type SwitchCallback = Box<dyn FnOnce() + Send>;

struct LagRuntime {
    schedule: LagSchedule,
    held: Option<gst::Buffer>,
    held_caps: Option<gst::Caps>,
    current_caps: Option<gst::Caps>,
    pending_switch: Option<SwitchCallback>,
}

/// Injects configurable random frame freezes into the outgoing video stream
/// (covering both Live camera and recorded Loop MP4) without stalling GStreamer clocks.
pub struct LagController {
    active: AtomicBool,
    started: Instant,
    runtime: Mutex<LagRuntime>,
}

impl LagController {
    pub fn new(cfg: LagConfig) -> Self {
        let clean = cfg.sanitized();
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x517C_C1B7_2722_0A95, |d| d.as_nanos() as u64);
        let schedule = LagSchedule::new(clean, seed);
        let is_active = schedule.is_active();
        Self {
            active: AtomicBool::new(is_active),
            started: Instant::now(),
            runtime: Mutex::new(LagRuntime {
                schedule,
                held: None,
                held_caps: None,
                current_caps: None,
                pending_switch: None,
            }),
        }
    }

    pub fn update_config(&self, cfg: LagConfig) {
        let clean = cfg.sanitized();
        let mut fire_after_lock: Option<SwitchCallback> = None;
        if let Ok(mut rt) = self.runtime.lock() {
            let now_ms = self.started.elapsed().as_millis() as u64;
            rt.schedule.update_config(clean, now_ms);
            let is_active = rt.schedule.is_active();
            self.active.store(is_active, Ordering::Release);
            if !is_active {
                rt.held = None;
                rt.held_caps = None;
                fire_after_lock = rt.pending_switch.take();
            }
        }
        if let Some(cb) = fire_after_lock {
            cb();
        }
    }

    /// Arms a pre-switch freeze on the *current* source before running `on_captured` to switch
    /// to the new source. Returns `Some(freeze_duration_ms)` when armed, or `None` if
    /// `freeze_on_switch` is disabled (caller should switch immediately).
    pub fn arm_switch_freeze(&self, on_captured: impl FnOnce() + Send + 'static) -> Option<u32> {
        let mut fire_now: Option<SwitchCallback> = None;
        let dur_ms = {
            let Ok(mut rt) = self.runtime.lock() else {
                return None;
            };
            let now_ms = self.started.elapsed().as_millis() as u64;
            let already_held = rt.held.is_some();
            let dur = rt.schedule.arm_switch_freeze(now_ms, already_held)?;
            self.active.store(true, Ordering::Release);
            if already_held && rt.schedule.is_frozen() {
                // Already frozen on a frame of the previous source: switch selector underneath
                // while continuing to emit the frozen previous frame until the freeze ends.
                rt.pending_switch = None;
                fire_now = Some(Box::new(on_captured));
            } else {
                // Keep selector on the previous source until `StartFreeze` captures its frame.
                rt.pending_switch = Some(Box::new(on_captured));
            }
            dur
        };
        if let Some(cb) = fire_now {
            cb();
        }
        Some(dur_ms)
    }

    /// Updates tracked stream caps; only drops a cached frozen buffer when caps actually change
    /// so redundant Caps events on identical-caps pad switches preserve the frozen previous frame.
    pub fn on_caps(&self, caps: &gst::CapsRef) {
        if let Ok(mut rt) = self.runtime.lock() {
            let same_as_held = rt
                .held_caps
                .as_ref()
                .is_some_and(|prev| prev.is_equal_fixed(caps));
            rt.current_caps = Some(caps.to_owned());
            if !same_as_held {
                rt.held = None;
                rt.held_caps = None;
            }
        }
    }

    /// Returns `Some(frozen_copy)` to replace the current frame during a freeze, or `None` to pass through.
    pub fn process_buffer(
        &self,
        buf: &gst::BufferRef,
        fallback_caps: impl FnOnce() -> Option<gst::Caps>,
    ) -> Option<gst::Buffer> {
        if !self.active.load(Ordering::Acquire) {
            return None;
        }
        let mut fire_after_lock: Option<SwitchCallback> = None;
        let out = {
            let Ok(mut rt) = self.runtime.lock() else {
                return None;
            };
            let now_ms = self.started.elapsed().as_millis() as u64;
            let decision = rt.schedule.step(now_ms);
            if !rt.schedule.is_active() {
                self.active.store(false, Ordering::Release);
            }
            match decision {
                LagDecision::PassThrough => {
                    rt.held = None;
                    rt.held_caps = None;
                    fire_after_lock = rt.pending_switch.take();
                    None
                }
                LagDecision::StartFreeze => {
                    // Deep-copy once on freeze entry (from the OLD source before switching) so
                    // upstream v4l2src buffer pools are released immediately.
                    rt.held = buf.copy_deep().ok();
                    if rt.current_caps.is_none() {
                        rt.current_caps = fallback_caps();
                    }
                    rt.held_caps = rt.current_caps.clone();
                    fire_after_lock = rt.pending_switch.take();
                    None
                }
                LagDecision::HoldFreeze => {
                    let Some(ref held) = rt.held else {
                        rt.held = buf.copy_deep().ok();
                        if rt.current_caps.is_none() {
                            rt.current_caps = fallback_caps();
                        }
                        rt.held_caps = rt.current_caps.clone();
                        return None;
                    };
                    // Shallow-copy shares the held sysmem pixels while stamping the incoming frame's timestamps.
                    let mut out = held.copy();
                    if let Some(out_ref) = out.get_mut() {
                        out_ref.set_pts(buf.pts());
                        out_ref.set_dts(buf.dts());
                        out_ref.set_duration(buf.duration());
                        out_ref.set_offset(buf.offset());
                        out_ref.set_offset_end(buf.offset_end());
                    }
                    Some(out)
                }
            }
        };
        if let Some(cb) = fire_after_lock {
            cb();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_freezes_within_configured_bounds_and_recovers() {
        let cfg = LagConfig {
            enabled: true,
            freeze_on_switch: false,
            interval_ms: 1000,
            min_duration_ms: 200,
            max_duration_ms: 600,
        };
        let mut sched = LagSchedule::new(cfg, 42);

        let mut starts = 0u32;
        let mut holds = 0u32;
        let mut current_hold = 0u32;
        let mut max_hold_seen = 0u32;

        // Simulate 30 seconds at 50ms ticks.
        for step in 0..600u64 {
            let now = step * 50;
            match sched.step(now) {
                LagDecision::StartFreeze => {
                    starts += 1;
                    current_hold = 50;
                }
                LagDecision::HoldFreeze => {
                    holds += 1;
                    current_hold += 50;
                    max_hold_seen = max_hold_seen.max(current_hold);
                }
                LagDecision::PassThrough => {
                    current_hold = 0;
                }
            }
        }

        assert!(starts >= 10, "expected repeated freezes, got {starts}");
        assert!(holds > 0, "expected frozen frames");
        assert!(
            max_hold_seen <= 650,
            "freeze duration exceeded max_duration_ms: {max_hold_seen}ms"
        );
    }

    #[test]
    fn disabling_lag_immediately_passes_through() {
        let cfg = LagConfig {
            enabled: true,
            freeze_on_switch: false,
            interval_ms: 500,
            min_duration_ms: 1000,
            max_duration_ms: 2000,
        };
        let mut sched = LagSchedule::new(cfg, 7);
        assert_eq!(sched.step(0), LagDecision::PassThrough);
        // Advance until frozen.
        let mut frozen_at = None;
        for t in (50..3000).step_by(50) {
            if sched.step(t) == LagDecision::StartFreeze {
                frozen_at = Some(t);
                break;
            }
        }
        let t0 = frozen_at.expect("must enter freeze");
        assert_eq!(sched.step(t0 + 50), LagDecision::HoldFreeze);

        sched.update_config(
            LagConfig {
                enabled: false,
                ..cfg
            },
            t0 + 100,
        );
        assert_eq!(sched.step(t0 + 150), LagDecision::PassThrough);
    }

    #[test]
    fn switch_freeze_triggers_when_periodic_lag_disabled() {
        let cfg = LagConfig {
            enabled: false,
            freeze_on_switch: true,
            interval_ms: 3000,
            min_duration_ms: 100,
            max_duration_ms: 500,
        };
        let mut sched = LagSchedule::new(cfg, 99);
        assert!(!sched.is_active());
        assert_eq!(sched.step(0), LagDecision::PassThrough);
        assert_eq!(sched.step(5000), LagDecision::PassThrough);

        let dur = sched
            .arm_switch_freeze(5000, false)
            .expect("switch freeze must arm");
        assert!((100..=500).contains(&dur));
        assert!(sched.is_active());
        assert_eq!(sched.step(5050), LagDecision::StartFreeze);
        assert_eq!(sched.step(5100), LagDecision::HoldFreeze);

        // Must unfreeze within max_duration_ms (500ms) and return to inactive idle.
        let mut recovered_at = None;
        for t in (5150..=5650).step_by(50) {
            if sched.step(t) == LagDecision::PassThrough {
                recovered_at = Some(t);
                break;
            }
        }
        let end = recovered_at.expect("switch freeze must recover");
        assert!(end - 5050 <= 550);
        assert!(!sched.is_active());
        assert_eq!(sched.step(end + 5000), LagDecision::PassThrough);
    }

    #[test]
    fn switch_freeze_ignored_when_flag_disabled() {
        let cfg = LagConfig {
            enabled: false,
            freeze_on_switch: false,
            interval_ms: 3000,
            min_duration_ms: 100,
            max_duration_ms: 500,
        };
        let mut sched = LagSchedule::new(cfg, 13);
        assert!(sched.arm_switch_freeze(100, false).is_none());
        assert!(!sched.is_active());
        assert_eq!(sched.step(100), LagDecision::PassThrough);
    }
}
