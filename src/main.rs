use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use gstreamer as gst;
use gtk4::{gdk, gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;

use cyanide::config::{Config, LoopReturnPolicy};
use cyanide::devices::{
    self, DeviceInventory, VIRTUAL_MIC_NAME, VIRTUAL_SINK_NAME, VirtualMicGuard, modprobe_hint,
};
use cyanide::gui::overlay::OverlayWindow;
use cyanide::gui::{ConfigPatch, MainWindow, UiAction};
use cyanide::hotkey::{Hotkey, HotkeyManager};
use cyanide::ipc::{self, IpcRequest, IpcServerHandle};
use cyanide::pipeline::record::{self, RecordingFinished};
use cyanide::pipeline::{
    self, DegradedBranch, ForceFallback, MediaPipeline, PipelineEvent, PreviewBackend,
};
use cyanide::state::{AppState, Command, RecordAction, Reply};

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliAction {
    Run {
        background: bool,
        test_sources: bool,
    },
    Ctl(Command),
    Doctor,
    Help,
}

fn parse_cli<I, S>(args: I) -> Result<CliAction, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let items: Vec<String> = args
        .into_iter()
        .skip(1)
        .map(|s| s.as_ref().to_string())
        .collect();
    if items.is_empty() {
        return Ok(CliAction::Run {
            background: false,
            test_sources: false,
        });
    }

    match items[0].as_str() {
        "-h" | "--help" | "help" => Ok(CliAction::Help),
        "doctor" => Ok(CliAction::Doctor),
        "ctl" => {
            if items.len() < 2 {
                return Err("missing ctl subcommand (try: toggle, mode, video, audio, mic, record, show, hide, status, quit)".into());
            }
            let joined = items[1..].join(" ");
            let cmd = ipc::parse_command_line(&joined)?;
            Ok(CliAction::Ctl(cmd))
        }
        _ => {
            let mut background = false;
            let mut test_sources = false;
            for flag in &items {
                match flag.as_str() {
                    "--background" | "-b" => background = true,
                    "--test-sources" => test_sources = true,
                    other => return Err(format!("unknown option '{other}'")),
                }
            }
            Ok(CliAction::Run {
                background,
                test_sources,
            })
        }
    }
}

fn print_help() {
    println!(
        "\
cyanide — Virtual camera (v4l2loopback) & virtual mic (PipeWire) studio

USAGE:
  cyanide [--background] [--test-sources]
  cyanide ctl <toggle|mode <loop|live>|video <loop|live>|audio <file|mic|mix|silence>|mic <on|off|toggle>|record <start|stop|toggle>|show|hide|status|quit>
  cyanide doctor"
    );
}

fn run_ctl(cmd: &Command) -> ExitCode {
    let sock = ipc::default_socket_path();
    match ipc::send_command(&sock, cmd) {
        Ok(reply) => {
            if let Ok(json) = serde_json::to_string(&reply) {
                println!("{json}");
            }
            match reply {
                Reply::Error { .. } => ExitCode::FAILURE,
                Reply::Ok { .. } | Reply::Status(_) => ExitCode::SUCCESS,
            }
        }
        Err(err) => {
            eprintln!("error: {err} (socket: {})", sock.display());
            ExitCode::FAILURE
        }
    }
}

fn run_doctor() -> ExitCode {
    if let Err(err) = gst::init() {
        eprintln!("GStreamer init failed: {err}");
        return ExitCode::FAILURE;
    }
    let _ = gstgtk4::plugin_register_static();

    println!("=== GStreamer plugins ===");
    let mut missing_good = false;
    for (name, present) in pipeline::check_runtime_plugins() {
        println!("  {name:<20} {}", if present { "OK" } else { "MISSING" });
        if !present
            && matches!(
                name,
                "v4l2src" | "v4l2sink" | "pulsesrc" | "pulsesink" | "level"
            )
        {
            missing_good = true;
        }
    }

    println!("\n=== V4L2 devices ===");
    let inv = devices::discover_all();
    if inv.real_cameras.is_empty() {
        println!("  real cameras:     none");
    } else {
        for cam in &inv.real_cameras {
            println!("  real camera:      {} ({})", cam.path, cam.name);
        }
    }
    if inv.loopback_cameras.is_empty() {
        println!("  v4l2loopback:     MISSING");
        println!("    hint: {}", modprobe_hint());
    } else {
        for cam in &inv.loopback_cameras {
            println!("  v4l2loopback:     {} ({})", cam.path, cam.name);
        }
    }

    println!("\n=== PipeWire / pactl ===");
    if devices::pipewire::pactl_available() {
        println!(
            "  pactl:            OK (will register {VIRTUAL_SINK_NAME} -> {VIRTUAL_MIC_NAME})"
        );
        println!("  real mics found:  {}", inv.real_mics.len());
    } else {
        println!("  pactl:            UNAVAILABLE");
    }

    if missing_good {
        println!("\nArch Linux hint:");
        println!("  sudo pacman -S gst-plugins-good gst-libav v4l2loopback-dkms");
    }
    ExitCode::SUCCESS
}

