pub mod compare;
pub mod controls;
pub mod overlay;
pub mod style;
pub mod workspace;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::config::{
    CompareConfig, CompareMode, Config, LagConfig, LoopReturnPolicy, StartupMode, UiLayoutConfig,
    VideoFormat,
};
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
    UiLayout(UiLayoutConfig),
}

#[derive(Debug, Clone)]
pub enum UiAction {
    Command(Command),
    UpdateConfig(ConfigPatch),
    PickLoopFile(PathBuf),
    RefreshDevices,
    SetReturnBookmarkCurrent,
    SeekLoopMs(u64),
    ToggleOverlay(bool),
    RefreshOverlay,
    ToggleFullscreen(bool),
    SetHotkeyCapture(bool),
}

pub struct MainWindow {
    window: adw::ApplicationWindow,
    toolbar_view: adw::ToolbarView,
    paned: gtk4::Paned,
    header: adw::HeaderBar,
    footer_box: gtk4::Box,
    device_pill: gtk4::Label,
    source_pill: gtk4::Label,
    toggle_btn: gtk4::Button,
    record_btn: gtk4::Button,
    overlay_btn: gtk4::ToggleButton,
    fullscreen_btn: gtk4::ToggleButton,
    sidebar_btn: gtk4::ToggleButton,
    banner: adw::Banner,
    workspace: VideoWorkspace,
    controls: ControlPanel,
    footer_label: gtk4::Label,
    diag_btn: gtk4::MenuButton,
    diag_label: gtk4::Label,
    lag_chip: gtk4::Label,
    hotkey_chip: gtk4::Label,
    layout: Rc<Cell<UiLayoutConfig>>,
    compare_cfg: Rc<Cell<CompareConfig>>,
    frozen_state: Rc<Cell<bool>>,
    fullscreen_active: Rc<Cell<bool>>,
    syncing: Rc<Cell<bool>>,
}

impl MainWindow {
    pub fn new(
        app: &adw::Application,
        initial_layout: UiLayoutConfig,
        on_action: Rc<dyn Fn(UiAction)>,
        on_visibility: Rc<dyn Fn(bool)>,
    ) -> Self {
        install_app_style();
        let syncing = Rc::new(Cell::new(false));
        let layout_cell = Rc::new(Cell::new(initial_layout.sanitized()));
        let compare_cfg = Rc::new(Cell::new(CompareConfig::default()));
        let frozen_state = Rc::new(Cell::new(false));
        let fullscreen_active = Rc::new(Cell::new(false));

        let init_l = layout_cell.get();
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Cyanide Studio")
            .default_width(init_l.window_width)
            .default_height(init_l.window_height)
            .hide_on_close(true)
            .build();

        let vis_cb = Rc::clone(&on_visibility);
        window.connect_visible_notify(move |w| {
            vis_cb(w.is_visible());
        });

        // --- HeaderBar ---
        let header = adw::HeaderBar::new();

        let device_pill = gtk4::Label::new(Some("VirtualCam"));
        device_pill.add_css_class("cyanide-status-pill");
        device_pill.add_css_class("live");
        header.pack_start(&device_pill);

        let source_pill = gtk4::Label::new(Some("В эфире: Камера"));
        source_pill.add_css_class("cyanide-status-pill");
        source_pill.add_css_class("live");

        let toggle_btn = gtk4::Button::with_label("Переключить");
        toggle_btn.add_css_class("suggested-action");
        toggle_btn.add_css_class("cyanide-primary-cta");
        toggle_btn.set_tooltip_text(Some(
            "Переключить источник виртуальной камеры (Видео ↔ Камера)",
        ));
        {
            let act = Rc::clone(&on_action);
            toggle_btn.connect_clicked(move |_| act(UiAction::Command(Command::Toggle)));
        }

        let center_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .valign(gtk4::Align::Center)
            .build();
        center_box.append(&source_pill);
        center_box.append(&toggle_btn);
        header.set_title_widget(Some(&center_box));

        let sidebar_btn = gtk4::ToggleButton::new();
        sidebar_btn.set_icon_name("sidebar-show-right-symbolic");
        sidebar_btn.set_active(init_l.sidebar_visible);
        sidebar_btn.set_tooltip_text(Some("Показать/скрыть панель выхода и настроек"));
        header.pack_end(&sidebar_btn);

        let fullscreen_btn = gtk4::ToggleButton::new();
        fullscreen_btn.set_icon_name("view-fullscreen-symbolic");
        fullscreen_btn.set_tooltip_text(Some("Полноэкранное сравнение (F11)"));
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            fullscreen_btn.connect_toggled(move |b| {
                if !s.get() {
                    act(UiAction::ToggleFullscreen(b.is_active()));
                }
            });
        }
        header.pack_end(&fullscreen_btn);

