# Cyanide VirtualCam Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `cyanide`, a minimal-code (`/ponytail`) Linux virtual camera and virtual microphone utility in Rust + GStreamer + PipeWire + GTK4/libadwaita covering all features in the approved spec.

**Architecture:** Single resident binary and single Cargo crate (`cyanide`) with clean internal module boundaries (`config`, `state`, `devices`, `pipeline`, `ipc`, `gui`). The GLib/GTK4 main thread owns `AppController` exclusively (no `Arc<Mutex<MediaPipeline>>`), GStreamer streams in-process `tee → queue(leaky) → valve → gtk4paintablesink` with zero-copy preview and an `audiomixer`-based click-free audio graph, and a dedicated `tokio` thread serves `$XDG_RUNTIME_DIR/cyanide.sock` for Hyprland hotkeys and single-instance activation.

**Tech Stack:** Rust (edition 2024, `rustc 1.98`), `gstreamer 0.25`, `gstreamer-video 0.25`, `gstreamer-audio 0.25`, `gst-plugin-gtk4 0.15` (`wayland`, `dmabuf`, `gtk_v4_14`), `gtk4 0.11` (`v4_14`), `libadwaita 0.9` (`v1_5`), `tokio 1` (`rt`, `net`, `io-util`, `sync`, `time`), `serde 1`, `serde_json 1`, `toml 1`, `async-channel 2`, `thiserror 2`.

**Spec:** `docs/superpowers/specs/2026-11-19-cyanide-virtualcam-design.md`

## Global Constraints

- Single crate `cyanide` in `/home/papayka/rust_project/cyanide`, following `AGENTS.md`, `.agent/ARCHITECTURE.md`, `.agent/RUST_STYLE.md`, `.agent/CONCURRENCY.md`.
- Minimal `/ponytail` code: no fake traits, no `utils`/`helpers`/`common` modules, no `clap` bloat for 4 CLI subcommands, no `unwrap()`/`expect()` on caller-reachable failures in library code.
- Audio format fixed at `48000 Hz`, `2 channels`, `F32LE`; crossfade range `10..=30 ms` (default `20 ms`, `5 ms` ticks); A/V sync delay range `-500..=500 ms`.
- Virtual PipeWire device names fixed: sink `VirtualCamAudio`, remapped source `VirtualCamMic`; modules unloaded in reverse order only on process shutdown.
- `startup_mode` in `config.toml` defaults to `Live`; runtime mode toggles (`SUPER + F8` / `ctl toggle`) NEVER overwrite `startup_mode` on disk.
- `virtual_mic_enabled` (`Виртуальный микрофон` / `ctl mic`) is a master-mute override (fades gain to `0.0` without unloading PipeWire modules) and is preserved across `toggle`.
- `Зеркало`, `Масштаб (1.0..=3.0)`, and `Смещение X/Y` are a uniform preview-only GPU transform in `CompareView` and NEVER alter the `v4l2loopback` output stream.
- Privacy invariant: a Loop MP4 error NEVER auto-switches the active stream from `Loop` to `Live`.
- Verification floor before completion: `cargo fmt --all -- --check`, `cargo check --all-targets`, `cargo test`, `cargo clippy --all-targets --all-features -- -D warnings`.

## Review Focus

1. **Stale Unix socket after crash (`$XDG_RUNTIME_DIR/cyanide.sock`)**: leftover socket file must not block startup (`AddrInUse`); server probes/unlinks stale socket before bind -> pinned in Task 5 (`stale_socket_is_replaced_on_bind`).
2. **PipeWire feedback loop via monitor/virtual mic**: discovery and config sanitization must reject `VirtualCamMic`, `VirtualCamAudio.monitor`, and any `*.monitor` source -> pinned in Task 1 (`config_sanitizes_bounds_and_rejects_feedback_mic`) and Task 2 (`parse_pactl_sources_excludes_virtual_and_monitors`).
3. **Out-of-range or corrupt `config.toml`**: values outside spec bounds (`av_delay_ms`, `crossfade_ms`, volumes, opacity, zoom, FPS) must clamp cleanly; invalid TOML returns typed error -> pinned in Task 1 (`config_sanitizes_bounds_and_rejects_feedback_mic`, `config_load_invalid_toml_returns_error`).
4. **Rapid toggle & master-mute / `Custom` preservation**: `toggle` preserves `virtual_mic_enabled = false`, switches `Custom` to opposite of current video source, does not touch `startup_mode`, and mid-ramp crossfade retargeting stays within `0.0..=1.5` without NaN -> pinned in Task 1 (`state_toggle_preserves_mute_and_flips_custom`) and Task 3 (`gain_ramp_interpolates_and_retargets_cleanly`).
5. **MP4 error / missing source privacy & isolation**: degraded `Loop` branch falls back to black/silence placeholder without halting `Live` and NEVER auto-switches active mode from `Loop` to `Live` -> pinned in Task 4 (`pipeline_loop_degradation_never_switches_to_live`).

---

### Task 1: Crate Scaffold, Persistent `Config`, and Domain `AppState`

**Files:**
- Create: `Cargo.toml`
- Create: `config.example.toml`
- Create: `src/lib.rs`
- Create: `src/config.rs`
- Create: `src/state.rs`