struct AppController {
    app: adw::Application,
    hold: Option<gio::ApplicationHoldGuard>,
    config_path: PathBuf,
    config: Config,
    state: AppState,
    devices: DeviceInventory,
    test_sources: bool,
    fallback: ForceFallback,
    virtual_mic: Option<VirtualMicGuard>,
    mic_init_warning: Option<String>,
    pipeline: Option<MediaPipeline>,
    ipc_server: Option<IpcServerHandle>,
    hotkey_manager: HotkeyManager,
    hotkey_error: Option<String>,
    window: Option<MainWindow>,
    overlay: Option<OverlayWindow>,
    save_timer: Option<glib::SourceId>,
    timecode_timer: Option<glib::SourceId>,
    shutting_down: bool,
}

impl AppController {
    fn hydrate_default_devices(&mut self) {
        if self.config.real_camera.is_none()
            && let Some(first) = self.devices.real_cameras.first()
        {
            self.config.real_camera = Some(first.path.clone());
        }
        if self.config.virtual_camera.is_none()
            && let Some(first) = self.devices.loopback_cameras.first()
        {
            self.config.virtual_camera = Some(first.path.clone());
        }
        // Keep real_mic as None by default so original microphone audio is not passed through
        // unless the user explicitly selects a microphone.
    }

    fn refresh_health(&mut self) {
        let mut health = self
            .pipeline
            .as_ref()
            .map(MediaPipeline::health)
            .unwrap_or_default();
        if let Some(ref w) = self.mic_init_warning {
            health.output_warning = Some(match health.output_warning.take() {
                Some(prev) => format!("{prev}; {w}"),
                None => w.clone(),
            });
        }
        self.state.health = health;
    }

    fn any_window_visible(&self) -> bool {
        self.window.as_ref().is_some_and(MainWindow::is_visible)
            || self.overlay.as_ref().is_some_and(OverlayWindow::is_visible)
    }

    fn sync_previews_active(&self) {
        if let Some(ref pipe) = self.pipeline {
            pipe.set_previews_active(self.any_window_visible());
        }
    }

    fn sync_hotkey(&mut self) {
        let parsed = self
            .config
            .hotkey
            .as_deref()
            .and_then(|s| Hotkey::parse(s).ok());
        self.hotkey_error = self.hotkey_manager.apply(parsed.as_ref()).err();
    }

    fn sync_window(&self) {
        let overlay_visible = self.overlay.as_ref().is_some_and(OverlayWindow::is_visible);
        if let Some(ref win) = self.window {
            win.sync_state(
                &self.config,
                &self.state,
                &self.devices,
                overlay_visible,
                self.hotkey_error.as_deref(),
            );
        }
        if let Some(ref ov) = self.overlay {
            ov.sync_state(&self.config, &self.state);
        }
    }

    fn bind_pipeline_paintables(&self) {
        let Some(pipe) = self.pipeline.as_ref() else {
            return;
        };
        let previews = pipe.preview_objects();
        let loop_p = previews.loop_sink.property::<gdk::Paintable>("paintable");
        let live_p = previews.live_sink.property::<gdk::Paintable>("paintable");
        let out_p = previews.out_sink.property::<gdk::Paintable>("paintable");
        if let Some(win) = self.window.as_ref() {
            win.bind_paintables(Some(loop_p.clone()), Some(live_p.clone()), Some(out_p));
        }
        if let Some(ov) = self.overlay.as_ref() {
            ov.bind_paintables(Some(loop_p), Some(live_p));
        }
    }
}

