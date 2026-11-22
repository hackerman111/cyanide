use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("failed to serialize config TOML: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartupMode {
    #[default]
    Live,
    Loop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoFormat {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for VideoFormat {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareMode {
    #[default]
    Overlay,
    SideBySide,
    Split,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareTarget {
    #[default]
    Both,
    Loop,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerTransform {
    pub zoom: f64,
    pub pan_x: f64,
    pub pan_y: f64,
    pub mirror: bool,
}

impl Default for LayerTransform {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            mirror: false,
        }
    }
}

impl LayerTransform {
    pub fn sanitized(mut self) -> Self {
        self.zoom = clamp_finite(self.zoom, 0.5, 3.0, 1.0);
        self.pan_x = clamp_finite(self.pan_x, -1.0, 1.0, 0.0);
        self.pan_y = clamp_finite(self.pan_y, -1.0, 1.0, 0.0);
        self
    }

    pub fn is_identity(&self) -> bool {
        (self.zoom - 1.0).abs() <= 0.001
            && self.pan_x.abs() <= 0.001
            && self.pan_y.abs() <= 0.001
            && !self.mirror
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompareConfig {
    pub mode: CompareMode,
    pub opacity: f64,
    pub split_pos: f64,
    pub split_vertical: bool,
    pub show_grid: bool,
    pub show_center: bool,
    pub show_guides: bool,
    pub similarity: bool,
    pub guide_x: f64,
    pub guide_y: f64,
    pub target: CompareTarget,
    pub mirror: bool,
    pub zoom: f64,
    pub pan_x: f64,
    pub pan_y: f64,
    pub loop_transform: LayerTransform,
    pub live_transform: LayerTransform,
}

impl Default for CompareConfig {
    fn default() -> Self {
        Self {
            mode: CompareMode::Overlay,
            opacity: 0.5,
            split_pos: 0.5,
            split_vertical: true,
            show_grid: false,
            show_center: false,
            show_guides: false,
            similarity: false,
            guide_x: 0.5,
            guide_y: 0.5,
            target: CompareTarget::Both,
            mirror: false,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            loop_transform: LayerTransform::default(),
            live_transform: LayerTransform::default(),
        }
    }
}

impl CompareConfig {
    pub fn active_transform(&self) -> (f64, f64, f64, bool) {
        match self.target {
            CompareTarget::Both => (self.zoom, self.pan_x, self.pan_y, self.mirror),
            CompareTarget::Loop => (
                self.loop_transform.zoom,
                self.loop_transform.pan_x,
                self.loop_transform.pan_y,
                self.loop_transform.mirror,
            ),
            CompareTarget::Live => (
                self.live_transform.zoom,
                self.live_transform.pan_x,
                self.live_transform.pan_y,
                self.live_transform.mirror,
            ),
        }
    }

    pub fn set_active_transform(&mut self, zoom: f64, pan_x: f64, pan_y: f64, mirror: bool) {
        match self.target {
            CompareTarget::Both => {
                self.zoom = zoom.clamp(1.0, 3.0);
                self.pan_x = pan_x.clamp(-1.0, 1.0);
                self.pan_y = pan_y.clamp(-1.0, 1.0);
                self.mirror = mirror;
            }
            CompareTarget::Loop => {
                self.loop_transform = LayerTransform {
                    zoom,
                    pan_x,
                    pan_y,
                    mirror,
                }
                .sanitized();
            }
            CompareTarget::Live => {
                self.live_transform = LayerTransform {
                    zoom,
                    pan_x,
                    pan_y,
                    mirror,
                }
                .sanitized();
            }
        }
    }

    pub fn reset_transforms(&mut self) {
        self.zoom = 1.0;
        self.pan_x = 0.0;
        self.pan_y = 0.0;
        self.mirror = false;
        self.loop_transform = LayerTransform::default();
        self.live_transform = LayerTransform::default();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiLayoutConfig {
    pub window_width: i32,
    pub window_height: i32,
    pub split_position: i32,
    pub sidebar_visible: bool,
}

impl Default for UiLayoutConfig {
    fn default() -> Self {
        Self {
            window_width: 1366,
            window_height: 768,
            split_position: 860,
            sidebar_visible: true,
        }
    }
}

impl UiLayoutConfig {
    pub fn sanitized(mut self) -> Self {
        self.window_width = self.window_width.clamp(960, 3840);
        self.window_height = self.window_height.clamp(620, 2160);
        self.split_position = self.split_position.clamp(380, 3200);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum LoopReturnPolicy {
    #[default]
    Continue,
    Restart,
    Bookmark {
        position_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LagConfig {
    pub enabled: bool,
    pub freeze_on_switch: bool,
    pub interval_ms: u32,
    pub min_duration_ms: u32,
    pub max_duration_ms: u32,
}

impl Default for LagConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            freeze_on_switch: false,
            interval_ms: 3000,
            min_duration_ms: 100,
            max_duration_ms: 500,
        }
    }
}

impl LagConfig {
    pub fn sanitized(mut self) -> Self {
        self.interval_ms = self.interval_ms.clamp(500, 15_000);
        self.min_duration_ms = self.min_duration_ms.clamp(100, 5_000);
        self.max_duration_ms = self.max_duration_ms.clamp(self.min_duration_ms, 5_000);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub startup_mode: StartupMode,
    pub loop_file: Option<PathBuf>,
    pub real_camera: Option<String>,
    pub virtual_camera: Option<String>,
    pub real_mic: Option<String>,
    pub hotkey: Option<String>,
    pub loop_volume: f64,
    pub live_volume: f64,
    pub av_delay_ms: i32,
    pub crossfade_ms: u32,
    pub video: VideoFormat,
    pub loop_return: LoopReturnPolicy,
    pub loop_ping_pong: bool,
    pub compare: CompareConfig,
    pub lag: LagConfig,
    pub ui: UiLayoutConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            startup_mode: StartupMode::Live,
            loop_file: None,
            real_camera: None,
            virtual_camera: None,
            real_mic: None,
            hotkey: None,
            loop_volume: 1.0,
            live_volume: 1.0,
            av_delay_ms: 0,
            crossfade_ms: 20,
            video: VideoFormat::default(),
            loop_return: LoopReturnPolicy::Continue,
            loop_ping_pong: false,
            compare: CompareConfig::default(),
            lag: LagConfig::default(),
            ui: UiLayoutConfig::default(),
        }
    }
}

impl Config {
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("cyanide").join("config.toml")
    }

    pub fn sanitized(mut self) -> Self {
        self.av_delay_ms = self.av_delay_ms.clamp(-500, 500);
        self.crossfade_ms = self.crossfade_ms.clamp(10, 30);
        self.loop_volume = clamp_finite(self.loop_volume, 0.0, 1.5, 1.0);
        self.live_volume = clamp_finite(self.live_volume, 0.0, 1.5, 1.0);

        if self.video.width == 0 || self.video.height == 0 || self.video.fps == 0 {
            self.video = VideoFormat::default();
        }

        self.compare.opacity = clamp_finite(self.compare.opacity, 0.0, 1.0, 0.5);
        self.compare.split_pos = clamp_finite(self.compare.split_pos, 0.05, 0.95, 0.5);
        self.compare.guide_x = clamp_finite(self.compare.guide_x, 0.0, 1.0, 0.5);
        self.compare.guide_y = clamp_finite(self.compare.guide_y, 0.0, 1.0, 0.5);
        self.compare.zoom = clamp_finite(self.compare.zoom, 1.0, 3.0, 1.0);
        self.compare.pan_x = clamp_finite(self.compare.pan_x, -1.0, 1.0, 0.0);
        self.compare.pan_y = clamp_finite(self.compare.pan_y, -1.0, 1.0, 0.0);
        self.compare.loop_transform = self.compare.loop_transform.sanitized();
        self.compare.live_transform = self.compare.live_transform.sanitized();
        self.ui = self.ui.sanitized();

        if self.real_mic.as_deref().is_some_and(is_forbidden_mic) {
            self.real_mic = None;
        }
        self.hotkey = self
            .hotkey
            .as_deref()
            .and_then(|s| crate::hotkey::Hotkey::parse(s).ok())
            .map(|h| h.canonical());
        self.lag = self.lag.sanitized();
        self
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path)?;
        let cfg: Self = toml::from_str(&raw)?;
        Ok(cfg.sanitized())
    }

    pub fn load_or_default(path: &Path) -> Result<Self, ConfigError> {
        match Self::load(path) {
            Ok(cfg) => Ok(cfg),
            Err(ConfigError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(Self::default())
            }
            Err(err) => Err(err),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let encoded = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, encoded)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

fn clamp_finite(value: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

pub fn is_forbidden_mic(name: &str) -> bool {
    name == "VirtualCamMic" || name.ends_with(".monitor")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_toml_roundtrip() {
        let cfg = Config::default();
        assert_eq!(cfg.startup_mode, StartupMode::Live);
        assert_eq!(cfg.video.width, 1280);
        assert_eq!(cfg.video.height, 720);
        assert_eq!(cfg.video.fps, 30);
        assert_eq!(cfg.crossfade_ms, 20);
        assert_eq!(cfg.av_delay_ms, 0);
        assert_eq!(cfg.real_mic, None);
        assert!(!cfg.loop_ping_pong);
        assert!(!cfg.compare.similarity);
        assert!(!cfg.lag.enabled);
        assert!(!cfg.lag.freeze_on_switch);
        assert_eq!(cfg.lag.min_duration_ms, 100);
        assert_eq!(cfg.lag.max_duration_ms, 500);
        let encoded = toml::to_string_pretty(&cfg).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, cfg);
    }

    #[test]
    fn config_sanitizes_bounds_and_rejects_feedback_mic() {
        let cfg = Config {
            av_delay_ms: 9999,
            crossfade_ms: 2,
            loop_volume: -1.0,
            live_volume: 5.0,
            video: VideoFormat {
                fps: 0,
                ..Default::default()
            },
            compare: CompareConfig {
                opacity: 3.0,
                zoom: 0.2,
                live_transform: LayerTransform {
                    zoom: 0.1,
                    pan_x: 5.0,
                    pan_y: -3.0,
                    mirror: true,
                },
                ..Default::default()
            },
            ui: UiLayoutConfig {
                window_width: 100,
                window_height: 9999,
                split_position: 50,
                sidebar_visible: false,
            },
            real_mic: Some("VirtualCamMic".into()),
            hotkey: Some("shift+super+f8".into()),
            lag: LagConfig {
                enabled: true,
                freeze_on_switch: true,
                interval_ms: 10,
                min_duration_ms: 2000,
                max_duration_ms: 500,
            },
            ..Default::default()
        };
        let clean = cfg.sanitized();
        assert_eq!(clean.av_delay_ms, 500);
        assert_eq!(clean.crossfade_ms, 10);
        assert_eq!(clean.loop_volume, 0.0);
        assert_eq!(clean.live_volume, 1.5);
        assert_eq!(clean.compare.opacity, 1.0);
        assert_eq!(clean.compare.zoom, 1.0);
        assert_eq!(clean.compare.live_transform.zoom, 0.5);
        assert_eq!(clean.compare.live_transform.pan_x, 1.0);
        assert_eq!(clean.compare.live_transform.pan_y, -1.0);
        assert_eq!(clean.ui.window_width, 960);
        assert_eq!(clean.ui.window_height, 2160);
        assert_eq!(clean.ui.split_position, 380);
        assert!(!clean.ui.sidebar_visible);
        assert_eq!(clean.video.fps, 30);
        assert_eq!(clean.real_mic, None);
        assert_eq!(clean.hotkey.as_deref(), Some("SUPER+SHIFT+F8"));
        assert_eq!(clean.lag.interval_ms, 500);
        assert_eq!(clean.lag.min_duration_ms, 2000);
        assert_eq!(clean.lag.max_duration_ms, 2000);
    }

    #[test]
    fn config_load_invalid_toml_returns_error() {
        let path = std::env::temp_dir().join("cyanide-bad-config.toml");
        std::fs::write(&path, "not valid = [toml").unwrap();
        assert!(matches!(Config::load(&path), Err(ConfigError::Parse(_))));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn config_parses_legacy_toml_without_ui_or_layer_fields() {
        let legacy = r#"
startup_mode = "loop"
[compare]
mode = "split"
opacity = 0.7
"#;
        let cfg: Config = toml::from_str(legacy).unwrap();
        let clean = cfg.sanitized();
        assert_eq!(clean.startup_mode, StartupMode::Loop);
        assert_eq!(clean.compare.mode, CompareMode::Split);
        assert!(clean.compare.split_vertical);
        assert_eq!(clean.compare.target, CompareTarget::Both);
        assert_eq!(clean.ui, UiLayoutConfig::default());
    }
}
