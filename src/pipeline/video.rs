use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_video as gst_video;

use crate::config::Config;
use crate::devices::modprobe_hint;
use crate::pipeline::audio::AudioBranch;
use crate::pipeline::lag::LagController;
use crate::pipeline::record::VideoRecorder;
use crate::pipeline::similarity::{FrameMatcher, FrameSide};
use crate::pipeline::{ForceFallback, PipelineError, PreviewBackend, PreviewObjects};
use crate::state::VideoSource;

pub const LOOP_EOS_MSG: &str = "cyanide-loop-eos";

pub struct VideoBranch {
    pub selector: gst::Element,
    pub loop_sel_pad: gst::Pad,
    pub live_sel_pad: gst::Pad,
    pub loop_source: Option<gst::Element>,
    pub out_queue: gst::Element,
    pub loop_valve: gst::Element,
    pub live_valve: gst::Element,
    pub out_valve: gst::Element,
    pub previews: PreviewObjects,
    pub matcher: Arc<FrameMatcher>,
    pub lag: Arc<LagController>,
    pub recorder: Arc<VideoRecorder>,
    pub rebaser: Arc<SegmentRebaser>,
    pub ping_pong: AtomicBool,
    loop_reversed: AtomicBool,
    pub loop_warning: Option<String>,
    pub live_warning: Option<String>,
    pub output_warning: Option<String>,
}

impl VideoBranch {
    pub fn build(
        pipeline: &gst::Pipeline,
        cfg: &Config,
        backend: PreviewBackend,
        test_sources: bool,
        fallback: ForceFallback,
        audio: &Arc<AudioBranch>,
        rebaser: Arc<SegmentRebaser>,
    ) -> Result<Self, PipelineError> {
        let common_caps = common_video_caps(cfg.video.width, cfg.video.height, cfg.video.fps);

        let selector = make_element("input-selector", Some("video_sel"))?;
        selector.set_property("sync-streams", true);
        selector.set_property_from_str("sync-mode", "clock");
        pipeline.add(&selector)?;

        let loop_sel_pad = selector
            .request_pad_simple("sink_%u")
            .ok_or_else(|| PipelineError::Link("failed to request video_sel loop pad".into()))?;
        let live_sel_pad = selector
            .request_pad_simple("sink_%u")
            .ok_or_else(|| PipelineError::Link("failed to request video_sel live pad".into()))?;

        let (loop_source, loop_valve, loop_sink, loop_warning) = build_loop_branch(
            pipeline,
            cfg,
            &common_caps,
            backend,
            test_sources,
            fallback.loop_black,
            &loop_sel_pad,
            audio,
            &rebaser,
        )?;

        let recorder = Arc::new(VideoRecorder::new());

        let (live_valve, live_sink, live_warning) = build_live_branch(
            pipeline,
            cfg,
            &common_caps,
            backend,
            test_sources,
            fallback.live_video_black,
            &live_sel_pad,
            &recorder,
        )?;

        let out_queue = make_element("queue", Some("video_out_queue"))?;
        out_queue.set_property("max-size-time", 2_000_000_000u64);
        out_queue.set_property("max-size-buffers", 0u32);
        out_queue.set_property("max-size-bytes", 0u32);

        // clocksync paces output after video_out_queue so pad.set_offset() actually delays
        // both v4l2sink and out_preview when av_delay_ms < 0 despite v4l2sink(sync=false).
        let out_clocksync = make_element("clocksync", Some("video_out_clocksync"))?;

        let out_tee = make_element("tee", Some("out_tee"))?;
        out_tee.set_property("allow-not-linked", true);

        let out_broadcast_queue = make_element("queue", Some("out_broadcast_queue"))?;
        let out_convert = make_element("videoconvert", Some("out_v4l2_convert"))?;
        let yuy2_caps = gst::Caps::builder("video/x-raw")
            .field("format", "YUY2")
            .field("width", cfg.video.width as i32)
            .field("height", cfg.video.height as i32)
            .field("framerate", gst::Fraction::new(cfg.video.fps as i32, 1))
            .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
            .field("interlace-mode", "progressive")
            .build();
        let out_caps = make_capsfilter("out_v4l2_caps", &yuy2_caps)?;

        let (broadcast_sink, output_warning) = build_virtual_cam_sink(cfg, test_sources)?;

        pipeline.add_many([
            &out_queue,
            &out_clocksync,
            &out_tee,
            &out_broadcast_queue,
            &out_convert,
            &out_caps,
            &broadcast_sink,
        ])?;

        gst::Element::link_many([
            &selector,
            &out_queue,
            &out_clocksync,
            &out_tee,
            &out_broadcast_queue,
            &out_convert,
            &out_caps,
            &broadcast_sink,
        ])
        .map_err(|e| PipelineError::Link(format!("video_sel -> broadcast: {e}")))?;

        let (out_valve, out_sink) = attach_preview_leg(pipeline, &out_tee, "out", backend)?;

        let matcher = Arc::new(FrameMatcher::new());
        attach_similarity_probe(&loop_valve, FrameSide::Loop, Arc::clone(&matcher));
        attach_similarity_probe(&live_valve, FrameSide::Live, Arc::clone(&matcher));

        let lag = Arc::new(LagController::new(cfg.lag));
        attach_lag_probe(&out_clocksync, Arc::clone(&lag));

        Ok(Self {
            selector,
            loop_sel_pad,
            live_sel_pad,
            loop_source,
            out_queue,
            loop_valve,
            live_valve,
            out_valve,
            previews: PreviewObjects {
                loop_sink,
                live_sink,
                out_sink,
            },
            matcher,
            lag,
            recorder,
            rebaser,
            ping_pong: AtomicBool::new(cfg.loop_ping_pong),
            loop_reversed: AtomicBool::new(false),
            loop_warning,
            live_warning,
            output_warning,
        })
    }

