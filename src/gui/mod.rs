pub mod compare;
pub mod controls;
pub mod overlay;
pub mod style;
pub mod workspace;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::config::{CompareConfig, Config, LagConfig, LoopReturnPolicy, StartupMode, VideoFormat};
use crate::devices::DeviceInventory;
use crate::gui::controls::ControlPanel;
use crate::gui::style::install_app_style;
use crate::gui::workspace::VideoWorkspace;
use crate::state::{
    AppState, AudioLevels, AudioSource, Command, RecordAction, StreamMode, VideoSource,
};

#[derive(Debug, Clone)]
pub enum ConfigPatch {
    StartupMode(StartupMode),
    RealCamera(Option<String>),
    VirtualCamera(Option<String>),
    RealMic(Option<String>),
    Hotkey(Option<String>),
    VideoFormat(VideoFormat),
    LoopVolume(f64),
    LiveVolume(f64),
    AvDelayMs(i32),
    CrossfadeMs(u32),
    LoopReturn(LoopReturnPolicy),
    LoopPingPong(bool),
    Compare(CompareConfig),
    Lag(LagConfig),
}

#[derive(Debug, Clone)]
pub enum UiAction {
    Command(Command),
    UpdateConfig(ConfigPatch),
    PickLoopFile(PathBuf),
    RefreshDevices,
    SetReturnBookmarkCurrent,
    ToggleOverlay(bool),
    RefreshOverlay,
    SetHotkeyCapture(bool),
}

pub struct MainWindow {
    window: adw::ApplicationWindow,
    status_pill: gtk4::Label,
    toggle_btn: gtk4::Button,
    record_btn: gtk4::Button,
    overlay_btn: gtk4::ToggleButton,
    banner: adw::Banner,
    workspace: VideoWorkspace,
    controls: ControlPanel,
    footer_label: gtk4::Label,
    format_label: gtk4::Label,
    lag_chip: gtk4::Label,
    hotkey_chip: gtk4::Label,
    syncing: Rc<Cell<bool>>,
}

