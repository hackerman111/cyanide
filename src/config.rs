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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompareConfig {
    pub mode: CompareMode,
    pub opacity: f64,
    pub split_pos: f64,
    pub show_grid: bool,
    pub show_center: bool,
    pub show_guides: bool,
    pub guide_x: f64,
    pub guide_y: f64,
    pub mirror: bool,
    pub zoom: f64,
    pub pan_x: f64,
    pub pan_y: f64,
}

impl Default for CompareConfig {
    fn default() -> Self {
        Self {
            mode: CompareMode::Overlay,
            opacity: 0.5,
            split_pos: 0.5,
            show_grid: false,
            show_center: false,
            show_guides: false,
            guide_x: 0.5,
            guide_y: 0.5,
            mirror: false,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
        }
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub startup_mode: StartupMode,
    pub loop_file: Option<PathBuf>,
    pub real_camera: Option<String>,
    pub virtual_camera: Option<String>,
    pub real_mic: Option<String>,
    pub loop_volume: f64,
    pub live_volume: f64,
    pub av_delay_ms: i32,
    pub crossfade_ms: u32,
    pub video: VideoFormat,
    pub loop_return: LoopReturnPolicy,
    pub compare: CompareConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            startup_mode: StartupMode::Live,
            loop_file: None,
            real_camera: None,
            virtual_camera: None,
            real_mic: None,
            loop_volume: 1.0,
            live_volume: 1.0,
            av_delay_ms: 0,
            crossfade_ms: 20,
            video: VideoFormat::default(),
            loop_return: LoopReturnPolicy::Continue,
            compare: CompareConfig::default(),
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

        if self.real_mic.as_deref().is_some_and(is_forbidden_mic) {
            self.real_mic = None;
        }
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
        let encoded = toml::to_string_pretty(&cfg).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, cfg);
    }

    #[test]
    fn config_sanitizes_bounds_and_rejects_feedback_mic() {
        let mut cfg = Config::default();
        cfg.av_delay_ms = 9999;
        cfg.crossfade_ms = 2;
        cfg.loop_volume = -1.0;
        cfg.live_volume = 5.0;
        cfg.compare.opacity = 3.0;
        cfg.compare.zoom = 0.2;
        cfg.video.fps = 0;
        cfg.real_mic = Some("VirtualCamMic".into());
        let clean = cfg.sanitized();
        assert_eq!(clean.av_delay_ms, 500);
        assert_eq!(clean.crossfade_ms, 10);
        assert_eq!(clean.loop_volume, 0.0);
        assert_eq!(clean.live_volume, 1.5);
        assert_eq!(clean.compare.opacity, 1.0);
        assert_eq!(clean.compare.zoom, 1.0);
        assert_eq!(clean.video.fps, 30);
        assert_eq!(clean.real_mic, None);
    }

    #[test]
    fn config_load_invalid_toml_returns_error() {
        let path = std::env::temp_dir().join("cyanide-bad-config.toml");
        std::fs::write(&path, "not valid = [toml").unwrap();
        assert!(matches!(Config::load(&path), Err(ConfigError::Parse(_))));
        let _ = std::fs::remove_file(&path);
    }
}