    pub fn select(&self, source: VideoSource) {
        let pad = match source {
            VideoSource::Loop => &self.loop_sel_pad,
            VideoSource::Live => &self.live_sel_pad,
        };
        self.selector.set_property("active-pad", pad);
    }

    /// Switches video source; when `freeze_on_switch` is enabled, freezes the *current*
    /// source's frame first and flips `input-selector` underneath so the viewer sees a natural
    /// pre-switch stutter before transitioning to the new mode. Returns the freeze duration in ms.
    pub fn select_with_pre_freeze(&self, source: VideoSource) -> Option<u32> {
        let selector = self.selector.clone();
        let target_pad = match source {
            VideoSource::Loop => self.loop_sel_pad.clone(),
            VideoSource::Live => self.live_sel_pad.clone(),
        };
        if let Some(dur_ms) = self.lag.arm_switch_freeze(move || {
            selector.set_property("active-pad", &target_pad);
        }) {
            Some(dur_ms)
        } else {
            self.select(source);
            None
        }
    }

    pub fn active_source(&self) -> VideoSource {
        match self.selector.property::<Option<gst::Pad>>("active-pad") {
            Some(pad) if pad == self.loop_sel_pad => VideoSource::Loop,
            _ => VideoSource::Live,
        }
    }

    pub fn set_delay_ns(&self, video_delay_ns: u64) {
        if let Some(pad) = self.out_queue.static_pad("src") {
            pad.set_offset(video_delay_ns as i64);
        }
    }

    pub fn set_previews_active(&self, active: bool) {
        let drop_frames = !active;
        self.loop_valve.set_property("drop", drop_frames);
        self.live_valve.set_property("drop", drop_frames);
        self.out_valve.set_property("drop", drop_frames);
    }

    pub fn set_ping_pong(&self, enabled: bool) {
        self.ping_pong.store(enabled, Ordering::Release);
    }

    pub fn is_loop_reversed(&self) -> bool {
        self.loop_reversed.load(Ordering::Acquire)
    }

    pub fn seek_loop(&self, pipeline: &gst::Pipeline, position_ms: u64, flushing: bool) -> bool {
        self.loop_reversed.store(false, Ordering::Release);
        self.seek_loop_segment(pipeline, false, Some(position_ms), flushing)
    }

    /// Advances the loop video at a segment boundary (SegmentDone / EOS).
    /// When ping-pong is enabled, alternates +1.0 (forward) and -1.0 (reverse).
    pub fn advance_loop_segment(&self, pipeline: &gst::Pipeline, flushing: bool) -> bool {
        if !self.ping_pong.load(Ordering::Acquire) {
            self.loop_reversed.store(false, Ordering::Release);
            return self.seek_loop_segment(pipeline, false, Some(0), flushing);
        }

        let next_reversed = !self.loop_reversed.load(Ordering::Acquire);
        if self.seek_loop_segment(pipeline, next_reversed, None, flushing) {
            self.loop_reversed.store(next_reversed, Ordering::Release);
            true
        } else {
            // Fall back to forward restart if the file/decoder rejects negative-rate seeking.
            self.loop_reversed.store(false, Ordering::Release);
            self.seek_loop_segment(pipeline, false, Some(0), flushing)
        }
    }