        let overlay_btn = make_icon_label_toggle("view-pin-symbolic", "Оверлей");
        overlay_btn.set_tooltip_text(Some("Открыть/закрыть полупрозрачное плавающее окно"));
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
        record_btn.set_tooltip_text(Some("Записать видео с камеры в MP4"));
        {
            let act = Rc::clone(&on_action);
            record_btn.connect_clicked(move |_| {
                act(UiAction::Command(Command::Record(RecordAction::Toggle)))
            });
        }
        header.pack_end(&record_btn);

        let banner = adw::Banner::new("");
        banner.set_revealed(false);

        // --- Resizable Two-Pane Studio Layout ---
        let workspace = VideoWorkspace::new(Rc::clone(&on_action), Rc::clone(&syncing));
        let controls = ControlPanel::new(&window, Rc::clone(&on_action), Rc::clone(&syncing));
        controls.widget.set_visible(init_l.sidebar_visible);

        let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
        paned.set_start_child(Some(&workspace.widget));
        paned.set_end_child(Some(&controls.widget));
        paned.set_resize_start_child(true);
        paned.set_resize_end_child(false);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);
        paned.set_wide_handle(true);
        let max_pos = (init_l.window_width - 380).max(420);
        paned.set_position(init_l.split_position.clamp(420, max_pos));
        paned.set_vexpand(true);
        paned.set_hexpand(true);

        {
            let panel = controls.widget.clone();
            let l_cell = Rc::clone(&layout_cell);
            let s = Rc::clone(&syncing);
            let fs = Rc::clone(&fullscreen_active);
            let act = Rc::clone(&on_action);
            sidebar_btn.connect_toggled(move |b| {
                if fs.get() {
                    return;
                }
                let shown = b.is_active();
                panel.set_visible(shown);
                if !s.get() {
                    let mut l = l_cell.get();
                    l.sidebar_visible = shown;
                    l_cell.set(l);
                    act(UiAction::UpdateConfig(ConfigPatch::UiLayout(l)));
                }
            });
        }

        {
            let l_cell = Rc::clone(&layout_cell);
            let s = Rc::clone(&syncing);
            let fs = Rc::clone(&fullscreen_active);
            let act = Rc::clone(&on_action);
            let sidebar_ref = controls.widget.clone();
            paned.connect_position_notify(move |p| {
                if s.get() || fs.get() || !sidebar_ref.is_visible() {
                    return;
                }
                let pos = p.position();
                let mut l = l_cell.get();
                if (l.split_position - pos).abs() >= 4 {
                    l.split_position = pos;
                    l_cell.set(l);
                    act(UiAction::UpdateConfig(ConfigPatch::UiLayout(l)));
                }
            });
        }

        {
            let l_cell = Rc::clone(&layout_cell);
            let fs = Rc::clone(&fullscreen_active);
            let paned_ref = paned.clone();
            let act = Rc::clone(&on_action);
            window.connect_close_request(move |w| {
                if !fs.get() {
                    let mut l = l_cell.get();
                    let (ww, wh) = (w.width(), w.height());
                    if ww >= 960 && wh >= 620 {
                        l.window_width = ww;
                        l.window_height = wh;
                    }
                    if paned_ref.position() >= 380 {
                        l.split_position = paned_ref.position();
                    }
                    l_cell.set(l);
                    act(UiAction::UpdateConfig(ConfigPatch::UiLayout(l)));
                }
                glib::Propagation::Proceed
            });
        }

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content_box.append(&banner);
        content_box.append(&paned);

        // --- Bottom Studio Status Strip & Diagnostics Popover ---
        let footer_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        footer_box.add_css_class("cyanide-footer");

        let footer_label = gtk4::Label::new(Some("1280×720 · 30 FPS · VirtualCam"));
        footer_label.set_xalign(0.0);
        footer_label.set_hexpand(true);
        footer_label.add_css_class("cyanide-caption");

        let lag_chip = gtk4::Label::new(Some("ЛАГ АКТИВЕН"));
        lag_chip.add_css_class("cyanide-lag-chip");
        lag_chip.set_visible(false);

        let hotkey_chip = gtk4::Label::new(None);
        hotkey_chip.add_css_class("cyanide-kbd-chip");
        hotkey_chip.set_visible(false);

        let diag_label = gtk4::Label::new(Some("Ошибок не обнаружено. Все ветки работают штатно."));
        diag_label.set_wrap(true);
        diag_label.set_xalign(0.0);
        diag_label.set_selectable(true);
        diag_label.set_max_width_chars(56);
        diag_label.add_css_class("cyanide-caption");

        let diag_pop_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(8)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        let diag_title = gtk4::Label::new(Some("Диагностика устройств и конвейера"));
        diag_title.set_xalign(0.0);
        diag_title.add_css_class("cyanide-section-title");
        diag_pop_box.append(&diag_title);
        diag_pop_box.append(&diag_label);

        let diag_popover = gtk4::Popover::builder().child(&diag_pop_box).build();
        let diag_btn = gtk4::MenuButton::builder()
            .label("Диагностика: OK")
            .popover(&diag_popover)
            .tooltip_text("Показать состояние виртуальных устройств и предупреждения")
            .build();
        diag_btn.add_css_class("cyanide-diag-btn");

        footer_box.append(&footer_label);
        footer_box.append(&lag_chip);
        footer_box.append(&hotkey_chip);
        footer_box.append(&diag_btn);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&content_box));
        toolbar_view.add_bottom_bar(&footer_box);
        window.set_content(Some(&toolbar_view));

        // --- Window Keyboard Shortcuts ---
        let win_key = gtk4::EventControllerKey::new();
        {
            let act = Rc::clone(&on_action);
            let fs = Rc::clone(&fullscreen_active);
            let cmp = Rc::clone(&compare_cfg);
            let frozen = Rc::clone(&frozen_state);
            let win_weak = window.downgrade();
            win_key.connect_key_pressed(move |_, key, _, _| {
                if let Some(w) = win_weak.upgrade()
                    && let Some(focused) = gtk4::prelude::RootExt::focus(&w)
                    && focused.is::<gtk4::Editable>()
                {
                    return glib::Propagation::Proceed;
                }
                match key {
                    gdk::Key::F11 => {
                        act(UiAction::ToggleFullscreen(!fs.get()));
                        glib::Propagation::Stop
                    }
                    gdk::Key::Escape if fs.get() => {
                        act(UiAction::ToggleFullscreen(false));
                        glib::Propagation::Stop
                    }
                    gdk::Key::_1 => {
                        let mut c = cmp.get();
                        c.mode = CompareMode::Overlay;
                        cmp.set(c);
                        act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
                        glib::Propagation::Stop
                    }
                    gdk::Key::_2 => {
                        let mut c = cmp.get();
                        c.mode = CompareMode::SideBySide;
                        cmp.set(c);
                        act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
                        glib::Propagation::Stop
                    }
                    gdk::Key::_3 => {
                        let mut c = cmp.get();
                        c.mode = CompareMode::Split;
                        cmp.set(c);
                        act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
                        glib::Propagation::Stop
                    }
                    gdk::Key::space => {
                        let next_frozen = !frozen.get();
                        act(UiAction::Command(Command::SetFreeze {
                            frozen: next_frozen,
                        }));
                        glib::Propagation::Stop
                    }
                    gdk::Key::r | gdk::Key::R => {
                        let mut c = cmp.get();
                        c.reset_transforms();
                        c.opacity = 0.5;
                        c.split_pos = 0.5;
                        cmp.set(c);
                        act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
                        glib::Propagation::Stop
                    }
                    gdk::Key::bracketleft => {
                        let mut c = cmp.get();
                        if c.mode == CompareMode::Overlay {
                            c.opacity = (c.opacity - 0.1).clamp(0.0, 1.0);
                            cmp.set(c);
                            act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
                            return glib::Propagation::Stop;
                        }
                        glib::Propagation::Proceed
                    }
                    gdk::Key::bracketright => {
                        let mut c = cmp.get();
                        if c.mode == CompareMode::Overlay {
                            c.opacity = (c.opacity + 0.1).clamp(0.0, 1.0);
                            cmp.set(c);
                            act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
                            return glib::Propagation::Stop;
                        }
                        glib::Propagation::Proceed
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
        }
        window.add_controller(win_key);

        Self {
            window,
            toolbar_view,
            paned,
            header,
            footer_box,
            device_pill,
            source_pill,
            toggle_btn,
            record_btn,
            overlay_btn,
            fullscreen_btn,
            sidebar_btn,
            banner,
            workspace,
            controls,
            footer_label,
            diag_btn,
            diag_label,
            lag_chip,
            hotkey_chip,
            layout: layout_cell,
            compare_cfg,
            frozen_state,
            fullscreen_active,
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

    pub fn set_fullscreen(&self, fullscreen: bool) {
        let prev = self.syncing.replace(true);
        self.fullscreen_active.set(fullscreen);
        self.fullscreen_btn.set_active(fullscreen);
        self.header.set_visible(!fullscreen);
        self.footer_box.set_visible(!fullscreen);
        self.toolbar_view.set_reveal_top_bars(!fullscreen);
        self.toolbar_view.set_reveal_bottom_bars(!fullscreen);
        if fullscreen {
            self.controls.widget.set_visible(false);
            self.window.fullscreen();
        } else {
            self.window.unfullscreen();
            self.controls
                .widget
                .set_visible(self.layout.get().sidebar_visible);
        }
        self.workspace.set_fullscreen(fullscreen);
        self.syncing.set(prev);
    }

    pub fn snapshot_layout(&self) -> UiLayoutConfig {
        let mut l = self.layout.get();
        if !self.fullscreen_active.get() {
            let (ww, wh) = (self.window.width(), self.window.height());
            if ww >= 960 && wh >= 620 {
                l.window_width = ww;
                l.window_height = wh;
            }
            if l.sidebar_visible && self.paned.position() >= 380 {
                l.split_position = self.paned.position();
            }
        }
        l.sanitized()
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

        self.compare_cfg.set(cfg.compare);
        self.frozen_state.set(state.frozen_reference);
        self.layout.set(cfg.ui);

        if !self.fullscreen_active.get() {
            self.sidebar_btn.set_active(cfg.ui.sidebar_visible);
            self.controls.widget.set_visible(cfg.ui.sidebar_visible);
            let avail_w = self.window.width().max(cfg.ui.window_width);
            let clamped_pos = cfg.ui.split_position.clamp(420, (avail_w - 380).max(420));
            if (self.paned.position() - clamped_pos).abs() >= 2 {
                self.paned.set_position(clamped_pos);
            }
        }

        let target_label = match state.mode.video() {
            VideoSource::Live => "Включить Видео",
            VideoSource::Loop => "Включить Камеру",
        };
        if let Some(ref hk) = cfg.hotkey {
            self.toggle_btn.set_label(&format!("{target_label} ({hk})"));
            self.hotkey_chip.set_label(hk);
            self.hotkey_chip.set_visible(true);
        } else {
            self.toggle_btn.set_label(target_label);
            self.hotkey_chip.set_visible(false);
        }
        if self.overlay_btn.is_active() != overlay_visible {
            self.overlay_btn.set_active(overlay_visible);
        }

        // VirtualCam device status pill (independent of selected video source).
        for cls in ["live", "warn"] {
            self.device_pill.remove_css_class(cls);
        }
        if state.health.output_warning.is_some() || cfg.virtual_camera.is_none() {
            self.device_pill.set_label("Заглушка");
            self.device_pill.add_css_class("warn");
            self.device_pill.set_tooltip_text(
                state
                    .health
                    .output_warning
                    .as_deref()
                    .or(Some("Устройство v4l2loopback не найдено")),
            );
        } else {
            self.device_pill.set_label("VirtualCam");
            self.device_pill.add_css_class("live");
            self.device_pill
                .set_tooltip_text(cfg.virtual_camera.as_deref());
        }

        // Active output source pill (never masked by recording state).
        for cls in ["live", "loop", "custom"] {
            self.source_pill.remove_css_class(cls);
        }
        let (src_text, src_cls) = match state.mode {
            StreamMode::Loop => ("В эфире: Видео", "loop"),
            StreamMode::Live => ("В эфире: Камера", "live"),
            StreamMode::Custom { video, .. } => match video {
                VideoSource::Loop => ("В эфире: Видео (аудио отд.)", "custom"),
                VideoSource::Live => ("В эфире: Камера (аудио отд.)", "custom"),
            },
        };
        self.source_pill.set_label(src_text);
        self.source_pill.add_css_class(src_cls);

        let vcam_str = cfg.virtual_camera.as_deref().unwrap_or("нет v4l2loopback");
        let mic_str = if state.virtual_mic_enabled {
            match state.mode.audio() {
                AudioSource::File => "Звук MP4",
                AudioSource::Mic => "Микрофон",
                AudioSource::Mix => "Смесь",
                AudioSource::Silence => "Тишина",
            }
        } else {
            "Mute"
        };
        self.footer_label.set_label(&format!(
            "{}×{} · {} FPS · Выход: {} · VirtualCamMic: {}",
            cfg.video.width, cfg.video.height, cfg.video.fps, vcam_str, mic_str
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
            self.diag_btn.set_label("Диагностика: OK");
            self.diag_btn.remove_css_class("has-warning");
            self.diag_label
                .set_label("Ошибок не обнаружено. Все ветки работают штатно.");
        } else {
            let has_fatal = warnings.iter().any(|w| w.starts_with("Ошибка"));
            if has_fatal {
                self.banner.set_title(&warnings.join(" | "));
                self.banner.set_revealed(true);
            } else {
                self.banner.set_revealed(false);
            }
            self.diag_btn
                .set_label(&format!("⚠ Диагностика ({})", warnings.len()));
            self.diag_btn.add_css_class("has-warning");
            self.diag_label.set_label(&warnings.join("\n\n"));
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