impl MainWindow {
    pub fn new(
        app: &adw::Application,
        on_action: Rc<dyn Fn(UiAction)>,
        on_visibility: Rc<dyn Fn(bool)>,
    ) -> Self {
        install_app_style();
        let syncing = Rc::new(Cell::new(false));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Cyanide Studio")
            .default_width(1280)
            .default_height(800)
            .hide_on_close(true)
            .build();

        let vis_cb = Rc::clone(&on_visibility);
        window.connect_visible_notify(move |w| {
            vis_cb(w.is_visible());
        });

        // --- HeaderBar ---
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new("Cyanide Studio", "")));

        let status_pill = gtk4::Label::new(Some("● LIVE · Камера"));
        status_pill.add_css_class("cyanide-status-pill");
        status_pill.add_css_class("live");
        header.pack_start(&status_pill);

        let sidebar_btn = gtk4::ToggleButton::new();
        sidebar_btn.set_icon_name("sidebar-show-right-symbolic");
        sidebar_btn.set_active(true);
        sidebar_btn.set_tooltip_text(Some("Показать/скрыть боковую панель"));
        header.pack_end(&sidebar_btn);

        let toggle_btn = gtk4::Button::with_label("Переключить");
        toggle_btn.add_css_class("suggested-action");
        toggle_btn.add_css_class("pill");
        {
            let act = Rc::clone(&on_action);
            toggle_btn.connect_clicked(move |_| act(UiAction::Command(Command::Toggle)));
        }
        header.pack_end(&toggle_btn);

        let overlay_btn = make_icon_label_toggle("view-pin-symbolic", "Плавающее");
        overlay_btn.set_tooltip_text(Some("Открыть/закрыть полупрозрачное окно совмещения"));
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            overlay_btn.connect_toggled(move |b| {
                if !s.get() {
                    act(UiAction::ToggleOverlay(b.is_active()));
                }
            });
        }
        header.pack_end(&overlay_btn);

        let record_btn = gtk4::Button::with_label("● Запись");
        record_btn.set_tooltip_text(Some("Записать видео с камеры на лету в MP4"));
        {
            let act = Rc::clone(&on_action);
            record_btn.connect_clicked(move |_| {
                act(UiAction::Command(Command::Record(RecordAction::Toggle)))
            });
        }
        header.pack_end(&record_btn);

        let banner = adw::Banner::new("");
        banner.set_revealed(false);

        // --- Main Split Studio Layout ---
        let workspace = VideoWorkspace::new(Rc::clone(&on_action), Rc::clone(&syncing));
        let controls = ControlPanel::new(&window, Rc::clone(&on_action), Rc::clone(&syncing));
        let sidebar_sep = gtk4::Separator::new(gtk4::Orientation::Vertical);

        {
            let panel = controls.widget.clone();
            let sep = sidebar_sep.clone();
            sidebar_btn.connect_toggled(move |b| {
                let shown = b.is_active();
                panel.set_visible(shown);
                sep.set_visible(shown);
            });
        }

        let body = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(0)
            .vexpand(true)
            .build();
        body.append(&workspace.widget);
        body.append(&sidebar_sep);
        body.append(&controls.widget);

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content_box.append(&banner);
        content_box.append(&body);

        // --- Bottom Studio Status Strip ---
        let footer_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
        footer_box.add_css_class("cyanide-footer");

        let footer_label = gtk4::Label::new(Some("Режим: Настоящая камера · Аудио: Микрофон"));
        footer_label.set_xalign(0.0);
        footer_label.set_hexpand(true);
        footer_label.add_css_class("caption");
        footer_label.add_css_class("dim-label");

        let lag_chip = gtk4::Label::new(Some("ЛАГ АКТИВЕН"));
        lag_chip.add_css_class("cyanide-lag-chip");
        lag_chip.set_visible(false);

        let format_label = gtk4::Label::new(Some("1280×720 · 30 FPS"));
        format_label.add_css_class("caption");
        format_label.add_css_class("numeric");
        format_label.add_css_class("dim-label");

        let hotkey_chip = gtk4::Label::new(None);
        hotkey_chip.add_css_class("cyanide-kbd-chip");
        hotkey_chip.set_visible(false);

        footer_box.append(&footer_label);
        footer_box.append(&lag_chip);
        footer_box.append(&format_label);
        footer_box.append(&hotkey_chip);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&content_box));
        toolbar_view.add_bottom_bar(&footer_box);
        window.set_content(Some(&toolbar_view));

        Self {
            window,
            status_pill,
            toggle_btn,
            record_btn,
            overlay_btn,
            banner,
            workspace,
            controls,
            footer_label,
            format_label,
            lag_chip,
            hotkey_chip,
            syncing,
        }
    }

    pub fn bind_paintables(
        &self,
        loop_p: Option<gdk::Paintable>,
        live_p: Option<gdk::Paintable>,
        out_p: Option<gdk::Paintable>,
    ) {
        self.workspace.bind_paintables(loop_p, live_p);
        self.controls.bind_output_paintable(out_p);
    }

    pub fn sync_state(
        &self,
        cfg: &Config,
        state: &AppState,
        devices: &DeviceInventory,
        overlay_visible: bool,
        hotkey_error: Option<&str>,
    ) {
        let prev = self.syncing.replace(true);

        if let Some(ref hk) = cfg.hotkey {
            self.toggle_btn.set_label(&format!("Переключить ({hk})"));
            self.hotkey_chip.set_label(hk);
            self.hotkey_chip.set_visible(true);
        } else {
            self.toggle_btn.set_label("Переключить");
            self.hotkey_chip.set_visible(false);
        }
        if self.overlay_btn.is_active() != overlay_visible {
            self.overlay_btn.set_active(overlay_visible);
        }

        let (mode_short, mode_text, pill_cls) = match state.mode {
            StreamMode::Loop => ("● LOOP · Видео", "Зацикленное видео", "loop"),
            StreamMode::Live => ("● LIVE · Камера", "Настоящая камера", "live"),
            StreamMode::Custom { video, .. } => match video {
                VideoSource::Loop => ("● CUSTOM · MP4", "Custom (Видео: MP4)", "custom"),
                VideoSource::Live => ("● CUSTOM · Камера", "Custom (Видео: Камера)", "custom"),
            },
        };
        let audio_text = if !state.virtual_mic_enabled {
            "Выключён (Mute)"
        } else {
            match state.mode.audio() {
                AudioSource::File => "Звук MP4",
                AudioSource::Mic => "Настоящий микрофон",
                AudioSource::Mix => "Смесь (MP4 + микрофон)",
                AudioSource::Silence => "Тишина",
            }
        };

        for cls in ["live", "loop", "custom", "recording"] {
            self.status_pill.remove_css_class(cls);
        }
        if state.recording {
            self.status_pill.set_label("● ЗАПИСЬ");
            self.status_pill.add_css_class("recording");
        } else {
            self.status_pill.set_label(mode_short);
            self.status_pill.add_css_class(pill_cls);
        }

        self.footer_label
            .set_label(&format!("Режим: {mode_text} · Аудио: {audio_text}"));
        self.format_label.set_label(&format!(
            "{}×{} · {} FPS",
            cfg.video.width, cfg.video.height, cfg.video.fps
        ));
        self.lag_chip
            .set_visible(cfg.lag.enabled || cfg.lag.freeze_on_switch);

        let warnings: Vec<&str> = [
            state.health.output_warning.as_deref(),
            state.health.loop_warning.as_deref(),
            state.health.live_warning.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        if warnings.is_empty() {
            self.banner.set_revealed(false);
        } else {
            self.banner.set_title(&warnings.join(" | "));
            self.banner.set_revealed(true);
        }

        self.workspace.sync_state(cfg, state);
        self.controls.sync_state(cfg, state, devices, hotkey_error);
        self.apply_recording_ui(
            state.recording,
            state.recording_saving,
            state.recording_elapsed_ms,
        );
        self.syncing.set(prev);
    }

    pub fn update_recording_timer(&self, recording: bool, saving: bool, elapsed_ms: u64) {
        self.apply_recording_ui(recording, saving, elapsed_ms);
    }

    fn apply_recording_ui(&self, recording: bool, saving: bool, elapsed_ms: u64) {
        if saving {
            self.record_btn.set_label("Сохранение…");
            self.record_btn.set_sensitive(false);
            self.record_btn.remove_css_class("destructive-action");
        } else if recording {
            let secs = elapsed_ms / 1000;
            self.record_btn
                .set_label(&format!("■ {:02}:{:02}", secs / 60, secs % 60));
            self.record_btn.set_sensitive(true);
            self.record_btn.add_css_class("destructive-action");
        } else {
            self.record_btn.set_label("● Запись");
            self.record_btn.set_sensitive(true);
            self.record_btn.remove_css_class("destructive-action");
        }
        self.controls
            .update_recording_ui(recording, saving, elapsed_ms);
    }

    pub fn update_levels(&self, levels: &AudioLevels) {
        self.controls.update_levels(levels);
    }

    pub fn update_timecode(
        &self,
        file_name: &str,
        pos_ms: u64,
        dur_ms: Option<u64>,
        reversed: bool,
    ) {
        self.workspace
            .update_timecode(file_name, pos_ms, dur_ms, reversed);
    }

    pub fn update_similarity(&self, score: f32) {
        self.workspace.update_similarity(score);
    }

    pub fn present(&self) {
        self.window.set_visible(true);
        self.window.present();
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }
}

fn make_icon_label_toggle(icon_name: &str, label: &str) -> gtk4::ToggleButton {
    let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    content.append(&gtk4::Image::from_icon_name(icon_name));
    content.append(&gtk4::Label::new(Some(label)));
    let btn = gtk4::ToggleButton::new();
    btn.set_child(Some(&content));
    btn
}