    fn seek_loop_segment(
        &self,
        pipeline: &gst::Pipeline,
        reversed: bool,
        start_ms: Option<u64>,
        flushing: bool,
    ) -> bool {
        let Some(ref src) = self.loop_source else {
            return false;
        };
        if flushing {
            // Rebase post-seek GstSegment.base to the live pipeline running_time so
            // audiomixer and clocksync do not drop frames/audio after Restart/Bookmark/reverse seeks.
            let rt_ns = pipeline
                .current_running_time()
                .map_or(0, gst::ClockTime::nseconds);
            self.rebaser.mark_flushing_seek(rt_ns);
        }
        let mut flags = gst::SeekFlags::SEGMENT | gst::SeekFlags::ACCURATE;
        if flushing {
            flags |= gst::SeekFlags::FLUSH;
        }
        if reversed {
            let stop = src.query_duration::<gst::ClockTime>();
            let (stop_type, stop_pos) = match stop {
                Some(dur) if dur > gst::ClockTime::ZERO => (gst::SeekType::Set, Some(dur)),
                _ => (gst::SeekType::None, gst::ClockTime::NONE),
            };
            src.seek(
                -1.0,
                flags,
                gst::SeekType::Set,
                gst::ClockTime::ZERO,
                stop_type,
                stop_pos,
            )
            .is_ok()
        } else {
            let pos = start_ms.unwrap_or(0);
            src.seek(
                1.0,
                flags,
                gst::SeekType::Set,
                gst::ClockTime::from_mseconds(pos),
                gst::SeekType::None,
                gst::ClockTime::NONE,
            )
            .is_ok()
        }
    }

    pub fn loop_position_ms(&self) -> Option<(u64, Option<u64>)> {
        let src = self.loop_source.as_ref()?;
        let pos = src.query_position::<gst::ClockTime>()?.mseconds();
        let dur = src.query_duration::<gst::ClockTime>().map(|d| d.mseconds());
        Some((pos, dur))
    }
}

#[allow(clippy::too_many_arguments)]
fn build_loop_branch(
    pipeline: &gst::Pipeline,
    cfg: &Config,
    common_caps: &gst::Caps,
    backend: PreviewBackend,
    test_sources: bool,
    force_black: bool,
    sel_pad: &gst::Pad,
    audio: &Arc<AudioBranch>,
    rebaser: &Arc<SegmentRebaser>,
) -> Result<
    (
        Option<gst::Element>,
        gst::Element,
        gst::Element,
        Option<String>,
    ),
    PipelineError,