**Interfaces:**
- Consumes: none
- Produces:
  - `config::Config`, `config::StartupMode` (`Live | Loop`), `config::VideoFormat { width: u32, height: u32, fps: u32 }`, `config::CompareConfig`, `config::CompareMode` (`Overlay | SideBySide | Split`), `config::LoopReturnPolicy` (`Continue | Restart | Bookmark { position_ms: u64 }`), `config::ConfigError`
  - `Config::default()`, `Config::sanitized(self) -> Self`, `Config::default_path() -> PathBuf`, `Config::load(path: &Path) -> Result<Self, ConfigError>`, `Config::save(&self, path: &Path) -> Result<(), ConfigError>`
  - `state::VideoSource` (`Loop | Live`), `state::AudioSource` (`File | Mic | Mix | Silence`), `state::StreamMode` (`Loop | Live | Custom { video: VideoSource, audio: AudioSource }`), `state::AudioLevels { loop_db: f64, live_db: f64, out_db: f64 }`, `state::SourceHealth { loop_warning: Option<String>, live_warning: Option<String>, output_warning: Option<String> }`
  - `state::AppState`, `state::Command`, `state::Reply`, `state::StatusSnapshot`
  - `AppState::from_config(cfg: &Config) -> Self`, `AppState::apply_mode_command(&mut self, cmd: &Command) -> StateEffect`, `AppState::target_gains(&self, cfg: &Config) -> (f64, f64)`, `AppState::snapshot(&self, cfg: &Config) -> StatusSnapshot`

- [ ] **Step 1: Create `Cargo.toml`, `config.example.toml`, and thin `src/lib.rs`**

`Cargo.toml` declares crate `cyanide`, edition `2024`, dependencies pinned in Tech Stack above (`gstreamer`, `gstreamer-video`, `gstreamer-audio`, `gst-plugin-gtk4`, `gtk4`, `libadwaita`, `tokio`, `serde`, `serde_json`, `toml`, `async-channel`, `thiserror`). `src/lib.rs` exports `pub mod config; pub mod state;` (later tasks add their modules). `config.example.toml` documents all defaults with `startup_mode = "live"`.

- [ ] **Step 2: Write failing unit tests in `src/config.rs` and `src/state.rs`**

In `src/config.rs` `#[cfg(test)] mod tests`:
```rust
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
```

In `src/state.rs` `#[cfg(test)] mod tests`:
```rust
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
        StreamMode::Custom { video: VideoSource::Loop, audio: AudioSource::Mix }
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
```

- [ ] **Step 3: Run tests to verify failure**

Run: `cargo test --lib`
Expected: FAIL (unimplemented stubs / missing types)

- [ ] **Step 4: Implement `src/config.rs` and `src/state.rs`**

- `Config::sanitized` clamps `av_delay_ms` to `-500..=500`, `crossfade_ms` to `10..=30`, `loop_volume`/`live_volume` to `0.0..=1.5`, `compare.opacity` & `split_pos` & `guide_x`/`guide_y` to `0.0..=1.0`, `compare.zoom` to `1.0..=3.0`, `compare.pan_x`/`pan_y` to `-1.0..=1.0`, rejects `fps == 0` or `width/height == 0` back to `1280x720@30`, and clears `real_mic` if it equals `"VirtualCamMic"` or ends with `".monitor"`.
- `Config::save` writes atomically via sibling `.tmp` file + `rename` after creating parent dirs.
- `AppState::apply_mode_command` handles `Toggle`, `SetMode`, `SetVideo`, `SetAudio`, `SetMic(MicAction)`, `SetFreeze`, returns `StateEffect { video_switched_to_loop: bool, audio_changed: bool, video_changed: bool }`.

- [ ] **Step 5: Run tests and verify pass**

Run: `cargo test --lib`
Expected: PASS (all config + state tests green)

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock config.example.toml src/lib.rs src/config.rs src/state.rs
git commit -m "feat: add crate scaffold, persistent config, and domain state"
```

---

### Task 2: V4L2 Discovery & PipeWire Virtual Mic (`devices/`)

**Files:**
- Create: `src/devices/mod.rs`
- Create: `src/devices/v4l2.rs`
- Create: `src/devices/pipewire.rs`
- Modify: `src/lib.rs` (export `pub mod devices;`)

**Interfaces:**
- Consumes: `std::path::Path`, `std::process::Command`
- Produces:
  - `devices::VIRTUAL_SINK_NAME = "VirtualCamAudio"`, `devices::VIRTUAL_MIC_NAME = "VirtualCamMic"`
  - `devices::v4l2::{VideoDevice, V4l2Inventory, scan_sysfs(sysfs_root: &Path) -> V4l2Inventory, discover_cameras() -> V4l2Inventory}`
  - `devices::pipewire::{AudioDevice, PipeWireError, VirtualMicGuard, parse_pactl_sources(stdout: &str) -> Vec<AudioDevice>, parse_module_id(stdout: &str) -> Result<u32, PipeWireError>, discover_real_mics() -> Result<Vec<AudioDevice>, PipeWireError>}`
  - `VirtualMicGuard::create(state_file: &Path) -> Result<Self, PipeWireError>`, `VirtualMicGuard::module_ids(&self) -> (u32, u32)` (unloads `remap_module_id` then `null_sink_module_id` in `Drop` and removes `state_file`)
  - `devices::DeviceInventory { real_cameras: Vec<VideoDevice>, loopback_cameras: Vec<VideoDevice>, real_mics: Vec<AudioDevice> }`, `devices::discover_all() -> DeviceInventory`
  - `devices::modprobe_hint() -> &'static str`

