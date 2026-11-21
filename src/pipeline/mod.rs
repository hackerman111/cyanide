pub mod audio;
pub mod lag;
pub mod record;
pub mod similarity;
pub mod video;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gstreamer as gst;
use gstreamer::bus::BusWatchGuard;
use gstreamer::glib;
use gstreamer::prelude::*;
use thiserror::Error;

use crate::config::{Config, LagConfig, LoopReturnPolicy};
use crate::pipeline::audio::{AudioBranch, GainRamp, parse_level_message};
use crate::pipeline::record::ActiveRecording;
use crate::pipeline::video::{LOOP_EOS_MSG, SegmentRebaser, VideoBranch};
use crate::state::{AppState, AudioLevels, SourceHealth, VideoSource};

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("missing required GStreamer element '{0}'")]
    MissingElement(&'static str),
    #[error("failed to link GStreamer elements: {0}")]
    Link(String),
    #[error("GStreamer state change failed: {0}")]
    StateChange(String),
    #[error("GStreamer error: {0}")]
    Gst(#[from] glib::BoolError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewBackend {
    Gtk4,
    Fake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DegradedBranch {
    Loop,
    LiveVideo,
    LiveAudio,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ForceFallback {
    pub loop_black: bool,
    pub live_video_black: bool,
    pub live_audio_silence: bool,
}

#[derive(Debug, Clone)]
pub struct PreviewObjects {
    pub loop_sink: gst::Element,
    pub live_sink: gst::Element,
    pub out_sink: gst::Element,
}

#[derive(Debug, Clone)]
pub enum PipelineEvent {
    Levels(AudioLevels),
    Position {
        pos_ms: u64,
        dur_ms: Option<u64>,
    },
    Degraded {
        branch: DegradedBranch,
        reason: String,
    },
    FatalError(String),
}

pub struct MediaPipeline {
    pipeline: gst::Pipeline,
    video: Rc<VideoBranch>,
    audio: Arc<AudioBranch>,
    bus_watch: Option<BusWatchGuard>,
    ramp_source: Rc<RefCell<Option<glib::SourceId>>>,
    ramp_state: Rc<RefCell<Option<GainRamp>>>,
    stopped: bool,
}

impl MediaPipeline {
    pub fn start(
        cfg: &Config,
        state: &AppState,
        backend: PreviewBackend,
        test_sources: bool,
        on_event: impl Fn(PipelineEvent) + 'static,
    ) -> Result<Self, PipelineError> {
        Self::start_with_fallback(
            cfg,
            state,
            backend,
            test_sources,
            ForceFallback::default(),
            on_event,
        )
    }

    pub fn start_with_fallback(
        cfg: &Config,
        state: &AppState,
        backend: PreviewBackend,
        test_sources: bool,
        fallback: ForceFallback,
        on_event: impl Fn(PipelineEvent) + 'static,
    ) -> Result<Self, PipelineError> {
        let pipeline = gst::Pipeline::with_name("cyanide_pipeline");
        let rebaser = Arc::new(SegmentRebaser::new());
        let audio = Arc::new(AudioBranch::build(
            &pipeline,
            cfg,
            test_sources,
            fallback,
            Arc::clone(&rebaser),
        )?);
        let video = Rc::new(VideoBranch::build(
            &pipeline,
            cfg,
            backend,
            test_sources,
            fallback,
            &audio,
            rebaser,
        )?);

        video.select(state.mode.video());
        video.matcher.set_frozen(state.frozen_reference);
        let (lg, rg) = state.target_gains(cfg);
        audio.set_gains(lg, rg);
        apply_av_delay(&video, &audio, cfg.av_delay_ms);

        let bus_watch = attach_bus_watch(&pipeline, Rc::clone(&video), on_event);

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| PipelineError::StateChange(format!("{e:?}")))?;

        Ok(Self {
            pipeline,
            video,
            audio,
            bus_watch,
            ramp_source: Rc::new(RefCell::new(None)),
            ramp_state: Rc::new(RefCell::new(None)),
            stopped: false,
        })
    }

    pub fn select_video(
        &self,
        source: VideoSource,
        return_policy: LoopReturnPolicy,
        switched_to_loop: bool,
    ) {
        self.video.lag.trigger_switch_freeze();
        if switched_to_loop {
            match return_policy {
                LoopReturnPolicy::Continue => {}
                LoopReturnPolicy::Restart => {
                    self.video.seek_loop(&self.pipeline, 0, true);
                }
                LoopReturnPolicy::Bookmark { position_ms } => {
                    self.video.seek_loop(&self.pipeline, position_ms, true);
                }
            }
        }
        self.video.select(source);
    }

    pub fn active_video_source(&self) -> VideoSource {
        self.video.active_source()
    }

    pub fn start_audio_ramp(&mut self, target: (f64, f64), crossfade_ms: u32) {
        let duration = crossfade_ms.clamp(10, 30);
        if let Some(existing) = self.ramp_state.borrow_mut().as_mut() {
            existing.retarget(target, duration);
            if self.ramp_source.borrow().is_some() {
                return;
            }
        } else {
            let start = self.audio.current_gains();
            *self.ramp_state.borrow_mut() = Some(GainRamp::new(start, target, duration, 5));
        }

        if let Some(id) = self.ramp_source.borrow_mut().take() {
            id.remove();
        }

        let audio = Arc::clone(&self.audio);
        let ramp_state = Rc::clone(&self.ramp_state);
        let ramp_source = Rc::clone(&self.ramp_source);
        let id = glib::timeout_add_local(Duration::from_millis(5), move || {
            let mut slot = ramp_state.borrow_mut();
            let Some(ramp) = slot.as_mut() else {
                *ramp_source.borrow_mut() = None;
                return glib::ControlFlow::Break;
            };
            let ((lg, rg), done) = ramp.step();
            audio.set_gains(lg, rg);
            if done {
                *slot = None;
                *ramp_source.borrow_mut() = None;
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        *self.ramp_source.borrow_mut() = Some(id);
    }

    pub fn set_av_delay_ms(&self, av_delay_ms: i32) {
        apply_av_delay(&self.video, &self.audio, av_delay_ms);
    }

    pub fn set_previews_active(&self, active: bool) {
        self.video.set_previews_active(active);
    }

    pub fn similarity(&self) -> f32 {
        self.video.matcher.score()
    }

    pub fn set_reference_frozen(&self, frozen: bool) {
        self.video.matcher.set_frozen(frozen);
    }

    pub fn set_lag_config(&self, lag: LagConfig) {
        self.video.lag.update_config(lag);
    }

    pub fn set_loop_ping_pong(&self, enabled: bool) {
        self.video.set_ping_pong(enabled);
    }

    pub fn is_loop_reversed(&self) -> bool {
        self.video.is_loop_reversed()
    }

    pub fn start_recording(&self, path: PathBuf, fps: u32) -> Result<PathBuf, PipelineError> {
        self.video.recorder.start(path, fps)
    }

    pub fn take_active_recording(&self) -> Option<ActiveRecording> {
        self.video.recorder.take_active()
    }

    pub fn is_recording(&self) -> bool {
        self.video.recorder.is_recording()
    }

    pub fn recording_elapsed_ms(&self) -> Option<u64> {
        self.video.recorder.elapsed_ms()
    }

    pub fn loop_position_ms(&self) -> Option<(u64, Option<u64>)> {
        self.video.loop_position_ms()
    }

    pub fn preview_objects(&self) -> &PreviewObjects {
        &self.video.previews
    }

    pub fn health(&self) -> SourceHealth {
        SourceHealth {
            loop_warning: self.video.loop_warning.clone(),
            live_warning: join_warnings(
                self.video.live_warning.as_deref(),
                self.audio.live_warning.as_deref(),
            ),
            output_warning: join_warnings(
                self.video.output_warning.as_deref(),
                self.audio.output_warning.as_deref(),
            ),
        }
    }

    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Some(rec) = self.video.recorder.take_active() {
            let _ = rec.finalize();
        }
        if let Some(id) = self.ramp_source.borrow_mut().take() {
            id.remove();
        }
        *self.ramp_state.borrow_mut() = None;
        self.bus_watch.take();
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

impl Drop for MediaPipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn check_runtime_plugins() -> Vec<(&'static str, bool)> {
    const REQUIRED: &[&str] = &[
        "gtk4paintablesink",
        "uridecodebin",
        "videoconvert",
        "videoscale",
        "videorate",
        "clocksync",
        "input-selector",
        "valve",
        "audioconvert",
        "audioresample",
        "audiomixer",
        "volume",
        "v4l2src",
        "v4l2sink",
        "pulsesrc",
        "pulsesink",
        "level",
    ];
    REQUIRED
        .iter()
        .map(|&name| (name, gst::ElementFactory::find(name).is_some()))
        .collect()
}

fn apply_av_delay(video: &VideoBranch, audio: &AudioBranch, av_delay_ms: i32) {
    let clamped = av_delay_ms.clamp(-500, 500);
    if clamped >= 0 {
        audio.set_delay_ns((clamped as u64) * 1_000_000);
        video.set_delay_ns(0);
    } else {
        video.set_delay_ns((clamped.unsigned_abs() as u64) * 1_000_000);
        audio.set_delay_ns(0);
    }
}

fn attach_bus_watch(
    pipeline: &gst::Pipeline,
    video: Rc<VideoBranch>,
    on_event: impl Fn(PipelineEvent) + 'static,
) -> Option<BusWatchGuard> {
    let bus = pipeline.bus()?;
    let weak_pipe = pipeline.downgrade();
    let armed_segment = AtomicBool::new(false);
    let levels = RefCell::new(AudioLevels::default());

    bus.add_watch_local(move |_bus, msg| {
        match msg.view() {
            gst::MessageView::AsyncDone(_) => {
                if video.loop_source.is_some()
                    && !armed_segment.swap(true, Ordering::AcqRel)
                    && let Some(p) = weak_pipe.upgrade()
                {
                    video.seek_loop(&p, 0, true);
                }
            }
            gst::MessageView::SegmentDone(_) => {
                if let Some(p) = weak_pipe.upgrade() {
                    video.advance_loop_segment(&p, false);
                }
            }
            gst::MessageView::Application(app_msg) => {
                if app_msg
                    .structure()
                    .is_some_and(|s| s.name() == LOOP_EOS_MSG)
                    && video.loop_source.is_some()
                    && let Some(p) = weak_pipe.upgrade()
                {
                    video.advance_loop_segment(&p, true);
                }
            }
            gst::MessageView::Eos(_) => {
                if video.loop_source.is_some()
                    && let Some(p) = weak_pipe.upgrade()
                {
                    video.advance_loop_segment(&p, true);
                }
            }
            gst::MessageView::Element(_) => {
                if let Some((name, db)) = parse_level_message(msg) {
                    let mut lv = levels.borrow_mut();
                    match name.as_str() {
                        "loop_level" => lv.loop_db = db,
                        "live_level" => lv.live_db = db,
                        "out_level" => lv.out_db = db,
                        _ => return glib::ControlFlow::Continue,
                    }
                    on_event(PipelineEvent::Levels(*lv));
                }
            }
            gst::MessageView::Error(err) => {
                let src_path = msg
                    .src()
                    .map(|s| s.path_string().to_string())
                    .unwrap_or_default();
                let reason = format!("{} ({:?})", err.error(), err.debug());
                if let Some(branch) = classify_error_branch(&src_path) {
                    on_event(PipelineEvent::Degraded { branch, reason });
                } else {
                    on_event(PipelineEvent::FatalError(reason));
                }
            }
            _ => {}
        }
        glib::ControlFlow::Continue
    })
    .ok()
}

fn classify_error_branch(src_path: &str) -> Option<DegradedBranch> {
    if src_path.contains("loop_") || src_path.contains("uridecodebin") {
        Some(DegradedBranch::Loop)
    } else if src_path.contains("live_mic_src")
        || src_path.contains("live_audio_")
        || src_path.contains("pulsesrc")
    {
        Some(DegradedBranch::LiveAudio)
    } else if src_path.contains("live_") || src_path.contains("v4l2src") {
        Some(DegradedBranch::LiveVideo)
    } else {
        None
    }
}

fn join_warnings(a: Option<&str>, b: Option<&str>) -> Option<String> {
    match (a, b) {
        (Some(x), Some(y)) => Some(format!("{x}; {y}")),
        (Some(x), None) => Some(x.to_string()),
        (None, Some(y)) => Some(y.to_string()),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::StartupMode;
    use crate::state::{Command, StreamMode};

    #[test]
    fn pipeline_starts_switches_modes_and_applies_av_delay() {
        gst::init().unwrap();
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let cfg = Config::default();
            let mut state = AppState::from_config(&cfg);
            let mut pipe =
                MediaPipeline::start(&cfg, &state, PreviewBackend::Fake, true, |_| {}).unwrap();

            let (res, gst_state, _) = pipe.pipeline.state(gst::ClockTime::from_seconds(2));
            assert!(res.is_ok() && gst_state == gst::State::Playing);

            pipe.set_previews_active(true);
            pipe.set_av_delay_ms(120);
            pipe.set_av_delay_ms(-80);
            state.apply_mode_command(&Command::Toggle);
            pipe.select_video(state.mode.video(), cfg.loop_return, true);

            // input-selector commits pending active-pad on the next incoming frame (~33ms at 30fps).
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            while pipe.active_video_source() != VideoSource::Loop
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(pipe.active_video_source(), VideoSource::Loop);

            pipe.set_previews_active(false);
            pipe.stop();
        })
        .unwrap();
    }

    #[test]
    fn pipeline_loop_degradation_never_switches_to_live() {
        gst::init().unwrap();
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let cfg = Config {
                startup_mode: StartupMode::Loop,
                loop_file: Some(PathBuf::from("/nonexistent/missing-loop-file.mp4")),
                ..Default::default()
            };
            let state = AppState::from_config(&cfg);
            let mut pipe =
                MediaPipeline::start(&cfg, &state, PreviewBackend::Fake, true, |_| {}).unwrap();

            // Missing MP4 falls back to synthetic placeholder with warning; mode stays Loop
            assert!(pipe.health().loop_warning.is_some());
            assert_eq!(state.mode, StreamMode::Loop);
            assert_eq!(pipe.active_video_source(), VideoSource::Loop);
            pipe.stop();
        })
        .unwrap();
    }

    #[test]
    fn pipeline_has_clocksync_on_loop_and_output_for_pacing_and_av_delay() {
        gst::init().unwrap();
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let cfg = Config::default();
            let state = AppState::from_config(&cfg);
            let mut pipe =
                MediaPipeline::start(&cfg, &state, PreviewBackend::Fake, true, |_| {}).unwrap();

            assert!(
                pipe.pipeline.by_name("loop_clocksync").is_some(),
                "loop branch must include clocksync for 1x MP4 pacing"
            );
            assert!(
                pipe.pipeline.by_name("video_out_clocksync").is_some(),
                "video output must include clocksync after video_out_queue so negative av_delay_ms takes effect"
            );

            pipe.set_av_delay_ms(-250);
            let v_off = pipe.video.out_queue.static_pad("src").unwrap().offset();
            let a_off = pipe.audio.out_queue.static_pad("src").unwrap().offset();
            assert_eq!(v_off, 250_000_000);
            assert_eq!(a_off, 0);

            pipe.stop();
        })
        .unwrap();
    }

    #[test]
    fn classify_error_isolates_live_audio_from_live_video() {
        let mic = classify_error_branch("/cyanide_pipeline/live_mic_src");
        let cam = classify_error_branch("/cyanide_pipeline/live_v4l2_src");
        let lp = classify_error_branch("/cyanide_pipeline/loop_uridecodebin");
        assert!(mic.is_some() && cam.is_some() && lp.is_some());
        assert_ne!(
            mic, cam,
            "pulsesrc/live_mic_src errors must NOT degrade the live camera branch"
        );
    }

    #[test]
    fn pipeline_records_live_mp4_and_plays_ping_pong_forward_reverse_forward() {
        gst::init().unwrap();
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let rec_path = std::env::temp_dir()
                .join(format!("cyanide-pingpong-test-{}.mp4", std::process::id()));
            let _ = std::fs::remove_file(&rec_path);

            // Step 1: Record ~0.55s of live test video via MediaPipeline.
            let rec_cfg = Config {
                video: crate::config::VideoFormat {
                    width: 320,
                    height: 240,
                    fps: 30,
                },
                ..Default::default()
            };
            let rec_state = AppState::from_config(&rec_cfg);
            let mut rec_pipe =
                MediaPipeline::start(&rec_cfg, &rec_state, PreviewBackend::Fake, true, |_| {})
                    .unwrap();
            let (res, gst_state, _) = rec_pipe.pipeline.state(gst::ClockTime::from_seconds(2));
            assert!(res.is_ok() && gst_state == gst::State::Playing);

            rec_pipe.start_recording(rec_path.clone(), 30).unwrap();
            assert!(rec_pipe.is_recording());
            std::thread::sleep(Duration::from_millis(600));
            let active = rec_pipe
                .take_active_recording()
                .expect("recording must be active");
            let finished = active.finalize().expect("mp4 recording must finalize");
            assert!(
                finished.frame_count >= 5,
                "expected recorded frames, got {}",
                finished.frame_count
            );
            rec_pipe.stop();

            // Step 2: Play the recorded MP4 in ping-pong mode and verify forward -> reverse -> forward.
            let play_cfg = Config {
                startup_mode: StartupMode::Loop,
                loop_file: Some(rec_path.clone()),
                loop_ping_pong: true,
                video: crate::config::VideoFormat {
                    width: 320,
                    height: 240,
                    fps: 30,
                },
                ..Default::default()
            };
            let play_state = AppState::from_config(&play_cfg);
            let errors = Rc::new(RefCell::new(Vec::<String>::new()));
            let errors_cb = Rc::clone(&errors);
            let mut play_pipe = MediaPipeline::start(
                &play_cfg,
                &play_state,
                PreviewBackend::Fake,
                true,
                move |ev| match ev {
                    PipelineEvent::Degraded { reason, .. } | PipelineEvent::FatalError(reason) => {
                        errors_cb.borrow_mut().push(reason);
                    }
                    _ => {}
                },
            )
            .unwrap();

            // Record raw decoded PTS on loop_video_queue sink (before reverse-segment normalization)
            // and paced output frames leaving loop_clocksync (after videorate + clocksync).
            let raw_pts_log = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
            if let Some(q) = play_pipe.pipeline.by_name("loop_video_queue")
                && let Some(sink_pad) = q.static_pad("sink")
            {
                let rlog = Arc::clone(&raw_pts_log);
                sink_pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
                    if let Some(gst::PadProbeData::Buffer(ref buf)) = info.data
                        && let Some(pts) = buf.pts()
                    {
                        rlog.lock().unwrap().push(pts.mseconds());
                    }
                    gst::PadProbeReturn::Ok
                });
            }
            let pts_log = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
            if let Some(cs) = play_pipe.pipeline.by_name("loop_clocksync")
                && let Some(src_pad) = cs.static_pad("src")
            {
                let log = Arc::clone(&pts_log);
                src_pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
                    if let Some(gst::PadProbeData::Buffer(ref buf)) = info.data
                        && let Some(pts) = buf.pts()
                    {
                        log.lock().unwrap().push(pts.mseconds());
                    }
                    gst::PadProbeReturn::Ok
                });

                let vid = Rc::clone(&play_pipe.video);
                let mut saw_reverse = false;
                let mut saw_return_forward = false;
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while std::time::Instant::now() < deadline {
                    while ctx.iteration(false) {}
                    let rev = vid.is_loop_reversed();
                    if rev {
                        saw_reverse = true;
                    } else if saw_reverse {
                        saw_return_forward = true;
                    }
                    let count = pts_log.lock().unwrap().len();
                    if saw_return_forward && count >= 45 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }

                assert!(
                    errors.borrow().is_empty(),
                    "unexpected pipeline errors: {:?}",
                    errors.borrow()
                );
                let pts = pts_log.lock().unwrap().clone();
                let raw_pts = raw_pts_log.lock().unwrap().clone();
                assert!(saw_reverse, "ping-pong never entered reverse playback");
                assert!(
                    saw_return_forward,
                    "ping-pong never returned to forward playback after reverse"
                );
                // Upstream decoded stream must have both ascending (forward) and descending (reverse) PTS.
                let asc = raw_pts.windows(2).filter(|w| w[1] > w[0]).count();
                let desc = raw_pts.windows(2).filter(|w| w[1] < w[0]).count();
                assert!(asc >= 10, "expected ascending forward PTS, got {raw_pts:?}");
                assert!(
                    desc >= 10,
                    "expected descending reverse PTS, got {raw_pts:?}"
                );
                // Downstream videorate + clocksync must emit all passes (~18 frames each) without dropping reverse.
                assert!(
                    pts.len() >= 45,
                    "expected >= 45 paced frames across forward+reverse+forward, got {} ({pts:?})",
                    pts.len()
                );
            } else {
                panic!("missing loop_clocksync");
            }

            play_pipe.stop();
            let _ = std::fs::remove_file(&rec_path);
        })
        .unwrap();
    }
}