> {
    let queue = make_element("queue", Some("loop_video_queue"))?;
    let convert = make_element("videoconvert", Some("loop_convert"))?;
    let scale = make_element("videoscale", Some("loop_scale"))?;
    let rate = make_element("videorate", Some("loop_rate"))?;
    let caps = make_capsfilter("loop_caps", common_caps)?;
    // clocksync paces non-live MP4 uridecodebin frames at 1x real-time even when audio-less.
    let clocksync = make_element("clocksync", Some("loop_clocksync"))?;
    let tee = make_element("tee", Some("loop_tee"))?;
    tee.set_property("allow-not-linked", true);
    let sel_queue = make_element("queue", Some("loop_sel_queue"))?;

    pipeline.add_many([
        &queue, &convert, &scale, &rate, &caps, &clocksync, &tee, &sel_queue,
    ])?;
    gst::Element::link_many([
        &queue, &convert, &scale, &rate, &caps, &clocksync, &tee, &sel_queue,
    ])
    .map_err(|e| PipelineError::Link(format!("loop video chain: {e}")))?;

    let src_pad = sel_queue
        .static_pad("src")
        .ok_or_else(|| PipelineError::Link("loop_sel_queue missing src pad".into()))?;
    install_selector_caps_normalizer(&src_pad);
    src_pad
        .link(sel_pad)
        .map_err(|e| PipelineError::Link(format!("loop -> video_sel: {e}")))?;

    let (valve, preview_sink) = attach_preview_leg(pipeline, &tee, "loop", backend)?;

    let missing_file_warning = cfg.loop_file.as_ref().and_then(|p| {
        if !p.exists() {
            Some(format!("Файл MP4 не найден: {}", p.display()))
        } else {
            None
        }
    });

    if !force_black
        && let Some(path) = cfg.loop_file.as_deref()
        && path.exists()
        && let Ok(uri) = path_to_uri(path)
    {
        let decodebin = make_element("uridecodebin", Some("loop_uridecodebin"))?;
        decodebin.set_property("uri", uri.as_str());
        pipeline.add(&decodebin)?;

        // Rebase segment base on flushing seeks, normalize reverse ping-pong segments into
        // forward PTS for videorate/clocksync, and trap upstream demuxer EOS before it stalls.
        if let Some(q_src) = queue.static_pad("src") {
            let reb = Arc::clone(rebaser);
            let seen_gen = AtomicU64::new(0);
            let weak_pipe = pipeline.downgrade();
            let rev_seg: Mutex<Option<gst::FormattedSegment<gst::ClockTime>>> = Mutex::new(None);
            q_src.add_probe(
                gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_DOWNSTREAM,
                move |_pad, info| {
                    match info.data {
                        Some(gst::PadProbeData::Event(ref ev)) => match ev.view() {
                            gst::EventView::Segment(seg_ev) => {
                                if let Some(seg) = seg_ev.segment().downcast_ref::<gst::ClockTime>()
                                {
                                    let rebased = reb.rebase_if_needed(&seen_gen, seg);
                                    let effective = rebased.as_ref().unwrap_or(seg);
                                    if effective.rate() < 0.0 {
                                        if let Ok(mut guard) = rev_seg.lock() {
                                            *guard = Some(effective.clone());
                                        }
                                        let fwd = normalize_reverse_segment(effective);
                                        let new_ev = gst::event::Segment::builder(&fwd)
                                            .seqnum(ev.seqnum())
                                            .build();
                                        info.data = Some(gst::PadProbeData::Event(new_ev));
                                    } else {
                                        if let Ok(mut guard) = rev_seg.lock() {
                                            *guard = None;
                                        }
                                        if let Some(rebased) = rebased {
                                            let new_ev = gst::event::Segment::builder(&rebased)
                                                .seqnum(ev.seqnum())
                                                .build();
                                            info.data = Some(gst::PadProbeData::Event(new_ev));
                                        }
                                    }
                                }
                            }
                            gst::EventView::Eos(_) => {
                                if let Some(p) = weak_pipe.upgrade() {
                                    let msg = gst::message::Application::new(
                                        gst::Structure::new_empty(LOOP_EOS_MSG),
                                    );
                                    let _ = p.post_message(msg);
                                }
                                return gst::PadProbeReturn::Drop;
                            }
                            _ => {}
                        },
                        Some(gst::PadProbeData::Buffer(ref mut buf)) => {
                            if let Ok(guard) = rev_seg.lock()
                                && let Some(ref rseg) = *guard
                                && let Some(pts) = buf.pts()
                                && let Some(rt) = rseg.to_running_time(pts)
                            {
                                let base = rseg.base().unwrap_or(gst::ClockTime::ZERO);
                                let norm_pts = rt.saturating_sub(base);
                                let buf_mut = buf.make_mut();
                                buf_mut.set_pts(norm_pts);
                                buf_mut.set_dts(gst::ClockTime::NONE);
                            }
                        }
                        _ => {}
                    }
                    gst::PadProbeReturn::Ok
                },
            );
        }

        let video_linked = Arc::new(AtomicBool::new(false));
        let video_queue = queue.clone();
        let audio_ref = Arc::clone(audio);
        decodebin.connect_pad_added(move |_src, pad| {
            let Some(caps) = pad.current_caps().or_else(|| Some(pad.query_caps(None))) else {
                return;
            };
            let Some(s) = caps.structure(0) else {
                return;
            };
            let media = s.name().as_str();
            if media.starts_with("video/") && !video_linked.swap(true, Ordering::AcqRel) {
                if let Some(sink_pad) = video_queue.static_pad("sink") {
                    let _ = pad.link(&sink_pad);
                }
            } else if media.starts_with("audio/") {
                let _ = audio_ref.attach_loop_audio_pad(pad);
            }
        });

        return Ok((Some(decodebin), valve, preview_sink, None));
    }

    let (pattern, warning) = if force_black {
        (
            "black",
            Some("Ошибка воспроизведения MP4 — показан чёрный кадр".into()),
        )
    } else if let Some(warn) = missing_file_warning {
        (if test_sources { "smpte" } else { "black" }, Some(warn))
    } else if test_sources {
        ("smpte", None)
    } else {
        (
            "black",
            Some("MP4-файл не выбран — используется чёрная заглушка".into()),
        )
    };

    let fallback_src = make_element("videotestsrc", Some("loop_fallback_src"))?;
    fallback_src.set_property_from_str("pattern", pattern);
    fallback_src.set_property("is-live", true);
    pipeline.add(&fallback_src)?;
    fallback_src
        .link(&queue)
        .map_err(|e| PipelineError::Link(format!("loop fallback -> queue: {e}")))?;

    Ok((None, valve, preview_sink, warning))
}