fn rebuild_pipeline(ctrl: &Rc<RefCell<AppController>>) {
    let weak = Rc::downgrade(ctrl);
    let (cfg, state, test_sources, fallback, visible) = {
        let mut c = ctrl.borrow_mut();
        if let Some(mut old) = c.pipeline.take() {
            old.stop();
        }
        c.state.recording = false;
        c.state.recording_elapsed_ms = 0;
        let visible = c.any_window_visible();
        (
            c.config.clone(),
            c.state.clone(),
            c.test_sources,
            c.fallback,
            visible,
        )
    };

    let on_event = move |event: PipelineEvent| {
        let weak_cb = weak.clone();
        glib::idle_add_local_once(move || {
            let Some(ctrl_rc) = weak_cb.upgrade() else {
                return;
            };
            handle_pipeline_event(&ctrl_rc, event);
        });
    };

    match MediaPipeline::start_with_fallback(
        &cfg,
        &state,
        PreviewBackend::Gtk4,
        test_sources,
        fallback,
        on_event,
    ) {
        Ok(pipe) => {
            pipe.set_previews_active(visible);
            let mut c = ctrl.borrow_mut();
            c.pipeline = Some(pipe);
            c.refresh_health();
            c.bind_pipeline_paintables();
            c.sync_window();
        }
        Err(err) => {
            let mut c = ctrl.borrow_mut();
            c.state.health.output_warning = Some(format!("Ошибка запуска пайплайна: {err}"));
            c.sync_window();
            eprintln!("pipeline start failed: {err}");
        }
    }
}

fn handle_pipeline_event(ctrl: &Rc<RefCell<AppController>>, event: PipelineEvent) {
    match event {
        PipelineEvent::Levels(levels) => {
            let mut c = ctrl.borrow_mut();
            c.state.levels = levels;
            if let Some(ref win) = c.window
                && win.is_visible()
            {
                win.update_levels(&levels);
            }
        }
        PipelineEvent::Position { pos_ms, dur_ms } => {
            let mut c = ctrl.borrow_mut();
            c.state.loop_pos_ms = pos_ms;
            c.state.loop_dur_ms = dur_ms;
        }
        PipelineEvent::Degraded { branch, reason } => {
            // Privacy invariant: NEVER alter `state.mode` (never auto-switch Loop -> Live).
            let should_rebuild = {
                let mut guard = ctrl.borrow_mut();
                let c = &mut *guard;
                match branch {
                    DegradedBranch::Loop if !c.fallback.loop_black => {
                        c.fallback.loop_black = true;
                        eprintln!("loop branch degraded ({reason}); falling back to black");
                        true
                    }
                    DegradedBranch::LiveVideo if !c.fallback.live_video_black => {
                        c.fallback.live_video_black = true;
                        eprintln!("live video degraded ({reason}); falling back to black");
                        true
                    }
                    DegradedBranch::LiveAudio if !c.fallback.live_audio_silence => {
                        c.fallback.live_audio_silence = true;
                        eprintln!("live audio degraded ({reason}); falling back to silence");
                        true
                    }
                    _ => false,
                }
            };
            if should_rebuild {
                rebuild_pipeline(ctrl);
            }
        }
        PipelineEvent::FatalError(reason) => {
            let mut c = ctrl.borrow_mut();
            c.state.health.output_warning = Some(format!("Ошибка GStreamer: {reason}"));
            c.sync_window();
            eprintln!("pipeline fatal error: {reason}");
        }
    }
}

fn ensure_window(ctrl: &Rc<RefCell<AppController>>) {
    if ctrl.borrow().window.is_some() {
        return;
    }

    let app = ctrl.borrow().app.clone();
    let weak_act = Rc::downgrade(ctrl);
    let on_action: Rc<dyn Fn(UiAction)> = Rc::new(move |action| {
        let weak_cb = weak_act.clone();
        glib::idle_add_local_once(move || {
            if let Some(c) = weak_cb.upgrade() {
                handle_ui_action(&c, action);
            }
        });
    });

    let weak_vis = Rc::downgrade(ctrl);
    let on_visibility: Rc<dyn Fn(bool)> = Rc::new(move |_visible| {
        let weak_cb = weak_vis.clone();
        glib::idle_add_local_once(move || {
            if let Some(c) = weak_cb.upgrade() {
                c.borrow().sync_previews_active();
            }
        });
    });

    let win = MainWindow::new(&app, on_action, on_visibility);
    {
        let mut c = ctrl.borrow_mut();
        c.window = Some(win);
        c.bind_pipeline_paintables();
        c.sync_window();
    }
}

