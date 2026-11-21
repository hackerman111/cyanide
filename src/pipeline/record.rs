use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gstreamer as gst;
use gstreamer::prelude::*;

use crate::pipeline::PipelineError;

#[derive(Debug, Clone)]
pub struct RecordingFinished {
    pub path: PathBuf,
    pub duration_ms: u64,
    pub frame_count: u64,
}

pub struct ActiveRecording {
    pipeline: gst::Pipeline,
    appsrc: gst::Element,
    path: PathBuf,
    started: Instant,
    caps_set: bool,
    frame_duration_ns: u64,
    last_pts_ns: Option<u64>,
    frame_count: u64,
}

impl ActiveRecording {
    /// Finalizes the MP4 container by pushing EOS and waiting for mp4mux to write the moov atom.
    /// Runs synchronously and should be executed off the GTK main thread.
    pub fn finalize(self) -> Result<RecordingFinished, String> {
        let duration_ms = self.started.elapsed().as_millis() as u64;
        if self.frame_count == 0 {
            let _ = self.pipeline.set_state(gst::State::Null);
            let _ = std::fs::remove_file(&self.path);
            return Err("Запись не содержит кадров".into());
        }

        let _ = self
            .appsrc
            .emit_by_name::<gst::FlowReturn>("end-of-stream", &[]);

        let mut error: Option<String> = None;
        if let Some(bus) = self.pipeline.bus() {
            match bus.timed_pop_filtered(
                gst::ClockTime::from_seconds(5),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            ) {
                Some(msg) => match msg.view() {
                    gst::MessageView::Eos(_) => {}
                    gst::MessageView::Error(err) => {
                        error = Some(format!("Ошибка записи MP4: {}", err.error()));
                    }
                    _ => {}
                },
                None => {
                    error = Some("Таймаут финализации MP4".into());
                }
            }
        }

        let _ = self.pipeline.set_state(gst::State::Null);

        if let Some(err) = error {
            return Err(err);
        }

        Ok(RecordingFinished {
            path: self.path,
            duration_ms,
            frame_count: self.frame_count,
        })
    }
}

/// Records frames from the live video branch into an isolated GStreamer encoding pipeline
/// so recording errors or disk stalls never disrupt the live broadcast.
pub struct VideoRecorder {
    recording: AtomicBool,
    inner: Mutex<Option<ActiveRecording>>,
}

impl Default for VideoRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoRecorder {
    pub fn new() -> Self {
        Self {
            recording: AtomicBool::new(false),
            inner: Mutex::new(None),
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Acquire)
    }

    pub fn elapsed_ms(&self) -> Option<u64> {
        if !self.is_recording() {
            return None;
        }
        self.inner
            .lock()
            .ok()?
            .as_ref()
            .map(|rec| rec.started.elapsed().as_millis() as u64)
    }

    pub fn start(&self, path: PathBuf, fps: u32) -> Result<PathBuf, PipelineError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| PipelineError::StateChange("recorder mutex poisoned".into()))?;
        if guard.is_some() {
            return Err(PipelineError::StateChange(
                "recording already active".into(),
            ));
        }

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| PipelineError::Link(format!("create recording dir: {e}")))?;
        }

        let (pipeline, appsrc) = build_recording_pipeline(&path)?;
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| PipelineError::StateChange(format!("recorder start failed: {e:?}")))?;

        let frame_duration_ns = 1_000_000_000u64 / (fps.max(1) as u64);
        *guard = Some(ActiveRecording {
            pipeline,
            appsrc,
            path: path.clone(),
            started: Instant::now(),
            caps_set: false,
            frame_duration_ns,
            last_pts_ns: None,
            frame_count: 0,
        });
        self.recording.store(true, Ordering::Release);
        Ok(path)
    }

    /// Hot-path hook called from the live video pad probe.
    /// Does a single atomic check when idle and deep-copies frames when active to release v4l2 pools.
    pub fn push_buffer(&self, buf: &gst::BufferRef, caps: &gst::Caps) {
        if !self.recording.load(Ordering::Acquire) {
            return;
        }
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        let Some(ref mut rec) = *guard else {
            return;
        };

        if !rec.caps_set {
            rec.appsrc.set_property("caps", caps);
            rec.caps_set = true;
        }

        let Ok(mut copy) = buf.copy_deep() else {
            return;
        };
        let raw_ns = rec.started.elapsed().as_nanos() as u64;
        let pts_ns = match rec.last_pts_ns {
            Some(prev) => raw_ns.max(prev.saturating_add(1)),
            None => 0,
        };
        rec.last_pts_ns = Some(pts_ns);

        if let Some(buf_mut) = copy.get_mut() {
            buf_mut.set_pts(gst::ClockTime::from_nseconds(pts_ns));
            buf_mut.set_dts(gst::ClockTime::NONE);
            buf_mut.set_duration(gst::ClockTime::from_nseconds(rec.frame_duration_ns));
        }

        let flow = rec
            .appsrc
            .emit_by_name::<gst::FlowReturn>("push-buffer", &[&copy]);
        if flow == gst::FlowReturn::Ok {
            rec.frame_count += 1;
        }
    }

    /// Detaches the active recording session immediately so finalization can run off-thread.
    pub fn take_active(&self) -> Option<ActiveRecording> {
        self.recording.store(false, Ordering::Release);
        self.inner.lock().ok()?.take()
    }
}

