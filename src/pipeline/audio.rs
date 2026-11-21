use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gstreamer as gst;
use gstreamer::prelude::*;

use crate::config::{Config, is_forbidden_mic};
use crate::devices::VIRTUAL_SINK_NAME;
use crate::pipeline::video::SegmentRebaser;
use crate::pipeline::{ForceFallback, PipelineError};

pub const AUDIO_RATE: i32 = 48_000;
pub const AUDIO_CHANNELS: i32 = 2;
pub const MIN_GAIN: f64 = 0.0;
pub const MAX_GAIN: f64 = 1.5;

#[derive(Debug, Clone)]
pub struct GainRamp {
    start: (f64, f64),
    current: (f64, f64),
    target: (f64, f64),
    step: u32,
    total_steps: u32,
    tick_ms: u32,
}

impl GainRamp {
    pub fn new(start: (f64, f64), target: (f64, f64), duration_ms: u32, tick_ms: u32) -> Self {
        let tick = tick_ms.max(1);
        let clamped_start = clamp_pair(start);
        let clamped_target = clamp_pair(target);
        let total_steps = duration_ms.div_ceil(tick).max(1);
        Self {
            start: clamped_start,
            current: clamped_start,
            target: clamped_target,
            step: 0,
            total_steps,
            tick_ms: tick,
        }
    }

    pub fn step(&mut self) -> ((f64, f64), bool) {
        if self.step < self.total_steps {
            self.step += 1;
        }
        let done = self.step >= self.total_steps;
        let t = (self.step as f64) / (self.total_steps as f64);
        let v0 = lerp(self.start.0, self.target.0, t).clamp(MIN_GAIN, MAX_GAIN);
        let v1 = lerp(self.start.1, self.target.1, t).clamp(MIN_GAIN, MAX_GAIN);
        self.current = if done { self.target } else { (v0, v1) };
        (self.current, done)
    }

    pub fn retarget(&mut self, new_target: (f64, f64), duration_ms: u32) {
        self.start = self.current;
        self.target = clamp_pair(new_target);
        self.step = 0;
        self.total_steps = duration_ms.div_ceil(self.tick_ms).max(1);
    }
}

pub struct AudioBranch {
    pipeline: gst::glib::WeakRef<gst::Pipeline>,
    rebaser: Arc<SegmentRebaser>,
    pub loop_vol: gst::Element,
    pub live_vol: gst::Element,
    pub out_queue: gst::Element,
    pub loop_input_queue: gst::Element,
    pub mixer: gst::Element,
    loop_attached: AtomicBool,
    pub live_warning: Option<String>,
    pub output_warning: Option<String>,
}