fn ensure_overlay(ctrl: &Rc<RefCell<AppController>>) {
    if ctrl.borrow().overlay.is_some() {
        return;
    }

    let app = ctrl.borrow().app.clone();
    let weak_act = Rc::downgrade(ctrl);
    let on_action: Rc<dyn Fn(UiAction)> = Rc::new(move |action| {
        let weak_cb = weak_act.clone();
        glib::idle_add_local_once(move || {
            if let Some(c) = weak_cb.upgrade() {
                handle_ui_action(&c, action);
            }
        });
    });

    let weak_vis = Rc::downgrade(ctrl);
    let on_visibility: Rc<dyn Fn(bool)> = Rc::new(move |_visible| {
        let weak_cb = weak_vis.clone();
        glib::idle_add_local_once(move || {
            if let Some(c) = weak_cb.upgrade() {
                let inner = c.borrow();
                inner.sync_previews_active();
                inner.sync_window();
            }
        });
    });

    let ov = OverlayWindow::new(&app, on_action, on_visibility);
    {
        let mut c = ctrl.borrow_mut();
        c.overlay = Some(ov);
        c.bind_pipeline_paintables();
        c.sync_window();
    }
}

fn set_overlay_visible(ctrl: &Rc<RefCell<AppController>>, visible: bool) {
    if visible {
        ensure_overlay(ctrl);
        if let Some(ref ov) = ctrl.borrow().overlay {
            ov.present();
        }
    } else if let Some(ref ov) = ctrl.borrow().overlay {
        ov.hide();
    }
}

fn show_window(ctrl: &Rc<RefCell<AppController>>) {
    ensure_window(ctrl);
    // Present outside any RefCell borrow to avoid re-entrant signal borrows.
    if let Some(ref win) = ctrl.borrow().window {
        win.present();
    }
}

fn hide_window(ctrl: &Rc<RefCell<AppController>>) {
    if let Some(ref win) = ctrl.borrow().window {
        win.hide();
    }
}

fn apply_command(ctrl: &Rc<RefCell<AppController>>, cmd: Command) -> Reply {
    match cmd {
        Command::Show => {
            show_window(ctrl);
            Reply::Ok {
                message: "window shown".into(),
            }
        }
        Command::Hide => {
            hide_window(ctrl);
            Reply::Ok {
                message: "window hidden".into(),
            }
        }
        Command::Status => {
            let c = ctrl.borrow();
            Reply::Status(c.state.snapshot(&c.config))
        }
        Command::Quit => {
            let weak = Rc::downgrade(ctrl);
            glib::idle_add_local_once(move || {
                if let Some(c) = weak.upgrade() {
                    shutdown_controller(&c);
                }
            });
            Reply::Ok {
                message: "shutting down".into(),
            }
        }
        Command::Record(action) => handle_record_command(ctrl, action),
        mode_cmd => {
            // Runtime mode/mic/freeze commands NEVER overwrite startup_mode in config.toml.
            let mut c = ctrl.borrow_mut();
            let effect = c.state.apply_mode_command(&mode_cmd);
            let target = c.state.target_gains(&c.config);
            let crossfade_ms = c.config.crossfade_ms;
            let return_policy = c.config.loop_return;
            let video_src = c.state.mode.video();

            if let Some(ref mut pipe) = c.pipeline {
                let freeze_delay_ms = if effect.video_changed {
                    pipe.select_video(video_src, return_policy, effect.video_switched_to_loop)
                } else {
                    None
                };
                if effect.audio_changed {
                    if let Some(delay_ms) = freeze_delay_ms {
                        pipe.start_audio_ramp_delayed(target, crossfade_ms, delay_ms);
                    } else {
                        pipe.start_audio_ramp(target, crossfade_ms);
                    }
                }
                if let Command::SetFreeze { frozen } = mode_cmd {
                    pipe.set_reference_frozen(frozen);
                }
            }
            c.sync_window();
            Reply::Status(c.state.snapshot(&c.config))
        }
    }
}