pub fn default_recording_path() -> PathBuf {
    let dir = std::env::var_os("XDG_VIDEOS_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Videos")))
        .unwrap_or_else(|| std::env::temp_dir().join("cyanide-recordings"))
        .join("cyanide");
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, d, hh, mm, ss) = format_utc_components(secs);
    dir.join(format!(
        "cyanide-{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}.mp4"
    ))
}

fn build_recording_pipeline(path: &Path) -> Result<(gst::Pipeline, gst::Element), PipelineError> {
    let pipeline = gst::Pipeline::with_name("cyanide_recorder");

    let appsrc = make_element("appsrc", Some("rec_appsrc"))?;
    appsrc.set_property("is-live", true);
    appsrc.set_property("format", gst::Format::Time);
    appsrc.set_property("block", false);
    appsrc.set_property("do-timestamp", false);

    let queue = make_element("queue", Some("rec_queue"))?;
    queue.set_property_from_str("leaky", "downstream");
    queue.set_property("max-size-buffers", 30u32);
    queue.set_property("max-size-bytes", 0u32);
    queue.set_property("max-size-time", 0u64);

    let convert = make_element("videoconvert", Some("rec_convert"))?;
    let i420_caps = gst::Caps::builder("video/x-raw")
        .field("format", "I420")
        .build();
    let capsfilter = make_element("capsfilter", Some("rec_i420_caps"))?;
    capsfilter.set_property("caps", &i420_caps);

    let encoder = build_h264_encoder()?;
    let parse = make_element("h264parse", Some("rec_h264parse"))?;
    let mux = make_element("mp4mux", Some("rec_mp4mux"))?;

    let sink = make_element("filesink", Some("rec_filesink"))?;
    let loc = path
        .to_str()
        .ok_or_else(|| PipelineError::Link("non-UTF8 recording path".into()))?;
    sink.set_property("location", loc);
    sink.set_property("sync", false);

    pipeline.add_many([
        &appsrc,
        &queue,
        &convert,
        &capsfilter,
        &encoder,
        &parse,
        &mux,
        &sink,
    ])?;

    gst::Element::link_many([
        &appsrc,
        &queue,
        &convert,
        &capsfilter,
        &encoder,
        &parse,
        &mux,
        &sink,
    ])
    .map_err(|e| PipelineError::Link(format!("recording chain: {e}")))?;

    Ok((pipeline, appsrc))
}

fn build_h264_encoder() -> Result<gst::Element, PipelineError> {
    if gst::ElementFactory::find("x264enc").is_some() {
        let enc = make_element("x264enc", Some("rec_x264enc"))?;
        enc.set_property_from_str("tune", "zerolatency");
        enc.set_property_from_str("speed-preset", "ultrafast");
        // Short GOP with zero B-frames keeps both seeking and reverse ping-pong playback smooth.
        enc.set_property("key-int-max", 15u32);
        enc.set_property("bitrate", 4000u32);
        return Ok(enc);
    }
    if gst::ElementFactory::find("openh264enc").is_some() {
        let enc = make_element("openh264enc", Some("rec_openh264enc"))?;
        enc.set_property("gop-size", 15u32);
        return Ok(enc);
    }
    Err(PipelineError::MissingElement("x264enc"))
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

/// Converts Unix timestamp seconds into (year, month, day, hour, minute, second) UTC.
fn format_utc_components(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as u32;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;

    // Howard Hinnant's civil-from-days algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = y + i64::from(m <= 2);
    (year, m, d, hh, mm, ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_components_match_known_epoch_dates() {
        assert_eq!(format_utc_components(0), (1970, 1, 1, 0, 0, 0));
        // 2024-02-29 12:34:56 UTC (leap day)
        assert_eq!(
            format_utc_components(1_709_210_096),
            (2024, 2, 29, 12, 34, 56)
        );
    }

    #[test]
    fn recorder_writes_valid_mp4_from_synthetic_frames() {
        gst::init().unwrap();
        let path =
            std::env::temp_dir().join(format!("cyanide-rec-test-{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let recorder = VideoRecorder::new();
        recorder.start(path.clone(), 30).unwrap();
        assert!(recorder.is_recording());

        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "I420")
            .field("width", 160i32)
            .field("height", 120i32)
            .field("framerate", gst::Fraction::new(30, 1))
            .build();
        let frame_bytes = 160 * 120 * 3 / 2;
        for i in 0..20u8 {
            let mut buf = gst::Buffer::with_size(frame_bytes).unwrap();
            {
                let mut map = buf.get_mut().unwrap().map_writable().unwrap();
                map.as_mut_slice().fill(i.wrapping_mul(10));
            }
            recorder.push_buffer(buf.as_ref(), &caps);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let active = recorder.take_active().expect("active recording");
        assert!(!recorder.is_recording());
        let finished = active.finalize().expect("mp4 finalization must succeed");
        assert_eq!(finished.path, path);
        assert!(finished.frame_count >= 15);
        let meta = std::fs::metadata(&path).expect("mp4 file must exist");
        assert!(meta.len() > 500, "mp4 too small: {} bytes", meta.len());

        let _ = std::fs::remove_file(&path);
    }
}