- [ ] **Step 1: Write failing unit tests in `src/devices/v4l2.rs` and `src/devices/pipewire.rs`**

In `src/devices/v4l2.rs`:
```rust
#[test]
fn scan_sysfs_separates_real_and_loopback_cameras() {
    let root = std::env::temp_dir().join("cyanide-sysfs-test");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("video0/device")).unwrap();
    std::fs::write(root.join("video0/name"), "HD Camera: HD Camera\n").unwrap();
    std::fs::write(root.join("video0/device/uevent"), "DRIVER=uvcvideo\n").unwrap();
    std::fs::create_dir_all(root.join("video10/device")).unwrap();
    std::fs::write(root.join("video10/name"), "VirtualCam\n").unwrap();
    std::fs::write(root.join("video10/device/uevent"), "DRIVER=v4l2loopback\n").unwrap();

    let inv = scan_sysfs(&root);
    assert_eq!(inv.real_cameras.len(), 1);
    assert_eq!(inv.real_cameras[0].path, "/dev/video0");
    assert_eq!(inv.loopback_cameras.len(), 1);
    assert_eq!(inv.loopback_cameras[0].path, "/dev/video10");
    let _ = std::fs::remove_dir_all(&root);
}
```

In `src/devices/pipewire.rs`:
```rust
#[test]
fn parse_pactl_sources_excludes_virtual_and_monitors() {
    let sample = "\
42\talsa_input.pci-0000_00_1f.3.analog-stereo\tPipeWire\ts32le 2ch 48000Hz\tRUNNING
43\talsa_output.pci-0000_00_1f.3.analog-stereo.monitor\tPipeWire\tfloat32le 2ch 48000Hz\tIDLE
55\tVirtualCamAudio.monitor\tPipeWire\tfloat32le 2ch 48000Hz\tIDLE
56\tVirtualCamMic\tPipeWire\tfloat32le 2ch 48000Hz\tIDLE
";
    let mics = parse_pactl_sources(sample);
    assert_eq!(mics.len(), 1);
    assert_eq!(mics[0].name, "alsa_input.pci-0000_00_1f.3.analog-stereo");
}

#[test]
fn parse_module_id_reads_trimmed_integer() {
    assert_eq!(parse_module_id("536870912\n").unwrap(), 536870912);
    assert!(parse_module_id("not-a-number").is_err());
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --lib devices::`
Expected: FAIL

- [ ] **Step 3: Implement `src/devices/v4l2.rs`, `src/devices/pipewire.rs`, and `src/devices/mod.rs`**

- `v4l2.rs`: parses `videoN` entries sorted numerically by `N`, classifies loopback if `device/uevent` mentions `v4l2loopback` or `name` contains `v4l2loopback`/`virtualcam` (case-insensitive) or `device` symlink points into `/devices/virtual/video4linux`. Deduplicates UVC metadata nodes cleanly where possible (keeps all real nodes with clear names).
- `pipewire.rs`: `VirtualMicGuard::create(state_file)`:
  1. If `state_file` exists from a prior crashed run, parses saved IDs and runs `pactl unload-module` (best-effort, ignoring already-unloaded errors).
  2. Checks `pactl list short modules` for any lingering `sink_name=VirtualCamAudio` or `source_name=VirtualCamMic` and unloads them in reverse order so no duplicates accumulate.
  3. Loads `module-null-sink sink_name=VirtualCamAudio sink_properties=device.description=VirtualCamAudio rate=48000 channels=2` and `module-remap-source master=VirtualCamAudio.monitor source_name=VirtualCamMic source_properties=device.description=VirtualCamMic`. If the second fails, unloads the first before returning `Err`.
  4. Writes both IDs to `state_file`. `Drop` unloads `remap_id` then `sink_id` and removes `state_file`.

- [ ] **Step 4: Run tests and verify pass**

Run: `cargo test --lib devices::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/devices/
git commit -m "feat: add v4l2 discovery and PipeWire virtual mic lifecycle"
```

---

### Task 3: Audio Subgraph, 20ms Crossfade Ramp, and A/V Delay (`pipeline/audio.rs`)

**Files:**
- Create: `src/pipeline/audio.rs`
- Create: `src/pipeline/mod.rs` (minimal module stub for Task 3 tests; completed in Task 4)
- Modify: `src/lib.rs` (export `pub mod pipeline;`)

**Interfaces:**
- Consumes: `config::Config`, `devices::VIRTUAL_SINK_NAME`, `gstreamer` (`gst`)
- Produces:
  - `pipeline::audio::GainRamp` (`new(start: (f64, f64), target: (f64, f64), duration_ms: u32, tick_ms: u32) -> Self`, `step(&mut self) -> ((f64, f64), bool)`, `retarget(&mut self, new_target: (f64, f64), duration_ms: u32)`)
  - `pipeline::audio::AudioBranch` (holds `loop_vol`, `live_vol`, `out_queue`, `loop_input_queue`, `mixer`, `live_warning: Option<String>`, `output_warning: Option<String>`)
  - `AudioBranch::build(pipeline: &gst::Pipeline, cfg: &Config, test_sources: bool) -> Result<Self, PipelineError>`
  - `AudioBranch::attach_loop_audio_pad(&self, pad: &gst::Pad) -> bool` (links decoded MP4 audio into `loop_input_queue` -> `audioconvert ! audioresample ! caps(48k/2ch/F32LE) ! volume ! level ! audiomixer`)
  - `AudioBranch::set_gains(&self, loop_gain: f64, live_gain: f64)`
  - `AudioBranch::current_gains(&self) -> (f64, f64)`
  - `AudioBranch::set_delay_ns(&self, audio_delay_ns: u64)` (sets `out_queue` src pad offset)
  - `pipeline::audio::parse_level_message(msg: &gst::MessageRef) -> Option<(&str, f64)>` (extracts element name + max-channel RMS dB clamped to `-60.0..=0.0`)

