use serde::{Deserialize, Serialize};

use crate::config::{Config, StartupMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoSource {
    Loop,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioSource {
    File,
    Mic,
    Mix,
    Silence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum StreamMode {
    Loop,
    Live,
    Custom {
        video: VideoSource,
        audio: AudioSource,
    },
}

impl StreamMode {
    pub fn video(self) -> VideoSource {
        match self {
            Self::Loop => VideoSource::Loop,
            Self::Live => VideoSource::Live,
            Self::Custom { video, .. } => video,
        }
    }

    pub fn audio(self) -> AudioSource {
        match self {
            Self::Loop => AudioSource::File,
            Self::Live => AudioSource::Mic,
            Self::Custom { audio, .. } => audio,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MicAction {
    On,
    Off,
    Toggle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Toggle,
    SetMode(StreamMode),
    SetVideo(VideoSource),
    SetAudio(AudioSource),
    SetMic(MicAction),
    SetFreeze { frozen: bool },
    Show,
    Hide,
    Status,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AudioLevels {
    pub loop_db: f64,
    pub live_db: f64,
    pub out_db: f64,
}

impl Default for AudioLevels {
    fn default() -> Self {
        Self {
            loop_db: -60.0,
            live_db: -60.0,
            out_db: -60.0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceHealth {
    pub loop_warning: Option<String>,
    pub live_warning: Option<String>,
    pub output_warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub mode: StreamMode,
    pub video: VideoSource,
    pub audio: AudioSource,
    pub virtual_mic_enabled: bool,
    pub frozen_reference: bool,
    pub startup_mode: StartupMode,
    pub av_delay_ms: i32,
    pub health: SourceHealth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Reply {
    Ok { message: String },
    Status(StatusSnapshot),
    Error { message: String },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StateEffect {
    pub video_changed: bool,
    pub audio_changed: bool,
    pub video_switched_to_loop: bool,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub mode: StreamMode,
    pub virtual_mic_enabled: bool,
    pub frozen_reference: bool,
    pub levels: AudioLevels,
    pub health: SourceHealth,
    pub loop_pos_ms: u64,
    pub loop_dur_ms: Option<u64>,
}

impl AppState {
    pub fn from_config(cfg: &Config) -> Self {
        let mode = match cfg.startup_mode {
            StartupMode::Live => StreamMode::Live,
            StartupMode::Loop => StreamMode::Loop,
        };
        Self {
            mode,
            virtual_mic_enabled: true,
            frozen_reference: false,
            levels: AudioLevels::default(),
            health: SourceHealth::default(),
            loop_pos_ms: 0,
            loop_dur_ms: None,
        }
    }

    pub fn apply_mode_command(&mut self, cmd: &Command) -> StateEffect {
        let prev_video = self.mode.video();
        let prev_audio = self.mode.audio();
        let prev_mic = self.virtual_mic_enabled;

        match *cmd {
            Command::Toggle => {
                // In Custom mode, toggle synchronously to the opposite of the active video source.
                self.mode = match self.mode.video() {
                    VideoSource::Live => StreamMode::Loop,
                    VideoSource::Loop => StreamMode::Live,
                };
            }
            Command::SetMode(mode) => self.mode = mode,
            Command::SetVideo(video) => {
                let audio = self.mode.audio();
                self.mode = simplify_mode(video, audio);
            }
            Command::SetAudio(audio) => {
                let video = self.mode.video();
                self.mode = simplify_mode(video, audio);
            }
            Command::SetMic(action) => {
                self.virtual_mic_enabled = match action {
                    MicAction::On => true,
                    MicAction::Off => false,
                    MicAction::Toggle => !self.virtual_mic_enabled,
                };
            }
            Command::SetFreeze { frozen } => self.frozen_reference = frozen,
            Command::Show | Command::Hide | Command::Status | Command::Quit => {}
        }

        let next_video = self.mode.video();
        let next_audio = self.mode.audio();
        let video_changed = next_video != prev_video;
        let audio_changed = next_audio != prev_audio || self.virtual_mic_enabled != prev_mic;

        StateEffect {
            video_changed,
            audio_changed,
            video_switched_to_loop: video_changed && next_video == VideoSource::Loop,
        }
    }

    pub fn target_gains(&self, cfg: &Config) -> (f64, f64) {
        if !self.virtual_mic_enabled {
            return (0.0, 0.0);
        }
        match self.mode.audio() {
            AudioSource::File => (cfg.loop_volume, 0.0),
            AudioSource::Mic => (0.0, cfg.live_volume),
            AudioSource::Mix => (cfg.loop_volume, cfg.live_volume),
            AudioSource::Silence => (0.0, 0.0),
        }
    }

    pub fn snapshot(&self, cfg: &Config) -> StatusSnapshot {
        StatusSnapshot {
            mode: self.mode,
            video: self.mode.video(),
            audio: self.mode.audio(),
            virtual_mic_enabled: self.virtual_mic_enabled,
            frozen_reference: self.frozen_reference,
            startup_mode: cfg.startup_mode,
            av_delay_ms: cfg.av_delay_ms,
            health: self.health.clone(),
        }
    }
}

fn simplify_mode(video: VideoSource, audio: AudioSource) -> StreamMode {
    match (video, audio) {
        (VideoSource::Loop, AudioSource::File) => StreamMode::Loop,
        (VideoSource::Live, AudioSource::Mic) => StreamMode::Live,
        _ => StreamMode::Custom { video, audio },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_toggle_preserves_mute_and_flips_custom() {
        let cfg = Config::default();
        let mut state = AppState::from_config(&cfg);
        assert_eq!(state.mode, StreamMode::Live);
        state.virtual_mic_enabled = false;
        state.apply_mode_command(&Command::Toggle);
        assert_eq!(state.mode, StreamMode::Loop);
        assert!(!state.virtual_mic_enabled);
        assert_eq!(state.target_gains(&cfg), (0.0, 0.0));

        state.apply_mode_command(&Command::SetAudio(AudioSource::Mix));
        assert_eq!(
            state.mode,
            StreamMode::Custom {
                video: VideoSource::Loop,
                audio: AudioSource::Mix,
            }
        );
        state.apply_mode_command(&Command::Toggle);
        assert_eq!(state.mode, StreamMode::Live);
        assert!(!state.virtual_mic_enabled);
    }

    #[test]
    fn state_target_gains_cover_all_audio_sources() {
        let mut cfg = Config::default();
        cfg.loop_volume = 0.8;
        cfg.live_volume = 1.2;
        let mut state = AppState::from_config(&cfg);
        assert_eq!(state.target_gains(&cfg), (0.0, 1.2));
        state.apply_mode_command(&Command::SetMode(StreamMode::Loop));
        assert_eq!(state.target_gains(&cfg), (0.8, 0.0));
        state.apply_mode_command(&Command::SetAudio(AudioSource::Mix));
        assert_eq!(state.target_gains(&cfg), (0.8, 1.2));
        state.apply_mode_command(&Command::SetAudio(AudioSource::Silence));
        assert_eq!(state.target_gains(&cfg), (0.0, 0.0));
    }
}