impl AudioBranch {
    pub fn build(
        pipeline: &gst::Pipeline,
        cfg: &Config,
        test_sources: bool,
        fallback: ForceFallback,
        rebaser: Arc<SegmentRebaser>,
    ) -> Result<Self, PipelineError> {
        let audio_caps = canonical_audio_caps();

        // `force-live` is construct-only on GstAggregator subclasses like audiomixer.
        let mixer = gst::ElementFactory::make("audiomixer")
            .name("audio_mixer")
            .property("force-live", true)
            .property("latency", 20_000_000u64) // 20 ms
            .build()
            .map_err(|_| PipelineError::MissingElement("audiomixer"))?;

        // Always-live silence baseline keeps audiomixer ticking even if MP4 has no audio track.
        let silence_src = make_element("audiotestsrc", Some("silence_baseline"))?;
        silence_src.set_property_from_str("wave", "silence");
        silence_src.set_property("is-live", true);
        let silence_caps = make_capsfilter("silence_caps", &audio_caps)?;

        let (live_src, live_warning) =
            build_live_audio_source(cfg, test_sources, fallback.live_audio_silence)?;
        let live_queue = make_element("queue", Some("live_audio_queue"))?;
        let live_convert = make_element("audioconvert", Some("live_audio_convert"))?;
        let live_resample = make_element("audioresample", Some("live_audio_resample"))?;
        let live_caps = make_capsfilter("live_audio_caps", &audio_caps)?;
        let live_vol = make_element("volume", Some("live_vol"))?;
        let live_level = try_make_level("live_level");

        // Pre-create loop volume and input queue so gain changes work before/without an MP4 audio pad.
        let loop_input_queue = make_element("queue", Some("loop_audio_queue"))?;
        let loop_vol = make_element("volume", Some("loop_vol"))?;

        let out_level = try_make_level("out_level");
        let out_queue = make_element("queue", Some("audio_out_queue"))?;
        out_queue.set_property("max-size-time", 2_000_000_000u64);
        out_queue.set_property("max-size-buffers", 0u32);
        out_queue.set_property("max-size-bytes", 0u32);

        let (out_sink, output_warning) = build_audio_sink(test_sources)?;

        let mut elements: Vec<&gst::Element> = vec![
            &mixer,
            &silence_src,
            &silence_caps,
            &live_src,
            &live_queue,
            &live_convert,
            &live_resample,
            &live_caps,
            &live_vol,
            &out_queue,
            &out_sink,
        ];
        if let Some(ref lvl) = live_level {
            elements.push(lvl);
        }
        if let Some(ref lvl) = out_level {
            elements.push(lvl);
        }
        pipeline.add_many(&elements)?;

        gst::Element::link_many([&silence_src, &silence_caps, &mixer])
            .map_err(|e| PipelineError::Link(format!("silence -> mixer: {e}")))?;

        let mut live_chain: Vec<&gst::Element> = vec![
            &live_src,
            &live_queue,
            &live_convert,
            &live_resample,
            &live_caps,
            &live_vol,
        ];
        if let Some(ref lvl) = live_level {
            live_chain.push(lvl);
        }
        live_chain.push(&mixer);
        gst::Element::link_many(&live_chain)
            .map_err(|e| PipelineError::Link(format!("live audio -> mixer: {e}")))?;

        let mut out_chain: Vec<&gst::Element> = vec![&mixer];
        if let Some(ref lvl) = out_level {
            out_chain.push(lvl);
        }
        out_chain.push(&out_queue);
        out_chain.push(&out_sink);
        gst::Element::link_many(&out_chain)
            .map_err(|e| PipelineError::Link(format!("mixer -> audio sink: {e}")))?;

        Ok(Self {
            pipeline: pipeline.downgrade(),
            rebaser,
            loop_vol,
            live_vol,
            out_queue,
            loop_input_queue,
            mixer,
            loop_attached: AtomicBool::new(false),
            live_warning,
            output_warning,
        })
    }

    pub fn attach_loop_audio_pad(&self, pad: &gst::Pad) -> bool {
        if self.loop_attached.swap(true, Ordering::AcqRel) {
            return false;
        }
        let Some(pipeline) = self.pipeline.upgrade() else {
            self.loop_attached.store(false, Ordering::Release);
            return false;
        };

        let Ok(convert) = make_element("audioconvert", Some("loop_audio_convert")) else {
            self.loop_attached.store(false, Ordering::Release);
            return false;
        };
        let Ok(resample) = make_element("audioresample", Some("loop_audio_resample")) else {
            self.loop_attached.store(false, Ordering::Release);
            return false;
        };
        let Ok(caps) = make_capsfilter("loop_audio_caps", &canonical_audio_caps()) else {
            self.loop_attached.store(false, Ordering::Release);
            return false;
        };
        let level = try_make_level("loop_level");

        let mut added: Vec<&gst::Element> = vec![
            &self.loop_input_queue,
            &convert,
            &resample,
            &caps,
            &self.loop_vol,
        ];
        if let Some(ref lvl) = level {
            added.push(lvl);
        }
        if pipeline.add_many(&added).is_err() {
            self.loop_attached.store(false, Ordering::Release);
            return false;
        }

        let mut chain = added.clone();
        chain.push(&self.mixer);
        if gst::Element::link_many(&chain).is_err() {
            let _ = pipeline.remove_many(&added);
            self.loop_attached.store(false, Ordering::Release);
            return false;
        }

        // Rebase segment base after flushing seeks so audiomixer never drops post-seek audio,
        // drop audio during reverse ping-pong passes (silence_baseline keeps audiomixer ticking),
        // and drop upstream EOS so audiomixer keeps the loop pad open across restarts.
        if let Some(src_pad) = self.loop_input_queue.static_pad("src") {
            let rebaser = Arc::clone(&self.rebaser);
            let seen_gen = AtomicU64::new(0);
            let in_reverse = AtomicBool::new(false);
            src_pad.add_probe(
                gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_DOWNSTREAM,
                move |_pad, info| {
                    match info.data {
                        Some(gst::PadProbeData::Event(ref ev)) => match ev.view() {
                            gst::EventView::Segment(seg_ev) => {
                                if let Some(seg) = seg_ev.segment().downcast_ref::<gst::ClockTime>()
                                {
                                    if seg.rate() < 0.0 {
                                        in_reverse.store(true, Ordering::Relaxed);
                                        return gst::PadProbeReturn::Drop;
                                    }
                                    in_reverse.store(false, Ordering::Relaxed);
                                    if let Some(rebased) = rebaser.rebase_if_needed(&seen_gen, seg)
                                    {
                                        let new_ev = gst::event::Segment::builder(&rebased)
                                            .seqnum(ev.seqnum())
                                            .build();
                                        info.data = Some(gst::PadProbeData::Event(new_ev));
                                    }
                                }
                            }
                            gst::EventView::Eos(_) => {
                                return gst::PadProbeReturn::Drop;
                            }
                            _ => {}
                        },
                        Some(gst::PadProbeData::Buffer(_))
                            if in_reverse.load(Ordering::Relaxed) =>
                        {
                            return gst::PadProbeReturn::Drop;
                        }
                        _ => {}
                    }
                    gst::PadProbeReturn::Ok
                },
            );
        }

        for el in &added {
            let _ = el.sync_state_with_parent();
        }

        let Some(sink_pad) = self.loop_input_queue.static_pad("sink") else {
            return false;
        };
        pad.link(&sink_pad).is_ok()
    }