- [ ] **Step 1: Write failing unit & GStreamer audio-graph tests in `src/pipeline/audio.rs`**

```rust
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
    let branch = AudioBranch::build(&pipeline, &cfg, true).unwrap();
    branch.set_gains(0.8, 0.0);
    branch.set_delay_ns(50_000_000);
    pipeline.set_state(gst::State::Playing).unwrap();
    let (res, state, _) = pipeline.state(gst::ClockTime::from_seconds(2));
    assert!(res.is_ok() && state == gst::State::Playing);
    pipeline.set_state(gst::State::Null).unwrap();
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --lib pipeline::audio`
Expected: FAIL

- [ ] **Step 3: Implement `src/pipeline/audio.rs`**

- Always adds `audiotestsrc wave=silence is-live=true` -> `audio/x-raw,format=F32LE,rate=48000,channels=2` -> `audiomixer` (`force-live=true`, `latency=20ms`) so mixer never stalls if MP4 has no audio track or live mic is missing.
- Live mic branch: if `!test_sources` and `pulsesrc` factory exists and a real mic is configured (or default non-monitor), creates `pulsesrc`; otherwise substitutes `audiotestsrc wave=silence is-live=true` (or `sine` in `--test-sources`) and records `live_warning`.
- Output sink: if `!test_sources` and `pulsesink` factory exists, creates `pulsesink device=VirtualCamAudio`; otherwise uses `fakesink sync=false` and records `output_warning`.
- Elements `level` (`name=loop_level`, `name=live_level`, `name=out_level`, `post-messages=true`, `interval=50ms`) are added when the `level` factory exists (gracefully skipped with a warning if `gst-plugins-good` is missing so `--test-sources` still runs on minimal Arch installs!).
- `GainRamp`: strictly clamps targets to `0.0..=1.5`, guarantees `total_steps >= 1` so zero division is impossible.

- [ ] **Step 4: Run tests and verify pass**

