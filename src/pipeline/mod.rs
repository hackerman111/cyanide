pub mod audio;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("missing required GStreamer element '{0}'")]
    MissingElement(&'static str),
    #[error("failed to link GStreamer elements: {0}")]
    Link(String),
    #[error("GStreamer state change failed: {0}")]
    StateChange(String),
    #[error("GStreamer error: {0}")]
    Gst(#[from] gstreamer::glib::BoolError),
}