fn handle_record_command(ctrl: &Rc<RefCell<AppController>>, action: RecordAction) -> Reply {
    let want_start = {
        let c = ctrl.borrow();
        if c.state.recording_saving {
            return Reply::Error {
                message: "recording is currently finalizing".into(),
            };
        }
        match action {
            RecordAction::Start => true,
            RecordAction::Stop => false,
            RecordAction::Toggle => !c.state.recording,
        }
    };

    if want_start {
        let mut c = ctrl.borrow_mut();
        if c.state.recording {
            return Reply::Status(c.state.snapshot(&c.config));
        }
        let Some(ref pipe) = c.pipeline else {
            return Reply::Error {
                message: "pipeline is not running".into(),
            };
        };
        let path = record::default_recording_path();
        match pipe.start_recording(path, c.config.video.fps) {
            Ok(_) => {
                c.state.recording = true;
                c.state.recording_elapsed_ms = 0;
                c.sync_window();
                Reply::Status(c.state.snapshot(&c.config))
            }
            Err(err) => {
                c.state.health.output_warning = Some(format!("Ошибка записи: {err}"));
                c.sync_window();
                Reply::Error {
                    message: err.to_string(),
                }
            }
        }
    } else {
        let (active, snap) = {
            let mut c = ctrl.borrow_mut();
            let active = c
                .pipeline
                .as_ref()
                .and_then(MediaPipeline::take_active_recording);
            if active.is_none() {
                c.state.recording = false;
                c.state.recording_elapsed_ms = 0;
                c.sync_window();
                return Reply::Status(c.state.snapshot(&c.config));
            }
            c.state.recording = false;
            c.state.recording_saving = true;
            c.sync_window();
            (active, c.state.snapshot(&c.config))
        };

        if let Some(active) = active {
            let (tx, rx) = async_channel::bounded(1);
            let spawn_res = std::thread::Builder::new()
                .name("cyanide-rec-finalize".into())
                .spawn(move || {
                    let res = active.finalize();
                    let _ = tx.send_blocking(res);
                });
            if let Err(err) = spawn_res {
                on_recording_finished(ctrl, Err(format!("Не удалось завершить запись: {err}")));
            } else {
                let weak = Rc::downgrade(ctrl);
                glib::spawn_future_local(async move {
                    let Ok(res) = rx.recv().await else {
                        return;
                    };
                    if let Some(ctrl_rc) = weak.upgrade() {
                        on_recording_finished(&ctrl_rc, res);
                    }
                });
            }
        }

        Reply::Status(snap)
    }
}

fn on_recording_finished(
    ctrl: &Rc<RefCell<AppController>>,
    res: Result<RecordingFinished, String>,
) {
    match res {
        Ok(finished) => {
            {
                let mut guard = ctrl.borrow_mut();
                let c = &mut *guard;
                c.state.recording_saving = false;
                c.state.recording_elapsed_ms = 0;
                c.config.loop_file = Some(finished.path);
                c.fallback.loop_black = false;
            }
            schedule_config_save(ctrl);
            rebuild_pipeline(ctrl);
        }
        Err(err) => {
            let mut c = ctrl.borrow_mut();
            c.state.recording_saving = false;
            c.state.recording_elapsed_ms = 0;
            c.state.health.output_warning = Some(err);
            c.sync_window();
        }
    }
}