#[allow(clippy::too_many_arguments)]
fn build_live_branch(
    pipeline: &gst::Pipeline,
    cfg: &Config,
    common_caps: &gst::Caps,
    backend: PreviewBackend,
    test_sources: bool,
    force_black: bool,
    sel_pad: &gst::Pad,
    recorder: &Arc<VideoRecorder>,
) -> Result<(gst::Element, gst::Element, Option<String>), PipelineError> {
    let queue = make_element("queue", Some("live_video_queue"))?;
    let convert = make_element("videoconvert", Some("live_convert"))?;
    let scale = make_element("videoscale", Some("live_scale"))?;
    let rate = make_element("videorate", Some("live_rate"))?;
    let caps = make_capsfilter("live_caps", common_caps)?;
    let tee = make_element("tee", Some("live_tee"))?;
    tee.set_property("allow-not-linked", true);
    let sel_queue = make_element("queue", Some("live_sel_queue"))?;

    let (src, warning) = if force_black {
        let s = make_element("videotestsrc", Some("live_fallback_src"))?;
        s.set_property_from_str("pattern", "black");
        s.set_property("is-live", true);
        (
            s,
            Some("Камера недоступна или отключена — показан чёрный кадр".into()),
        )
    } else if test_sources {
        let s = make_element("videotestsrc", Some("live_test_src"))?;
        s.set_property_from_str("pattern", "ball");
        s.set_property("is-live", true);
        (s, None)
    } else if gst::ElementFactory::find("v4l2src").is_none() {
        let s = make_element("videotestsrc", Some("live_fallback_src"))?;
        s.set_property_from_str("pattern", "black");
        s.set_property("is-live", true);
        (
            s,
            Some(
                "Нет плагина v4l2src (установите gst-plugins-good) — камера заменена заглушкой"
                    .into(),
            ),
        )
    } else if let Some(dev) = cfg.real_camera.as_deref().filter(|p| Path::new(p).exists()) {
        let s = make_element("v4l2src", Some("live_v4l2_src"))?;
        s.set_property("device", dev);
        (s, None)
    } else {
        let s = make_element("videotestsrc", Some("live_fallback_src"))?;
        s.set_property_from_str("pattern", "black");
        s.set_property("is-live", true);
        (
            s,
            Some("Реальная веб-камера не выбрана или не найдена".into()),
        )
    };

    pipeline.add_many([
        &src, &queue, &convert, &scale, &rate, &caps, &tee, &sel_queue,
    ])?;
    gst::Element::link_many([
        &src, &queue, &convert, &scale, &rate, &caps, &tee, &sel_queue,
    ])
    .map_err(|e| PipelineError::Link(format!("live video chain: {e}")))?;

    let src_pad = sel_queue
        .static_pad("src")
        .ok_or_else(|| PipelineError::Link("live_sel_queue missing src pad".into()))?;
    install_selector_caps_normalizer(&src_pad);
    src_pad
        .link(sel_pad)
        .map_err(|e| PipelineError::Link(format!("live -> video_sel: {e}")))?;

    let (valve, preview_sink) = attach_preview_leg(pipeline, &tee, "live", backend)?;
    attach_record_probe(&caps, Arc::clone(recorder));
    Ok((valve, preview_sink, warning))
}