    pub fn set_gains(&self, loop_gain: f64, live_gain: f64) {
        self.loop_vol.set_property("volume", clamp_gain(loop_gain));
        self.live_vol.set_property("volume", clamp_gain(live_gain));
    }

    pub fn current_gains(&self) -> (f64, f64) {
        (
            self.loop_vol.property::<f64>("volume"),
            self.live_vol.property::<f64>("volume"),
        )
    }

    pub fn set_delay_ns(&self, audio_delay_ns: u64) {
        if let Some(pad) = self.out_queue.static_pad("src") {
            pad.set_offset(audio_delay_ns as i64);
        }
    }
}

pub fn parse_level_message(msg: &gst::MessageRef) -> Option<(String, f64)> {
    let structure = msg.structure()?;
    if structure.name() != "level" {
        return None;
    }
    let src_name = msg.src()?.name().to_string();
    let rms = structure.get::<gst::glib::ValueArray>("rms").ok()?;
    let max_db = rms
        .iter()
        .filter_map(|v| v.get::<f64>().ok())
        .filter(|db| db.is_finite())
        .fold(-60.0_f64, f64::max)
        .clamp(-60.0, 0.0);
    Some((src_name, max_db))
}

fn canonical_audio_caps() -> gst::Caps {
    gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("rate", AUDIO_RATE)
        .field("channels", AUDIO_CHANNELS)
        .build()
}

fn build_live_audio_source(
    cfg: &Config,
    test_sources: bool,
    force_silence: bool,
) -> Result<(gst::Element, Option<String>), PipelineError> {
    if force_silence {
        let src = make_element("audiotestsrc", Some("live_audio_fallback"))?;
        src.set_property_from_str("wave", "silence");
        src.set_property("is-live", true);
        return Ok((
            src,
            Some("Ошибка реального микрофона — используется тишина".into()),
        ));
    }

    if test_sources {
        let src = make_element("audiotestsrc", Some("live_audio_test"))?;
        src.set_property_from_str("wave", "silence");
        src.set_property("is-live", true);
        return Ok((src, None));
    }

    // Never bind unconstrained pulsesrc (@DEFAULT_SOURCE@): real_mic is None by default
    // so original microphone audio is not passed through unless explicitly selected.
    let Some(dev) = cfg
        .real_mic
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty() && !is_forbidden_mic(d))
    else {
        let src = make_element("audiotestsrc", Some("live_audio_fallback"))?;
        src.set_property_from_str("wave", "silence");
        src.set_property("is-live", true);
        return Ok((src, None));
    };

    if gst::ElementFactory::find("pulsesrc").is_none() {
        let src = make_element("audiotestsrc", Some("live_audio_fallback"))?;
        src.set_property_from_str("wave", "silence");
        src.set_property("is-live", true);
        return Ok((
            src,
            Some("Нет плагина pulsesrc (установите gst-plugins-good) — микрофон заглушён".into()),
        ));
    }

    let src = make_element("pulsesrc", Some("live_mic_src"))?;
    src.set_property("device", dev);
    Ok((src, None))
}