fn handle_ui_action(ctrl: &Rc<RefCell<AppController>>, action: UiAction) {
    match action {
        UiAction::Command(cmd) => {
            let _ = apply_command(ctrl, cmd);
        }
        UiAction::PickLoopFile(path) => {
            {
                let mut guard = ctrl.borrow_mut();
                let c = &mut *guard;
                c.config.loop_file = Some(path);
                c.fallback.loop_black = false;
            }
            schedule_config_save(ctrl);
            rebuild_pipeline(ctrl);
        }
        UiAction::RefreshDevices => {
            let (config_changed, needs_rebuild) = {
                let mut guard = ctrl.borrow_mut();
                let c = &mut *guard;
                let inv = devices::discover_all();
                let had_warning = c.state.health.live_warning.is_some()
                    || c.state.health.output_warning.is_some();
                let (changed, rebuild) = apply_device_refresh(&mut c.config, &mut c.fallback, &inv);
                c.devices = inv;
                c.sync_window();
                (changed, rebuild || had_warning)
            };
            if config_changed {
                schedule_config_save(ctrl);
            }
            if needs_rebuild {
                rebuild_pipeline(ctrl);
            }
        }
        UiAction::SetReturnBookmarkCurrent => {
            {
                let mut guard = ctrl.borrow_mut();
                let c = &mut *guard;
                let pos = c
                    .pipeline
                    .as_ref()
                    .and_then(MediaPipeline::loop_position_ms)
                    .map(|(p, _)| p)
                    .unwrap_or(c.state.loop_pos_ms);
                c.config.loop_return = LoopReturnPolicy::Bookmark { position_ms: pos };
                c.sync_window();
            }
            schedule_config_save(ctrl);
        }
        UiAction::ToggleOverlay(visible) => {
            set_overlay_visible(ctrl, visible);
        }
        UiAction::SetHotkeyCapture(capturing) => {
            ctrl.borrow_mut().hotkey_manager.set_suspended(capturing);
        }
        UiAction::UpdateConfig(patch) => {
            let mut needs_rebuild = false;
            let mut audio_gain_changed = false;
            let mut av_delay_changed = false;
            let mut hotkey_changed = false;
            let mut lag_changed = false;
            let mut ping_pong_changed = false;

            {
                let mut guard = ctrl.borrow_mut();
                let c = &mut *guard;
                match patch {
                    ConfigPatch::StartupMode(m) => c.config.startup_mode = m,
                    ConfigPatch::RealCamera(cam) => {
                        if c.config.real_camera != cam {
                            c.config.real_camera = cam;
                            c.fallback.live_video_black = false;
                            needs_rebuild = true;
                        }
                    }
                    ConfigPatch::VirtualCamera(cam) => {
                        if c.config.virtual_camera != cam {
                            c.config.virtual_camera = cam;
                            needs_rebuild = true;
                        }
                    }
                    ConfigPatch::RealMic(mic) => {
                        if c.config.real_mic != mic {
                            c.config.real_mic = mic;
                            c.fallback.live_audio_silence = false;
                            needs_rebuild = true;
                        }
                    }
                    ConfigPatch::Hotkey(hk) => {
                        c.config.hotkey = hk;
                        hotkey_changed = true;
                    }
                    ConfigPatch::VideoFormat(fmt) => {
                        if c.config.video != fmt {
                            c.config.video = fmt;
                            needs_rebuild = true;
                        }
                    }
                    ConfigPatch::LoopVolume(v) => {
                        c.config.loop_volume = v;
                        audio_gain_changed = true;
                    }
                    ConfigPatch::LiveVolume(v) => {
                        c.config.live_volume = v;
                        audio_gain_changed = true;
                    }
                    ConfigPatch::AvDelayMs(ms) => {
                        c.config.av_delay_ms = ms;
                        av_delay_changed = true;
                    }
                    ConfigPatch::CrossfadeMs(ms) => {
                        c.config.crossfade_ms = ms;
                    }
                    ConfigPatch::LoopReturn(policy) => {
                        c.config.loop_return = match (policy, c.config.loop_return) {
                            (
                                LoopReturnPolicy::Bookmark { position_ms: 0 },
                                LoopReturnPolicy::Bookmark { position_ms },
                            ) => LoopReturnPolicy::Bookmark { position_ms },
                            (new_policy, _) => new_policy,
                        };
                    }
                    ConfigPatch::LoopPingPong(enabled) => {
                        c.config.loop_ping_pong = enabled;
                        ping_pong_changed = true;
                    }
                    ConfigPatch::Compare(cmp) => {
                        c.config.compare = cmp;
                    }
                    ConfigPatch::Lag(lag) => {
                        c.config.lag = lag;
                        lag_changed = true;
                    }
                }
                c.config = c.config.clone().sanitized();

                if audio_gain_changed {
                    let target = c.state.target_gains(&c.config);
                    let cf = c.config.crossfade_ms;
                    if let Some(ref mut pipe) = c.pipeline {
                        pipe.start_audio_ramp(target, cf);
                    }
                }
                if av_delay_changed && let Some(ref pipe) = c.pipeline {
                    pipe.set_av_delay_ms(c.config.av_delay_ms);
                }
                if hotkey_changed {
                    c.sync_hotkey();
                }
                if lag_changed && let Some(ref pipe) = c.pipeline {
                    pipe.set_lag_config(c.config.lag);
                }
                if ping_pong_changed && let Some(ref pipe) = c.pipeline {
                    pipe.set_loop_ping_pong(c.config.loop_ping_pong);
                }
                c.sync_window();
            }

            schedule_config_save(ctrl);
            if needs_rebuild {
                rebuild_pipeline(ctrl);
            }
        }
    }
}

fn schedule_config_save(ctrl: &Rc<RefCell<AppController>>) {
    let mut c = ctrl.borrow_mut();
    if let Some(id) = c.save_timer.take() {
        id.remove();
    }
    let weak = Rc::downgrade(ctrl);
    c.save_timer = Some(glib::timeout_add_local_once(
        Duration::from_millis(300),
        move || {
            let Some(ctrl_rc) = weak.upgrade() else {
                return;
            };
            let mut inner = ctrl_rc.borrow_mut();
            inner.save_timer = None;
            if let Err(err) = inner.config.save(&inner.config_path) {
                eprintln!("failed to save config: {err}");
            }
        },
    ));
}