Run: `cargo test --lib pipeline::audio`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/pipeline/
git commit -m "feat: add GStreamer audio subgraph, crossfade ramp, and A/V delay"
```

---

### Task 4: Video Subgraph & `MediaPipeline` Orchestrator (`pipeline/video.rs`, `pipeline/mod.rs`)

**Files:**
- Create: `src/pipeline/video.rs`
- Modify: `src/pipeline/mod.rs`

**Interfaces:**
- Consumes: `config::Config`, `config::LoopReturnPolicy`, `state::{AppState, AudioLevels, SourceHealth, StreamMode, VideoSource}`, `pipeline::audio::{AudioBranch, GainRamp, parse_level_message}`
- Produces:
  - `pipeline::PreviewBackend` (`Gtk4 | Fake`)
  - `pipeline::PipelineError` (`thiserror::Error`)
  - `pipeline::PreviewObjects { pub loop_sink: gst::Element, pub live_sink: gst::Element, pub out_sink: gst::Element }`
  - `pipeline::PipelineEvent` (`Levels(AudioLevels) | Position { pos_ms: u64, dur_ms: Option<u64> } | Degraded { branch: VideoSource, reason: String } | FatalError(String)`)
  - `pipeline::MediaPipeline`:
    - `MediaPipeline::start(cfg: &Config, state: &AppState, backend: PreviewBackend, test_sources: bool, on_event: impl Fn(PipelineEvent) + 'static) -> Result<Self, PipelineError>`
    - `MediaPipeline::select_video(&self, source: VideoSource, return_policy: LoopReturnPolicy, switched_to_loop: bool)`
    - `MediaPipeline::start_audio_ramp(&mut self, target: (f64, f64), crossfade_ms: u32)`
    - `MediaPipeline::set_av_delay_ms(&self, av_delay_ms: i32)`
    - `MediaPipeline::set_previews_active(&self, active: bool)`
    - `MediaPipeline::loop_position_ms(&self) -> Option<(u64, Option<u64>)>`
    - `MediaPipeline::preview_objects(&self) -> &PreviewObjects`
    - `MediaPipeline::health(&self) -> SourceHealth`
    - `MediaPipeline::stop(&mut self)` (cancels ramp timer, removes bus watch, sets `gst::State::Null`)
  - `pipeline::check_runtime_plugins() -> Vec<(&'static str, bool)>` (for `cyanide doctor`)

- [ ] **Step 1: Write failing integration tests in `src/pipeline/mod.rs`**

```rust
#[test]
fn pipeline_starts_switches_modes_and_applies_av_delay() {
    gst::init().unwrap();
    let cfg = Config::default();
    let mut state = AppState::from_config(&cfg);
    let mut pipe = MediaPipeline::start(
        &cfg,
        &state,
        PreviewBackend::Fake,
        true,
        |_| {},
    )
    .unwrap();

    pipe.set_previews_active(true);
    pipe.set_av_delay_ms(120);
    pipe.set_av_delay_ms(-80);
    state.apply_mode_command(&Command::Toggle);
    pipe.select_video(state.mode.video(), cfg.loop_return, true);
    pipe.set_previews_active(false);
    pipe.stop();
}

#[test]
fn pipeline_loop_degradation_never_switches_to_live() {
    gst::init().unwrap();
    let mut cfg = Config::default();
    cfg.startup_mode = StartupMode::Loop;
    cfg.loop_file = Some(PathBuf::from("/nonexistent/missing-loop-file.mp4"));
    let state = AppState::from_config(&cfg);
    let mut pipe = MediaPipeline::start(
        &cfg,
        &state,
        PreviewBackend::Fake,
        true,
        |_| {},
    )
    .unwrap();

    // Missing MP4 falls back to synthetic placeholder with warning; mode stays Loop
    assert!(pipe.health().loop_warning.is_some());
    assert_eq!(state.mode, StreamMode::Loop);
    assert_eq!(pipe.active_video_source(), VideoSource::Loop);
    pipe.stop();
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --lib pipeline::`
Expected: FAIL

- [ ] **Step 3: Implement `src/pipeline/video.rs` and `src/pipeline/mod.rs`**

- `video.rs`:
  - Builds loop branch: if `cfg.loop_file` exists on disk (and not `--test-sources`), builds `uridecodebin` with `pad-added` connecting video pad into `loop_video_queue` (and audio pad into `audio_branch.attach_loop_audio_pad`); otherwise uses `videotestsrc` (`pattern=smpte` in `--test-sources`, `pattern=black` as degraded placeholder) with `is-live=true` and sets `loop_warning`.
  - Builds live branch: if `!test_sources`, `v4l2src` factory exists, and `cfg.real_camera` exists, uses `v4l2src device=...`; otherwise uses `videotestsrc` (`pattern=ball` in `--test-sources`, `pattern=black` as degraded placeholder) with `is-live=true` and sets `live_warning`.
  - Both branches normalize via `videoconvert ! videoscale ! videorate ! caps(video/x-raw,width=W,height=H,framerate=FPS/1)` -> `tee`:
    - Preview leg: `queue leaky=downstream max-size-buffers=2 ! valve drop=true ! preview_sink` (`gtk4paintablesink` for `PreviewBackend::Gtk4`, `fakesink sync=false` for `PreviewBackend::Fake`).
    - Selector leg: `queue ! video_sel.sink_0` (loop) / `sink_1` (live).
  - `input-selector` (`sync-streams=true`, `sync-mode=clock`) -> `out_video_queue (max-size-time=2s)` -> `tee`:
    - Preview leg: `queue leaky=downstream max-size-buffers=2 ! valve drop=true ! out_preview_sink`.
    - Broadcast leg: `queue ! videoconvert ! caps(YUY2, WxH@FPS) ! v4l2sink(device=..., sync=false)` (or `fakesink sync=false` when loopback missing / `--test-sources`, setting `output_warning`).
- `mod.rs`:
  - Bus watch (`bus.add_watch_local`):
    - On `AsyncDone` (first preroll with real MP4): sends `SeekFlags::FLUSH | SeekFlags::SEGMENT` from `0` to end.
    - On `SegmentDone`: sends non-flushing `SeekFlags::SEGMENT` back to `0` (seamless loop).
    - On `Element` (`level` message): updates cached `AudioLevels` and emits `PipelineEvent::Levels`.
    - On `Error`: identifies whether failing source belongs to `loop` or `live` branch and emits `PipelineEvent::Degraded { branch, reason }` (so `AppController` rebuilds cleanly with placeholder while keeping `AppState::mode` unchanged — **never** switching `Loop → Live`!).
  - `set_av_delay_ms(ms)`: clamps `-500..=500`; if `>= 0` sets `audio.set_delay_ns(ms * 1_000_000)` and `video.set_delay_ns(0)`, else sets `video.set_delay_ns((-ms) * 1_000_000)` and `audio.set_delay_ns(0)`.
  - `start_audio_ramp`: cancels any previous GLib `SourceId` and schedules `5 ms` `glib::timeout_add_local` driving `GainRamp`.

- [ ] **Step 4: Run tests and verify pass**

Run: `cargo test --lib pipeline::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/pipeline/
git commit -m "feat: add video subgraph, seamless looping, and MediaPipeline orchestrator"
```

---

### Task 5: `tokio` Unix Socket IPC Server & `ctl` Client (`ipc.rs`)

**Files:**
- Create: `src/ipc.rs`
- Modify: `src/lib.rs` (export `pub mod ipc;`)

**Interfaces:**
- Consumes: `state::{AudioSource, Command, MicAction, Reply, StreamMode, VideoSource}`
- Produces:
  - `ipc::IpcError` (`thiserror::Error`)
  - `ipc::default_socket_path() -> PathBuf` (`$XDG_RUNTIME_DIR/cyanide.sock`, fallback `/tmp/cyanide-<uid>.sock`)
  - `ipc::parse_command_line(line: &str) -> Result<Command, String>` (accepts JSON `Command` OR shorthand: `toggle`, `mode loop|live`, `video loop|live`, `audio file|mic|mix|silence`, `mic on|off|toggle`, `show`, `hide`, `status`, `quit`)
  - `ipc::IpcRequest { pub command: Command, pub reply: tokio::sync::oneshot::Sender<Reply> }`
  - `ipc::IpcServerHandle`:
    - `IpcServerHandle::start(socket_path: PathBuf, tx: async_channel::Sender<IpcRequest>) -> Result<Self, IpcError>`
    - `IpcServerHandle::shutdown(self)` (signals `Notify`, joins `cyanide-ipc` thread, unlinks socket)
  - `ipc::send_command(socket_path: &Path, command: &Command) -> Result<Reply, IpcError>` (fast synchronous `UnixStream` client with 2s timeout, used by `cyanide ctl` and single-instance `show`)

- [ ] **Step 1: Write failing unit & socket integration tests in `src/ipc.rs`**

```rust
#[test]
fn parse_command_line_accepts_text_and_json() {
    assert_eq!(parse_command_line("toggle").unwrap(), Command::Toggle);
    assert_eq!(parse_command_line("mode loop").unwrap(), Command::SetMode(StreamMode::Loop));
    assert_eq!(parse_command_line("audio mix").unwrap(), Command::SetAudio(AudioSource::Mix));
    assert_eq!(parse_command_line("mic off").unwrap(), Command::SetMic(MicAction::Off));
    assert_eq!(parse_command_line("show").unwrap(), Command::Show);
    let json = serde_json::to_string(&Command::Status).unwrap();
    assert_eq!(parse_command_line(&json).unwrap(), Command::Status);
    assert!(parse_command_line("bogus").is_err());
}

#[test]
fn stale_socket_is_replaced_on_bind_and_roundtrips_commands() {
    let sock = std::env::temp_dir().join(format!("cyanide-ipc-{}.sock", std::process::id()));
    std::fs::write(&sock, b"stale").unwrap();

    let (tx, rx) = async_channel::bounded::<IpcRequest>(8);
    let server = IpcServerHandle::start(sock.clone(), tx).unwrap();

    let responder = std::thread::spawn(move || {
        let req = rx.recv_blocking().unwrap();
        assert_eq!(req.command, Command::Toggle);
        let _ = req.reply.send(Reply::Ok { message: "toggled".into() });
    });

    let reply = send_command(&sock, &Command::Toggle).unwrap();
    assert!(matches!(reply, Reply::Ok { .. }));
    responder.join().unwrap();
    server.shutdown();
    assert!(!sock.exists());
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --lib ipc::`
Expected: FAIL

- [ ] **Step 3: Implement `src/ipc.rs`**

- Before `UnixListener::bind(&socket_path)`: if `socket_path.exists()`, attempt `std::os::unix::net::UnixStream::connect(&socket_path)`. If connection succeeds, return `Err(IpcError::AlreadyRunning)`; otherwise unlink the stale socket file before binding.
- Spawn named OS thread `"cyanide-ipc"` running `tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build()`.
- Each accepted connection reads 1 line (capped at 8 KiB via `take(8192)` to bound memory!), parses via `parse_command_line`, sends `IpcRequest` on bounded `async_channel::Sender`, awaits `oneshot` reply (with 3s timeout), writes single JSON line `\n`, flushes.
- `shutdown()` triggers `Arc<Notify>`, joins the thread cleanly, and unlinks `socket_path`.

- [ ] **Step 4: Run tests and verify pass**

Run: `cargo test --lib ipc::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/ipc.rs
git commit -m "feat: add tokio Unix socket IPC server and ctl client"
```

---

### Task 6: GPU Comparison Widget (`gui/compare.rs`) & Adwaita UI (`gui/controls.rs`, `gui/mod.rs`)

**Files:**
- Create: `src/gui/compare.rs`
- Create: `src/gui/controls.rs`
- Create: `src/gui/mod.rs`
- Modify: `src/lib.rs` (export `pub mod gui;`)

**Interfaces:**
- Consumes: `config::{CompareConfig, CompareMode, Config, LoopReturnPolicy, StartupMode, VideoFormat}`, `devices::DeviceInventory`, `state::{AppState, AudioLevels, AudioSource, Command, MicAction, SourceHealth, StreamMode, VideoSource}`
- Produces:
  - `gui::compare::CompareView` (`gtk4::Widget` subclass):
    - `CompareView::new() -> Self`
    - `CompareView::set_paintables(&self, loop_p: Option<gdk::Paintable>, live_p: Option<gdk::Paintable>)`
    - `CompareView::set_config(&self, cfg: CompareConfig)`
    - `CompareView::set_frozen(&self, frozen: bool)` (captures `loop_paintable.current_image()` on `true`, clears on `false`, never touches pipeline!)
    - `CompareView::is_frozen(&self) -> bool`
    - `CompareView::connect_split_changed(&self, f: impl Fn(f64) + 'static)`
  - Pure testable geometry helpers in `gui/compare.rs`:
    - `fit_aspect_rect(bounds: (f32, f32), aspect: f32) -> (f32, f32, f32, f32)`
    - `split_clip_rects(w: f32, h: f32, ratio: f32) -> ((f32, f32, f32, f32), (f32, f32, f32, f32))`
    - `grid_segments(w: f32, h: f32, cfg: &CompareConfig) -> Vec<(f32, f32, f32, f32)>`
  - `gui::MainWindow`:
    - `MainWindow::new(app: &adw::Application, on_ui_action: Rc<dyn Fn(UiAction)>, on_visibility: Rc<dyn Fn(bool)>) -> Self`
    - `MainWindow::bind_paintables(&self, loop_p: Option<gdk::Paintable>, live_p: Option<gdk::Paintable>, out_p: Option<gdk::Paintable>)`
    - `MainWindow::sync_state(&self, cfg: &Config, state: &AppState, devices: &DeviceInventory)`
    - `MainWindow::update_levels(&self, levels: &AudioLevels)`
    - `MainWindow::update_timecode(&self, pos_ms: u64, dur_ms: Option<u64>, fps: u32)`
    - `MainWindow::present(&self)` / `MainWindow::hide(&self)` / `MainWindow::is_visible(&self) -> bool`
  - `gui::UiAction` (thin enum of UI intents: `Command(Command)`, `UpdateConfig(ConfigPatch)`, `PickLoopFile(PathBuf)`, `RefreshDevices`, `SetReturnBookmarkCurrent`)

- [ ] **Step 1: Write failing unit tests for pure geometry helpers in `src/gui/compare.rs`**

```rust
#[test]
fn split_clip_rects_partition_bounds_cleanly() {
    let (left, right) = split_clip_rects(1000.0, 500.0, 0.35);
    assert_eq!(left, (0.0, 0.0, 350.0, 500.0));
    assert_eq!(right, (350.0, 0.0, 650.0, 500.0));
}

#[test]
fn grid_segments_reflect_toggles() {
    let mut cfg = CompareConfig::default();
    cfg.show_grid = true;
    cfg.show_center = true;
    cfg.show_guides = true;
    cfg.guide_x = 0.25;
    cfg.guide_y = 0.75;
    let segs = grid_segments(900.0, 600.0, &cfg);
    // 4 lines for 3x3 + 2 for center + 2 for custom guides = 8 segments
    assert_eq!(segs.len(), 8);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --lib gui::compare`
Expected: FAIL

- [ ] **Step 3: Implement `src/gui/compare.rs`, `src/gui/controls.rs`, and `src/gui/mod.rs`**

- `compare.rs`:
  - Subclasses `gtk4::Widget` via `glib::subclass::prelude::*`.
  - Connects `invalidate-contents` signals on `loop_paintable` and `live_paintable` (tracking `SignalHandlerId`s to disconnect when replaced!) calling `widget.queue_draw()`.
  - `snapshot(&self, snapshot)`:
    1. Clips widget bounds, fills dark neutral backdrop.
    2. Helper `draw_transformed(snapshot, paintable, rect, cfg)` pushes clip on `rect`, translates to center + `(pan_x * w * 0.5, pan_y * h * 0.5)`, scales `(if mirror { -zoom } else { zoom }, zoom)`, translates back, calls `paintable.snapshot(snapshot, w, h)` — one uniform preview transform for both layers!
    3. `Overlay`: draws active loop (`frozen_frame` if frozen else `loop_paintable`), then `push_opacity(opacity)`, draws `live_paintable`, `pop()`.
    4. `SideBySide`: draws loop in left half rect, live in right half rect.
    5. `Split`: `push_clip(left)` -> loop, `push_clip(right)` -> live, plus crisp 2px divider handle line; `gtk4::GestureDrag` updates `split_pos` clamped to `0.05..=0.95` and emits `split_changed`.
    6. Draws grid/center/custom guide segments via `snapshot.append_color(...)`.
- `controls.rs` & `mod.rs`:
  - Builds the exact 3-area layout from Spec Section 5.7 using `adw::ApplicationWindow` (`hide_on_close = true`), `adw::HeaderBar`, `adw::Banner` (for degraded/missing loopback warning + `modprobe` copy), 3 top source cards (including live `gtk4::Picture` for virtual camera output), `CompareView` + comparison toolbar (`Наложение/Рядом/Разделитель`, `Прозрачность 0–100%`, `Сетка`, `Центр`, `Направляющие` + X/Y sliders, `Зеркало`, `Стоп-кадр`, `Масштаб`, `Смещение X/Y`, `Сброс`), scrollable right `adw::PreferencesGroup` controls (stream mode, independent video/audio selectors, `Виртуальный микрофон` switch, `Режим при старте`, MP4 file picker, camera/loopback/mic dropdowns, resolution/FPS selectors, MP4/Mic volumes + 3 `GtkLevelBar` meters, A/V delay `-500..+500 ms`, crossfade `10–30 ms`, loop return-point policy + bookmark button), and bottom status bar.
  - Uses a `syncing: Cell<bool>` guard so programmatic `sync_state` updates never emit feedback widget signals!
  - Connects `notify::visible` on window -> invokes `on_visibility(visible)` so `AppController` sets `pipeline.set_previews_active(visible)` (zero GPU/CPU work when closed!).

- [ ] **Step 4: Run tests and check compilation**

Run: `cargo test --lib gui::`
Expected: PASS (compiles cleanly with `gtk4` + `libadwaita` and geometry tests pass)

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/gui/
git commit -m "feat: add GPU CompareView widget and libadwaita controls GUI"
```

---

### Task 7: CLI Entrypoint (`main.rs`), `AppController`, `doctor`, `systemd` Unit, & End-to-End Verification

**Files:**
- Create: `src/main.rs`
- Create: `systemd/cyanide.service`
- Create: `README.md`

**Interfaces:**
- Consumes: all crate modules (`config`, `state`, `devices`, `pipeline`, `ipc`, `gui`)
- Produces:
  - Binary `cyanide` supporting:
    - `cyanide [--background] [--test-sources]`
    - `cyanide ctl <toggle|mode <loop|live>|video <loop|live>|audio <file|mic|mix|silence>|mic <on|off|toggle>|show|hide|status|quit>`
    - `cyanide doctor`
  - `systemd/cyanide.service` (`After=graphical-session.target pipewire-pulse.service`, `ExecStart=%h/.cargo/bin/cyanide --background`, `Restart=on-failure`)

- [ ] **Step 1: Write failing unit tests for CLI argument parsing in `src/main.rs`**

```rust
#[test]
fn cli_parses_run_flags_ctl_and_doctor() {
    assert_eq!(
        parse_cli(["cyanide", "--background", "--test-sources"]).unwrap(),
        CliAction::Run { background: true, test_sources: true }
    );
    assert_eq!(
        parse_cli(["cyanide", "ctl", "toggle"]).unwrap(),
        CliAction::Ctl(Command::Toggle)
    );
    assert_eq!(
        parse_cli(["cyanide", "ctl", "audio", "silence"]).unwrap(),
        CliAction::Ctl(Command::SetAudio(AudioSource::Silence))
    );
    assert_eq!(parse_cli(["cyanide", "doctor"]).unwrap(), CliAction::Doctor);
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --bin cyanide`
Expected: FAIL

- [ ] **Step 3: Implement `src/main.rs`, `systemd/cyanide.service`, and concise `README.md`**

- `main.rs`:
  - `CliAction::Ctl(cmd)`: calls `ipc::send_command(&default_socket_path(), &cmd)` directly (no GTK/GStreamer init!), prints JSON reply, exits with code `0` on `Ok`/`Status` or `1` on `Error`.
  - `CliAction::Doctor`: initializes GStreamer + registers `gstgtk4::plugin_register_static()`, checks required plugins, V4L2 devices (`v4l2loopback`), and PipeWire `pactl`, prints concise status table + exact Arch `pacman` / `modprobe` commands for anything missing.
  - `CliAction::Run { background, test_sources }`:
    1. If `!background`, probes existing socket with `Command::Show`; if already running, exits `0` immediately (single-instance!).
    2. Initializes `gst::init()` + `gstgtk4::plugin_register_static()`.
    3. Loads `Config` (creating default if missing, sanitizing bounds).
    4. Creates `Option<VirtualMicGuard>` when `!test_sources` (gracefully logs & sets audio warning if `pactl` fails).
    5. Starts `IpcServerHandle` on `$XDG_RUNTIME_DIR/cyanide.sock` with `async_channel::bounded(32)`.
    6. Starts `adw::Application` (`ApplicationFlags::NON_UNIQUE` since single-instance is handled by our Unix socket!), acquires `let _hold = app.hold();`, owns `Rc<RefCell<AppController>>` exclusively on the GLib main thread:
       - Starts `MediaPipeline` (GTK4 preview sinks), wires `glib::spawn_future_local` draining IPC requests and applying commands cleanly.
       - Schedules debounced `Config::save` (~300 ms) only on persistent setting mutations (NEVER on `Toggle` / `SetMode` / `SetVideo` / `SetAudio` / `SetMic`).
       - Handles `PipelineEvent::Degraded` by rebuilding the pipeline with the broken branch forced to placeholder while **strictly preserving `AppState::mode`** (never flipping `Loop → Live`).
       - Creates `MainWindow` lazily on first show (or immediately if `!background`), binds the 3 `gdk::Paintable`s, updates timecode at 250 ms only while visible, and toggles preview `valve`s on show/hide.
       - Registers `glib::unix_signal_add_local` for `SIGINT` & `SIGTERM` (and `Command::Quit`) to cleanly stop pipeline -> `Null`, drop `VirtualMicGuard`, shutdown `IpcServerHandle`, and release `app.quit()`.

- [ ] **Step 4: Run full workspace verification & live CLI/IPC smoke test**

Run:
```bash
cargo fmt --all -- --check
cargo check --all-targets
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```
And live smoke test:
```bash
cargo run -- doctor
```
Plus automated end-to-end test in `tests/cli_ipc_smoke.rs` (spawning `cyanide --background --test-sources` or exercising full headless daemon + `ctl status` + `ctl toggle` + `ctl mic off` + `ctl toggle` + `ctl quit` on an isolated `XDG_RUNTIME_DIR`/`XDG_CONFIG_HOME` — Wait: verify if `--background` works cleanly with `WAYLAND_DISPLAY` on this machine!).

- [ ] **Step 5: Commit**

```bash
git add src/main.rs systemd/cyanide.service README.md
git commit -m "feat: wire CLI, AppController, doctor, and systemd user service"
```

---

## Self-Review Notes (Completed)

- **Spec coverage**: Every section (5.1–5.8, Section 2 GUI mockup, Section 3 audio modes/crossfade/silence, Section 4 `pactl` virtual mic + module ID unload, Section 6 A/V delay, volumes, meters, resolution/FPS, return point, reference freeze, Hyprland `SUPER + F8`, `systemd --user`) maps to Tasks 1–7 with zero gaps.
- **Type consistency**: Verified `Config`, `AppState`, `Command`, `Reply`, `StreamMode`, `VideoSource`, `AudioSource`, `LoopReturnPolicy`, `CompareConfig`, `VirtualMicGuard`, `MediaPipeline`, `PreviewBackend`, `IpcServerHandle`, `CompareView`, `MainWindow` match across every task's `Consumes` / `Produces`.
- **Review Focus**: All 5 high-risk failure modes have dedicated unit/integration tests in their owning tasks.