fn build_audio_sink(test_sources: bool) -> Result<(gst::Element, Option<String>), PipelineError> {
    if test_sources {
        let sink = make_element("fakesink", Some("audio_out_fake"))?;
        sink.set_property("sync", false);
        return Ok((sink, None));
    }

    if gst::ElementFactory::find("pulsesink").is_none() {
        let sink = make_element("fakesink", Some("audio_out_fallback"))?;
        sink.set_property("sync", false);
        return Ok((
            sink,
            Some(
                "Нет плагина pulsesink (установите gst-plugins-good) — вывод звука отключён".into(),
            ),
        ));
    }

    // Explicitly target VirtualCamAudio so virtual mic audio never routes to speakers.
    let sink = make_element("pulsesink", Some("virtual_mic_sink"))?;
    sink.set_property("device", VIRTUAL_SINK_NAME);
    Ok((sink, None))
}

fn try_make_level(name: &str) -> Option<gst::Element> {
    let lvl = gst::ElementFactory::make("level").name(name).build().ok()?;
    lvl.set_property("post-messages", true);
    lvl.set_property("interval", 50_000_000u64); // 50 ms
    Some(lvl)
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

fn clamp_gain(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(MIN_GAIN, MAX_GAIN)
    } else {
        0.0
    }
}

fn clamp_pair(p: (f64, f64)) -> (f64, f64) {
    (clamp_gain(p.0), clamp_gain(p.1))
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_ramp_interpolates_and_retargets_cleanly() {
        let mut ramp = GainRamp::new((0.0, 1.0), (1.0, 0.0), 20, 5);
        let mut last = (0.0, 1.0);
        for expected_step in 1..=4 {
            let (vals, done) = ramp.step();
            assert!((vals.0 - 0.25 * expected_step as f64).abs() < 1e-9);
            assert!((vals.1 - (1.0 - 0.25 * expected_step as f64)).abs() < 1e-9);
            assert_eq!(done, expected_step == 4);
            last = vals;
        }
        assert_eq!(last, (1.0, 0.0));

        // Mid-ramp retargeting preserves continuity and clamps bounds
        let mut mid = GainRamp::new((0.0, 1.0), (1.0, 0.0), 20, 5);
        let (v1, _) = mid.step(); // (0.25, 0.75)
        mid.retarget((0.0, 2.0), 10); // clamps 2.0 -> 1.5
        let (v2, done2) = mid.step();
        assert!((v2.0 - v1.0 * 0.5).abs() < 1e-9);
        assert!(v2.1 > v1.1 && v2.1 <= 1.5);
        assert!(!done2);
        let (v3, done3) = mid.step();
        assert_eq!(v3, (0.0, 1.5));
        assert!(done3);
    }

    #[test]
    fn audio_branch_runs_with_silence_fallback_when_no_mp4_audio() {
        gst::init().unwrap();
        let pipeline = gst::Pipeline::new();
        let cfg = Config::default();
        let branch = AudioBranch::build(
            &pipeline,
            &cfg,
            true,
            ForceFallback::default(),
            Arc::new(SegmentRebaser::new()),
        )
        .unwrap();
        branch.set_gains(0.8, 0.0);
        branch.set_delay_ns(50_000_000);
        pipeline.set_state(gst::State::Playing).unwrap();
        let (res, state, _) = pipeline.state(gst::ClockTime::from_seconds(2));
        assert!(res.is_ok() && state == gst::State::Playing);
        pipeline.set_state(gst::State::Null).unwrap();
    }

    #[test]
    fn audio_branch_refuses_unconstrained_default_source_when_real_mic_none() {
        gst::init().unwrap();
        let pipeline = gst::Pipeline::new();
        let cfg = Config {
            real_mic: None,
            ..Default::default()
        };
        let branch = AudioBranch::build(
            &pipeline,
            &cfg,
            false,
            ForceFallback::default(),
            Arc::new(SegmentRebaser::new()),
        )
        .unwrap();
        assert!(pipeline.by_name("live_mic_src").is_none());
        assert!(pipeline.by_name("live_audio_fallback").is_some());
        assert!(
            branch.live_warning.is_none(),
            "intentional disabled real_mic should not emit a warning banner, got {:?}",
            branch.live_warning
        );
    }
}