fn shutdown_controller(ctrl: &Rc<RefCell<AppController>>) {
    let app = {
        let mut c = ctrl.borrow_mut();
        if c.shutting_down {
            return;
        }
        c.shutting_down = true;
        if let Some(id) = c.save_timer.take() {
            id.remove();
            let _ = c.config.save(&c.config_path);
        }
        if let Some(id) = c.timecode_timer.take() {
            id.remove();
        }
        if let Some(mut pipe) = c.pipeline.take() {
            pipe.stop();
        }
        c.virtual_mic.take();
        if let Some(server) = c.ipc_server.take() {
            server.shutdown();
        }
        let _ = c.hotkey_manager.apply(None);
        c.overlay.take();
        c.window.take();
        c.hold.take();
        c.app.clone()
    };
    app.quit();
}

fn run_daemon(background: bool, test_sources: bool) -> ExitCode {
    let sock_path = ipc::default_socket_path();
    if !background && let Ok(Reply::Ok { .. }) = ipc::send_command(&sock_path, &Command::Show) {
        return ExitCode::SUCCESS;
    }

    if let Err(err) = gst::init() {
        eprintln!("GStreamer init failed: {err}");
        return ExitCode::FAILURE;
    }
    if let Err(err) = gstgtk4::plugin_register_static() {
        eprintln!("gst-plugin-gtk4 registration failed: {err}");
        return ExitCode::FAILURE;
    }

    let config_path = Config::default_path();
    let config = match Config::load_or_default(&config_path) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("error loading {}: {err}", config_path.display());
            return ExitCode::FAILURE;
        }
    };

    let (ipc_tx, ipc_rx) = async_channel::bounded::<IpcRequest>(32);
    let ipc_server = match IpcServerHandle::start_with_signals(sock_path, ipc_tx) {
        Ok(server) => server,
        Err(err) => {
            eprintln!("IPC server failed to start: {err}");
            return ExitCode::FAILURE;
        }
    };

    let (virtual_mic, mic_init_warning) = if test_sources {
        (None, None)
    } else {
        match VirtualMicGuard::create(&devices::default_pw_state_file()) {
            Ok(guard) => (Some(guard), None),
            Err(err) => (
                None,
                Some(format!(
                    "Не удалось создать виртуальный микрофон PipeWire: {err}"
                )),
            ),
        }
    };

    let app = adw::Application::builder()
        .application_id("io.github.cyanide.VirtualCam")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let startup_bundle = RefCell::new(Some((
        config_path,
        config,
        virtual_mic,
        mic_init_warning,
        ipc_server,
        ipc_rx,
    )));
    let controller_slot: Rc<RefCell<Option<Rc<RefCell<AppController>>>>> =
        Rc::new(RefCell::new(None));

    let slot_activate = Rc::clone(&controller_slot);
    app.connect_activate(move |app| {
        if let Some(existing) = slot_activate.borrow().as_ref() {
            if !background {
                show_window(existing);
            }
            return;
        }

        let Some((config_path, config, virtual_mic, mic_init_warning, ipc_server, ipc_rx)) =
            startup_bundle.borrow_mut().take()
        else {
            return;
        };

        let hold = app.hold();
        let state = AppState::from_config(&config);
        let devices = devices::discover_all();
        let mut ctrl_inner = AppController {
            app: app.clone(),
            hold: Some(hold),
            config_path,
            config,
            state,
            devices,
            test_sources,
            fallback: ForceFallback::default(),
            virtual_mic,
            mic_init_warning,
            pipeline: None,
            ipc_server: Some(ipc_server),
            hotkey_manager: HotkeyManager::new(!test_sources),
            hotkey_error: None,
            window: None,
            overlay: None,
            save_timer: None,
            timecode_timer: None,
            shutting_down: false,
        };
        ctrl_inner.hydrate_default_devices();
        ctrl_inner.sync_hotkey();

        let ctrl = Rc::new(RefCell::new(ctrl_inner));
        *slot_activate.borrow_mut() = Some(Rc::clone(&ctrl));

        rebuild_pipeline(&ctrl);

        // Drain IPC commands on GLib main thread (single-threaded state ownership).
        let ctrl_ipc = Rc::clone(&ctrl);
        glib::spawn_future_local(async move {
            while let Ok(req) = ipc_rx.recv().await {
                let reply = apply_command(&ctrl_ipc, req.command);
                let _ = req.reply.send(reply);
            }
        });

        // Poll MP4 timecode and overlay frame similarity only while a window is visible.
        let weak_tc = Rc::downgrade(&ctrl);
        let tc_id = glib::timeout_add_local(Duration::from_millis(150), move || {
            let Some(ctrl_rc) = weak_tc.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let mut c = ctrl_rc.borrow_mut();
            let main_visible = c.window.as_ref().is_some_and(MainWindow::is_visible);
            let overlay_visible = c.overlay.as_ref().is_some_and(OverlayWindow::is_visible);
            if !main_visible && !overlay_visible {
                return glib::ControlFlow::Continue;
            }
            if main_visible {
                if let Some((pos_ms, dur_ms)) = c
                    .pipeline
                    .as_ref()
                    .and_then(MediaPipeline::loop_position_ms)
                {
                    c.state.loop_pos_ms = pos_ms;
                    c.state.loop_dur_ms = dur_ms;
                }
                let fname = c
                    .config
                    .loop_file
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Нет MP4".into());
                let pos = c.state.loop_pos_ms;
                let dur = c.state.loop_dur_ms;
                let fps = c.config.video.fps;
                let reversed = c
                    .pipeline
                    .as_ref()
                    .is_some_and(MediaPipeline::is_loop_reversed);
                if c.state.recording
                    && let Some(elapsed) = c
                        .pipeline
                        .as_ref()
                        .and_then(MediaPipeline::recording_elapsed_ms)
                {
                    c.state.recording_elapsed_ms = elapsed;
                }
                let rec = c.state.recording;
                let saving = c.state.recording_saving;
                let rec_ms = c.state.recording_elapsed_ms;
                if let Some(ref win) = c.window {
                    win.update_timecode(&fname, pos, dur, fps, reversed);
                    if rec {
                        win.update_recording_timer(rec, saving, rec_ms);
                    }
                }
            }
            if overlay_visible
                && let Some(ref ov) = c.overlay
                && let Some(ref pipe) = c.pipeline
            {
                ov.set_similarity(pipe.similarity());
            }
            glib::ControlFlow::Continue
        });
        ctrl.borrow_mut().timecode_timer = Some(tc_id);

        if !background {
            show_window(&ctrl);
        }
    });

    let code = app.run_with_args::<&str>(&[]);
    if let Some(ctrl) = controller_slot.borrow().as_ref() {
        shutdown_controller(ctrl);
    }
    if code == glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    match parse_cli(std::env::args()) {
        Ok(CliAction::Help) => {
            print_help();
            ExitCode::SUCCESS
        }
        Ok(CliAction::Doctor) => run_doctor(),
        Ok(CliAction::Ctl(cmd)) => run_ctl(&cmd),
        Ok(CliAction::Run {
            background,
            test_sources,
        }) => run_daemon(background, test_sources),
        Err(msg) => {
            eprintln!("error: {msg}");
            print_help();
            ExitCode::FAILURE
        }
    }
}

