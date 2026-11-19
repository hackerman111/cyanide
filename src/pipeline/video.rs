use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gstreamer as gst;
use gstreamer::prelude::*;

use crate::config::Config;
use crate::devices::modprobe_hint;
use crate::pipeline::audio::AudioBranch;
use crate::pipeline::{ForceFallback, PipelineError, PreviewBackend, PreviewObjects};
use crate::state::VideoSource;

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
        )?;

        let (live_valve, live_sink, live_warning) = build_live_branch(
            pipeline,
            cfg,
            &common_caps,
            backend,
            test_sources,
            fallback.live_black,
            &live_sel_pad,
        )?;

        let out_queue = make_element("queue", Some("video_out_queue"))?;
        out_queue.set_property("max-size-time", 2_000_000_000u64);
        out_queue.set_property("max-size-buffers", 0u32);
        out_queue.set_property("max-size-bytes", 0u32);

        let out_tee = make_element("tee", Some("out_tee"))?;
        out_tee.set_property("allow-not-linked", true);

        let out_broadcast_queue = make_element("queue", Some("out_broadcast_queue"))?;
        let out_convert = make_element("videoconvert", Some("out_v4l2_convert"))?;
        let yuy2_caps = gst::Caps::builder("video/x-raw")
            .field("format", "YUY2")
            .field("width", cfg.video.width as i32)
            .field("height", cfg.video.height as i32)
            .field("framerate", gst::Fraction::new(cfg.video.fps as i32, 1))
            .build();
        let out_caps = make_capsfilter("out_v4l2_caps", &yuy2_caps)?;

        let (broadcast_sink, output_warning) = build_virtual_cam_sink(cfg, test_sources)?;

        pipeline.add_many([
            &out_queue,
            &out_tee,
            &out_broadcast_queue,
            &out_convert,
            &out_caps,
            &broadcast_sink,
        ])?;

        gst::Element::link_many([
            &selector,
            &out_queue,
            &out_tee,
            &out_broadcast_queue,
            &out_convert,
            &out_caps,
            &broadcast_sink,
        ])
        .map_err(|e| PipelineError::Link(format!("video_sel -> broadcast: {e}")))?;

        let (out_valve, out_sink) = attach_preview_leg(pipeline, &out_tee, "out", backend)?;

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

    pub fn seek_loop(&self, position_ms: u64, flushing: bool) -> bool {
        let Some(ref src) = self.loop_source else {
            return false;
        };
        let mut flags = gst::SeekFlags::SEGMENT | gst::SeekFlags::ACCURATE;
        if flushing {
            flags |= gst::SeekFlags::FLUSH;
        }
        src.seek(
            1.0,
            flags,
            gst::SeekType::Set,
            gst::ClockTime::from_mseconds(position_ms),
            gst::SeekType::None,
            gst::ClockTime::NONE,
        )
        .is_ok()
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
    let tee = make_element("tee", Some("loop_tee"))?;
    tee.set_property("allow-not-linked", true);
    let sel_queue = make_element("queue", Some("loop_sel_queue"))?;

    pipeline.add_many([&queue, &convert, &scale, &rate, &caps, &tee, &sel_queue])?;
    gst::Element::link_many([&queue, &convert, &scale, &rate, &caps, &tee, &sel_queue])
        .map_err(|e| PipelineError::Link(format!("loop video chain: {e}")))?;

    let src_pad = sel_queue
        .static_pad("src")
        .ok_or_else(|| PipelineError::Link("loop_sel_queue missing src pad".into()))?;
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
        && !test_sources
        && let Some(path) = cfg.loop_file.as_deref()
        && path.exists()
        && let Ok(uri) = path_to_uri(path)
    {
        let decodebin = make_element("uridecodebin", Some("loop_uridecodebin"))?;
        decodebin.set_property("uri", uri.as_str());
        pipeline.add(&decodebin)?;

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
    src_pad
        .link(sel_pad)
        .map_err(|e| PipelineError::Link(format!("live -> video_sel: {e}")))?;

    let (valve, preview_sink) = attach_preview_leg(pipeline, &tee, "live", backend)?;
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
    gst::Caps::builder("video/x-raw")
        .field("width", width as i32)
        .field("height", height as i32)
        .field("framerate", gst::Fraction::new(fps as i32, 1))
        .build()
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