fn attach_preview_leg(
    pipeline: &gst::Pipeline,
    tee: &gst::Element,
    prefix: &str,
    backend: PreviewBackend,
) -> Result<(gst::Element, gst::Element), PipelineError> {
    let q_name = format!("{prefix}_preview_queue");
    let v_name = format!("{prefix}_preview_valve");
    let s_name = format!("{prefix}_preview_sink");

    let queue = make_element("queue", Some(&q_name))?;
    queue.set_property_from_str("leaky", "downstream");
    queue.set_property("max-size-buffers", 2u32);
    queue.set_property("max-size-bytes", 0u32);
    queue.set_property("max-size-time", 0u64);

    // Start dropped; GUI opens the valves only while the window is visible.
    let valve = make_element("valve", Some(&v_name))?;
    valve.set_property("drop", true);

    match backend {
        PreviewBackend::Gtk4 => {
            let conv_name = format!("{prefix}_preview_convert");
            let convert = make_element("videoconvert", Some(&conv_name))?;
            let sink = make_element("gtk4paintablesink", Some(&s_name))?;
            // async=false prevents preroll stall while preview valve starts closed.
            sink.set_property("async", false);
            sink.set_property("sync", false);
            pipeline.add_many([&queue, &valve, &convert, &sink])?;
            gst::Element::link_many([tee, &queue, &valve, &convert, &sink])
                .map_err(|e| PipelineError::Link(format!("{prefix} gtk4 preview: {e}")))?;
            Ok((valve, sink))
        }
        PreviewBackend::Fake => {
            let sink = make_element("fakesink", Some(&s_name))?;
            sink.set_property("async", false);
            sink.set_property("sync", false);
            pipeline.add_many([&queue, &valve, &sink])?;
            gst::Element::link_many([tee, &queue, &valve, &sink])
                .map_err(|e| PipelineError::Link(format!("{prefix} fake preview: {e}")))?;
            Ok((valve, sink))
        }
    }
}

fn attach_similarity_probe(valve: &gst::Element, side: FrameSide, matcher: Arc<FrameMatcher>) {
    let Some(src_pad) = valve.static_pad("src") else {
        return;
    };
    let cached_info: Mutex<Option<gst_video::VideoInfo>> = Mutex::new(None);
    src_pad.add_probe(
        gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_DOWNSTREAM,
        move |pad, info| {
            match info.data {
                Some(gst::PadProbeData::Event(ref ev)) => {
                    if let gst::EventView::Caps(c) = ev.view()
                        && let Ok(mut guard) = cached_info.lock()
                    {
                        *guard = gst_video::VideoInfo::from_caps(c.caps()).ok();
                    }
                }
                Some(gst::PadProbeData::Buffer(ref buf)) => {
                    if let Ok(mut guard) = cached_info.lock() {
                        if guard.is_none() {
                            *guard = pad
                                .current_caps()
                                .and_then(|c| gst_video::VideoInfo::from_caps(&c).ok());
                        }
                        if let Some(ref vinfo) = *guard {
                            matcher.on_buffer(side, vinfo, buf);
                        }
                    }
                }
                _ => {}
            }
            gst::PadProbeReturn::Ok
        },
    );
}

fn normalize_reverse_segment(
    seg: &gst::FormattedSegment<gst::ClockTime>,
) -> gst::FormattedSegment<gst::ClockTime> {
    let base = seg.base().unwrap_or(gst::ClockTime::ZERO);
    let start = seg.start().unwrap_or(gst::ClockTime::ZERO);
    let duration = seg
        .stop()
        .and_then(|stop| stop.checked_sub(start))
        .map(|d| {
            let speed = seg.rate().abs();
            if speed > 0.0 && (speed - 1.0).abs() > 1e-6 {
                gst::ClockTime::from_nseconds((d.nseconds() as f64 / speed) as u64)
            } else {
                d
            }
        });
    let mut fwd = gst::FormattedSegment::<gst::ClockTime>::new();
    fwd.set_rate(1.0);
    fwd.set_applied_rate(seg.applied_rate());
    fwd.set_base(base);
    fwd.set_start(gst::ClockTime::ZERO);
    fwd.set_position(gst::ClockTime::ZERO);
    fwd.set_time(gst::ClockTime::ZERO);
    if let Some(dur) = duration {
        fwd.set_stop(dur);
    }
    fwd
}

fn attach_record_probe(live_caps: &gst::Element, recorder: Arc<VideoRecorder>) {
    let Some(src_pad) = live_caps.static_pad("src") else {
        return;
    };
    let cached_caps: Mutex<Option<gst::Caps>> = Mutex::new(None);
    src_pad.add_probe(
        gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_DOWNSTREAM,
        move |pad, info| {
            match info.data {
                Some(gst::PadProbeData::Event(ref ev)) => {
                    if let gst::EventView::Caps(c) = ev.view()
                        && let Ok(mut guard) = cached_caps.lock()
                    {
                        *guard = Some(c.caps().to_owned());
                    }
                }
                Some(gst::PadProbeData::Buffer(ref buf)) => {
                    if !recorder.is_recording() {
                        return gst::PadProbeReturn::Ok;
                    }
                    if let Ok(mut guard) = cached_caps.lock() {
                        if guard.is_none() {
                            *guard = pad.current_caps();
                        }
                        if let Some(ref caps) = *guard {
                            recorder.push_buffer(buf, caps);
                        }
                    }
                }
                _ => {}
            }
            gst::PadProbeReturn::Ok
        },
    );
}

