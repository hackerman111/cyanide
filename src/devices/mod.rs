pub mod pipewire;
pub mod v4l2;

use std::path::PathBuf;

pub use pipewire::{AudioDevice, PipeWireError, VirtualMicGuard};
pub use v4l2::{V4l2Inventory, VideoDevice};

pub const VIRTUAL_SINK_NAME: &str = "VirtualCamAudio";
pub const VIRTUAL_MIC_NAME: &str = "VirtualCamMic";

#[derive(Debug, Clone, Default)]
pub struct DeviceInventory {
    pub real_cameras: Vec<VideoDevice>,
    pub loopback_cameras: Vec<VideoDevice>,
    pub real_mics: Vec<AudioDevice>,
}

pub fn discover_all() -> DeviceInventory {
    let cams = v4l2::discover_cameras();
    let real_mics = pipewire::discover_real_mics().unwrap_or_default();
    DeviceInventory {
        real_cameras: cams.real_cameras,
        loopback_cameras: cams.loopback_cameras,
        real_mics,
    }
}

pub fn default_pw_state_file() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("cyanide-pw-modules")
}

pub fn modprobe_hint() -> &'static str {
    "sudo modprobe v4l2loopback devices=1 video_nr=10 card_label=\"VirtualCam\" exclusive_caps=1"
}