fn apply_device_refresh(
    cfg: &mut Config,
    fallback: &mut ForceFallback,
    inv: &DeviceInventory,
) -> (bool, bool) {
    let mut config_changed = false;
    if cfg.real_camera.is_none()
        && let Some(first) = inv.real_cameras.first()
    {
        cfg.real_camera = Some(first.path.clone());
        config_changed = true;
    }
    if cfg.virtual_camera.is_none()
        && let Some(first) = inv.loopback_cameras.first()
    {
        cfg.virtual_camera = Some(first.path.clone());
        config_changed = true;
    }

    let had_live_fallback = fallback.live_video_black || fallback.live_audio_silence;
    fallback.live_video_black = false;
    fallback.live_audio_silence = false;

    (config_changed, config_changed || had_live_fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyanide::state::AudioSource;

    #[test]
    fn cli_parses_run_flags_ctl_and_doctor() {
        assert_eq!(
            parse_cli(["cyanide", "--background", "--test-sources"]).unwrap(),
            CliAction::Run {
                background: true,
                test_sources: true,
            }
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

    #[test]
    fn refresh_devices_resets_fallbacks_and_requests_rebuild() {
        let mut cfg = Config::default();
        let mut fb = ForceFallback {
            loop_black: false,
            live_video_black: true,
            live_audio_silence: true,
        };
        let inv = DeviceInventory {
            real_cameras: vec![devices::VideoDevice {
                path: "/dev/video0".into(),
                name: "HD Camera".into(),
                is_loopback: false,
            }],
            loopback_cameras: vec![devices::VideoDevice {
                path: "/dev/video10".into(),
                name: "VirtualCam".into(),
                is_loopback: true,
            }],
            real_mics: vec![],
        };
        let (config_changed, needs_rebuild) = apply_device_refresh(&mut cfg, &mut fb, &inv);
        assert!(config_changed);
        assert!(needs_rebuild);
        assert!(!fb.live_video_black);
        assert!(!fb.live_audio_silence);
        assert_eq!(cfg.virtual_camera.as_deref(), Some("/dev/video10"));
    }
}