fn attach_lag_probe(out_clocksync: &gst::Element, lag: Arc<LagController>) {
    let Some(src_pad) = out_clocksync.static_pad("src") else {
        return;
    };
    src_pad.add_probe(
        gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_DOWNSTREAM,
        move |pad, info| {
            match info.data {
                Some(gst::PadProbeData::Event(ref ev)) => {
                    if let gst::EventView::Caps(c) = ev.view() {
                        lag.on_caps(c.caps());
                    }
                }
                Some(gst::PadProbeData::Buffer(ref buf)) => {
                    if let Some(frozen) = lag.process_buffer(buf, || pad.current_caps()) {
                        info.data = Some(gst::PadProbeData::Buffer(frozen));
                    }
                }
                _ => {}
            }
            gst::PadProbeReturn::Ok
        },
    );
}

fn build_virtual_cam_sink(
    cfg: &Config,
    test_sources: bool,
) -> Result<(gst::Element, Option<String>), PipelineError> {
    if test_sources {
        let sink = make_element("fakesink", Some("virtual_cam_fake"))?;
        sink.set_property("sync", false);
        sink.set_property("async", false);
        return Ok((sink, None));
    }

    if gst::ElementFactory::find("v4l2sink").is_none() {
        let sink = make_element("fakesink", Some("virtual_cam_fallback"))?;
        sink.set_property("sync", false);
        sink.set_property("async", false);
        return Ok((
            sink,
            Some(
                "Нет плагина v4l2sink (установите gst-plugins-good) — работает только предпросмотр"
                    .into(),
            ),
        ));
    }

    if let Some(dev) = cfg
        .virtual_camera
        .as_deref()
        .filter(|p| Path::new(p).exists())
    {
        let sink = make_element("v4l2sink", Some("virtual_cam_sink"))?;
        sink.set_property("device", dev);
        sink.set_property("sync", false);
        sink.set_property("async", false);
        // Prevent GstBaseSink::last-sample from pinning an MMAP buffer from v4l2bufferpool,
        // which otherwise blocks REQBUFS(0) and causes v4l2loopback S_FMT to return EBUSY.
        sink.set_property("enable-last-sample", false);
        return Ok((sink, None));
    }

    let sink = make_element("fakesink", Some("virtual_cam_fallback"))?;
    sink.set_property("sync", false);
    sink.set_property("async", false);
    Ok((
        sink,
        Some(format!(
            "Устройство v4l2loopback не найдено. Выполните: {}",
            modprobe_hint()
        )),
    ))
}

fn common_video_caps(width: u32, height: u32, fps: u32) -> gst::Caps {
    // Pin format/PAR/interlace/colorimetry identically on both branches so switching
    // input-selector never renegotiates v4l2sink, and I420 -> YUY2 prevents videoconvert
    // passthrough from forwarding unsupported camera colorimetry to v4l2loopback.
    gst::Caps::builder("video/x-raw")
        .field("format", "I420")
        .field("width", width as i32)
        .field("height", height as i32)
        .field("framerate", gst::Fraction::new(fps as i32, 1))
        .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
        .field("interlace-mode", "progressive")
        .field("colorimetry", "bt709")
        .build()
}

fn strip_optional_caps_fields(caps: &gst::CapsRef) -> Option<gst::Caps> {
    const STRIP_FIELDS: &[&str] = &["multiview-mode", "multiview-flags", "chroma-site"];
    let s = caps.structure(0)?;
    if !STRIP_FIELDS.iter().any(|&f| s.has_field(f)) {
        return None;
    }
    let mut owned = caps.to_owned();
    if let Some(ms) = owned.make_mut().structure_mut(0) {
        ms.remove_fields(STRIP_FIELDS.iter().copied());
    }
    Some(owned)
}

fn install_selector_caps_normalizer(src_pad: &gst::Pad) {
    src_pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, |_pad, info| {
        if let Some(gst::PadProbeData::Event(ref ev)) = info.data
            && let gst::EventView::Caps(caps_ev) = ev.view()
            && let Some(normalized) = strip_optional_caps_fields(caps_ev.caps())
        {
            let new_ev = gst::event::Caps::builder(&normalized)
                .seqnum(ev.seqnum())
                .build();
            info.data = Some(gst::PadProbeData::Event(new_ev));
        }
        gst::PadProbeReturn::Ok
    });
}

fn path_to_uri(path: &Path) -> Result<String, PipelineError> {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    gst::glib::filename_to_uri(&abs, None)
        .map(|s| s.to_string())
        .map_err(|e| PipelineError::Link(format!("invalid MP4 path URI: {e}")))
}

fn make_element(factory: &'static str, name: Option<&str>) -> Result<gst::Element, PipelineError> {
    let mut builder = gst::ElementFactory::make(factory);
    if let Some(n) = name {
        builder = builder.name(n);
    }
    builder
        .build()
        .map_err(|_| PipelineError::MissingElement(factory))
}

fn make_capsfilter(name: &str, caps: &gst::Caps) -> Result<gst::Element, PipelineError> {
    let filter = make_element("capsfilter", Some(name))?;
    filter.set_property("caps", caps);
    Ok(filter)
}

#[derive(Default)]
pub struct SegmentRebaser {
    seek_gen: AtomicU64,
    rebase_base_ns: AtomicU64,
}

impl SegmentRebaser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_flushing_seek(&self, running_time_ns: u64) {
        self.rebase_base_ns
            .store(running_time_ns, Ordering::Relaxed);
        self.seek_gen.fetch_add(1, Ordering::Release);
    }

    pub fn rebase_if_needed(
        &self,
        seen_gen: &AtomicU64,
        seg: &gst::FormattedSegment<gst::ClockTime>,
    ) -> Option<gst::FormattedSegment<gst::ClockTime>> {
        let cur_gen = self.seek_gen.load(Ordering::Acquire);
        if cur_gen == 0 || cur_gen == seen_gen.load(Ordering::Relaxed) {
            return None;
        }
        seen_gen.store(cur_gen, Ordering::Relaxed);
        let base_ns = self.rebase_base_ns.load(Ordering::Relaxed);
        let mut rebased = seg.clone();
        rebased.set_base(gst::ClockTime::from_nseconds(base_ns));
        Some(rebased)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn segment_rebase_aligns_flushing_seek_without_touching_segment_done() {
        gst::init().unwrap();
        let rebaser = SegmentRebaser::new();
        let video_gen = AtomicU64::new(0);
        let audio_gen = AtomicU64::new(0);

        // Before any flushing seek, segment passes through untouched.
        let mut raw_seg = gst::FormattedSegment::<gst::ClockTime>::new();
        raw_seg.set_base(gst::ClockTime::ZERO);
        assert!(rebaser.rebase_if_needed(&video_gen, &raw_seg).is_none());

        // After flushing seek at running_time = 5s, BOTH video and audio rebase once to 5s.
        rebaser.mark_flushing_seek(5_000_000_000);
        let v_rebased = rebaser
            .rebase_if_needed(&video_gen, &raw_seg)
            .expect("video probe must rebase after flushing seek");
        let a_rebased = rebaser
            .rebase_if_needed(&audio_gen, &raw_seg)
            .expect("audio probe must rebase after flushing seek");
        assert_eq!(v_rebased.base(), Some(gst::ClockTime::from_seconds(5)));
        assert_eq!(a_rebased.base(), Some(gst::ClockTime::from_seconds(5)));

        // Subsequent non-flushing SegmentDone segment (same seek gen) stays untouched.
        assert!(rebaser.rebase_if_needed(&video_gen, &raw_seg).is_none());
        assert!(rebaser.rebase_if_needed(&audio_gen, &raw_seg).is_none());
    }

    #[test]
    fn common_caps_and_normalizer_produce_identical_selector_caps() {
        gst::init().unwrap();
        let canonical = common_video_caps(1280, 720, 30);
        let s = canonical.structure(0).unwrap();
        assert_eq!(s.get::<&str>("format").unwrap(), "I420");
        assert_eq!(
            s.get::<gst::Fraction>("pixel-aspect-ratio").unwrap(),
            gst::Fraction::new(1, 1)
        );
        assert_eq!(s.get::<&str>("interlace-mode").unwrap(), "progressive");
        assert_eq!(s.get::<&str>("colorimetry").unwrap(), "bt709");

        // No-op when optional metadata fields are absent.
        assert!(strip_optional_caps_fields(&canonical).is_none());

        // Decoder-added multiview/chroma metadata is stripped so both input-selector pads match.
        let mut with_meta = canonical.clone();
        let ms = with_meta.make_mut().structure_mut(0).unwrap();
        ms.set("multiview-mode", "mono");
        ms.set("chroma-site", "mpeg2");
        let stripped = strip_optional_caps_fields(&with_meta).expect("metadata must be stripped");
        assert!(stripped.is_equal_fixed(&canonical));
    }
}
