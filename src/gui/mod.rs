pub mod compare;
pub mod controls;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::config::{
    CompareConfig, CompareMode, Config, LoopReturnPolicy, StartupMode, VideoFormat,
};
use crate::devices::DeviceInventory;
use crate::gui::compare::CompareView;
use crate::gui::controls::ControlPanel;
use crate::state::{AppState, AudioLevels, AudioSource, Command, StreamMode, VideoSource};

#[derive(Debug, Clone)]
pub enum ConfigPatch {
    StartupMode(StartupMode),
    RealCamera(Option<String>),
    VirtualCamera(Option<String>),
    RealMic(Option<String>),
    VideoFormat(VideoFormat),
    LoopVolume(f64),
    LiveVolume(f64),
    AvDelayMs(i32),
    CrossfadeMs(u32),
    LoopReturn(LoopReturnPolicy),
    Compare(CompareConfig),
}

#[derive(Debug, Clone)]
pub enum UiAction {
    Command(Command),
    UpdateConfig(ConfigPatch),
    PickLoopFile(PathBuf),
    RefreshDevices,
    SetReturnBookmarkCurrent,
}

pub struct MainWindow {
    window: adw::ApplicationWindow,
    status_pill: gtk4::Label,
    banner: adw::Banner,
    loop_info_label: gtk4::Label,
    live_info_label: gtk4::Label,
    out_picture: gtk4::Picture,
    compare_view: CompareView,
    controls: ControlPanel,
    footer_label: gtk4::Label,
    syncing: Rc<Cell<bool>>,
    compare_cfg: Rc<Cell<CompareConfig>>,
    mode_overlay_btn: gtk4::ToggleButton,
    mode_side_btn: gtk4::ToggleButton,
    mode_split_btn: gtk4::ToggleButton,
    opacity_scale: gtk4::Scale,
    grid_btn: gtk4::ToggleButton,
    center_btn: gtk4::ToggleButton,
    guides_btn: gtk4::ToggleButton,
    guide_x_scale: gtk4::Scale,
    guide_y_scale: gtk4::Scale,
    mirror_btn: gtk4::ToggleButton,
    freeze_btn: gtk4::ToggleButton,
    zoom_scale: gtk4::Scale,
    pan_x_scale: gtk4::Scale,
    pan_y_scale: gtk4::Scale,
}

impl MainWindow {
    pub fn new(
        app: &adw::Application,
        on_action: Rc<dyn Fn(UiAction)>,
        on_visibility: Rc<dyn Fn(bool)>,
    ) -> Self {
        let syncing = Rc::new(Cell::new(false));
        let compare_cfg = Rc::new(Cell::new(CompareConfig::default()));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("VirtualCam Studio — Cyanide")
            .default_width(1260)
            .default_height(800)
            .hide_on_close(true)
            .build();

        let vis_cb = Rc::clone(&on_visibility);
        window.connect_visible_notify(move |w| {
            vis_cb(w.is_visible());
        });

        let header = adw::HeaderBar::new();
        let status_pill = gtk4::Label::new(Some("Трансляция активна"));
        status_pill.add_css_class("caption");
        status_pill.add_css_class("accent");
        header.pack_start(&status_pill);

        let toggle_btn = gtk4::Button::with_label("Переключить (SUPER+F8)");
        toggle_btn.add_css_class("suggested-action");
        {
            let act = Rc::clone(&on_action);
            toggle_btn.connect_clicked(move |_| act(UiAction::Command(Command::Toggle)));
        }
        header.pack_end(&toggle_btn);

        let banner = adw::Banner::new("");
        banner.set_revealed(false);

        // --- Left/Center Video Workspace ---
        let left_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(8)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(6)
            .hexpand(true)
            .vexpand(true)
            .build();

        // Top 3-area summary strip: Loop info, Live info, Virtual Camera Output preview
        let top_strip = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .homogeneous(false)
            .build();

        let loop_info_label = gtk4::Label::new(Some("Не выбран · 00:00 / --:--"));
        loop_info_label.set_xalign(0.0);
        loop_info_label.add_css_class("caption");
        loop_info_label.add_css_class("numeric");
        let loop_card = make_info_card("Зацикленное видео", &loop_info_label);
        loop_card.set_hexpand(true);

        let live_info_label = gtk4::Label::new(Some("LIVE · камера не выбрана"));
        live_info_label.set_xalign(0.0);
        live_info_label.add_css_class("caption");
        live_info_label.add_css_class("numeric");
        let live_card = make_info_card("Настоящая камера", &live_info_label);
        live_card.set_hexpand(true);

        let out_picture = gtk4::Picture::builder()
            .width_request(192)
            .height_request(108)
            .can_shrink(true)
            .content_fit(gtk4::ContentFit::Contain)
            .build();
        let out_card = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(4)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(8)
            .margin_end(8)
            .build();
        out_card.add_css_class("card");
        let out_title = gtk4::Label::new(Some("Выход виртуальной камеры"));
        out_title.set_xalign(0.0);
        out_title.add_css_class("caption-heading");
        out_card.append(&out_title);
        out_card.append(&out_picture);

        top_strip.append(&loop_card);
        top_strip.append(&live_card);
        top_strip.append(&out_card);
        left_box.append(&top_strip);

        // Main GPU CompareView
        let compare_view = CompareView::new();
        compare_view.set_size_request(560, 340);
        let compare_frame = gtk4::Frame::builder().child(&compare_view).build();
        compare_frame.set_vexpand(true);
        compare_frame.set_hexpand(true);
        left_box.append(&compare_frame);

        {
            let act = Rc::clone(&on_action);
            let cfg_cell = Rc::clone(&compare_cfg);
            compare_view.connect_split_changed(move |ratio| {
                let mut c = cfg_cell.get();
                c.split_pos = ratio;
                cfg_cell.set(c);
                act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
            });
        }

        // Comparison Toolbar (Row 1: modes, opacity, toggles; Row 2: zoom/pan/guides/reset)
        let toolbar_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(6)
            .build();
        toolbar_box.add_css_class("card");
        toolbar_box.set_margin_top(4);

        let row1 = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .margin_top(8)
            .margin_start(8)
            .margin_end(8)
            .build();

        let mode_overlay_btn = gtk4::ToggleButton::with_label("Наложение");
        let mode_side_btn = gtk4::ToggleButton::with_label("Рядом");
        let mode_split_btn = gtk4::ToggleButton::with_label("Разделитель");
        mode_side_btn.set_group(Some(&mode_overlay_btn));
        mode_split_btn.set_group(Some(&mode_overlay_btn));
        mode_overlay_btn.set_active(true);
        let mode_group = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        mode_group.add_css_class("linked");
        mode_group.append(&mode_overlay_btn);
        mode_group.append(&mode_side_btn);
        mode_group.append(&mode_split_btn);
        row1.append(&mode_group);

        let opacity_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1.0, 0.01);
        opacity_scale.set_value(0.5);
        opacity_scale.set_width_request(110);
        opacity_scale.set_draw_value(false);
        opacity_scale.set_tooltip_text(Some("Прозрачность наложения (0–100%)"));
        row1.append(&gtk4::Label::new(Some("Прозр.:")));
        row1.append(&opacity_scale);

        let grid_btn = gtk4::ToggleButton::with_label("Сетка 3×3");
        let center_btn = gtk4::ToggleButton::with_label("Центр");
        let guides_btn = gtk4::ToggleButton::with_label("Направл.");
        let mirror_btn = gtk4::ToggleButton::with_label("Зеркало");
        let freeze_btn = gtk4::ToggleButton::with_label("Стоп-кадр");
        for b in [&grid_btn, &center_btn, &guides_btn, &mirror_btn, &freeze_btn] {
            row1.append(b);
        }
        toolbar_box.append(&row1);

        let row2 = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();

        let zoom_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 1.0, 3.0, 0.05);
        zoom_scale.set_value(1.0);
        zoom_scale.set_width_request(85);
        zoom_scale.set_draw_value(false);

        let pan_x_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, -1.0, 1.0, 0.02);
        pan_x_scale.set_value(0.0);
        pan_x_scale.set_width_request(80);
        pan_x_scale.set_draw_value(false);

        let pan_y_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, -1.0, 1.0, 0.02);
        pan_y_scale.set_value(0.0);
        pan_y_scale.set_width_request(80);
        pan_y_scale.set_draw_value(false);

        let guide_x_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1.0, 0.01);
        guide_x_scale.set_value(0.5);
        guide_x_scale.set_width_request(75);
        guide_x_scale.set_draw_value(false);

        let guide_y_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1.0, 0.01);
        guide_y_scale.set_value(0.5);
        guide_y_scale.set_width_request(75);
        guide_y_scale.set_draw_value(false);

        let reset_btn = gtk4::Button::with_label("Сброс");
        row2.append(&gtk4::Label::new(Some("Зум:")));
        row2.append(&zoom_scale);
        row2.append(&gtk4::Label::new(Some("X:")));
        row2.append(&pan_x_scale);
        row2.append(&gtk4::Label::new(Some("Y:")));
        row2.append(&pan_y_scale);
        row2.append(&gtk4::Label::new(Some("Напр. X/Y:")));
        row2.append(&guide_x_scale);
        row2.append(&guide_y_scale);
        row2.append(&reset_btn);
        toolbar_box.append(&row2);
        left_box.append(&toolbar_box);

        // Wire compare toolbar signals
        wire_compare_toggle(&mode_overlay_btn, &syncing, &compare_cfg, &compare_view, &on_action, |c, active| {
            if active { c.mode = CompareMode::Overlay; }
        });
        wire_compare_toggle(&mode_side_btn, &syncing, &compare_cfg, &compare_view, &on_action, |c, active| {
            if active { c.mode = CompareMode::SideBySide; }
        });
        wire_compare_toggle(&mode_split_btn, &syncing, &compare_cfg, &compare_view, &on_action, |c, active| {
            if active { c.mode = CompareMode::Split; }
        });
        wire_compare_toggle(&grid_btn, &syncing, &compare_cfg, &compare_view, &on_action, |c, a| c.show_grid = a);
        wire_compare_toggle(&center_btn, &syncing, &compare_cfg, &compare_view, &on_action, |c, a| c.show_center = a);
        wire_compare_toggle(&guides_btn, &syncing, &compare_cfg, &compare_view, &on_action, |c, a| c.show_guides = a);
        wire_compare_toggle(&mirror_btn, &syncing, &compare_cfg, &compare_view, &on_action, |c, a| c.mirror = a);

        wire_compare_scale(&opacity_scale, &syncing, &compare_cfg, &compare_view, &on_action, |c, v| c.opacity = v);
        wire_compare_scale(&zoom_scale, &syncing, &compare_cfg, &compare_view, &on_action, |c, v| c.zoom = v);
        wire_compare_scale(&pan_x_scale, &syncing, &compare_cfg, &compare_view, &on_action, |c, v| c.pan_x = v);
        wire_compare_scale(&pan_y_scale, &syncing, &compare_cfg, &compare_view, &on_action, |c, v| c.pan_y = v);
        wire_compare_scale(&guide_x_scale, &syncing, &compare_cfg, &compare_view, &on_action, |c, v| c.guide_x = v);
        wire_compare_scale(&guide_y_scale, &syncing, &compare_cfg, &compare_view, &on_action, |c, v| c.guide_y = v);

        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let cv = compare_view.clone();
            freeze_btn.connect_toggled(move |b| {
                if s.get() {
                    return;
                }
                let frozen = b.is_active();
                cv.set_frozen(frozen);
                act(UiAction::Command(Command::SetFreeze { frozen }));
            });
        }
        {
            let act = Rc::clone(&on_action);
            let cfg_cell = Rc::clone(&compare_cfg);
            let cv = compare_view.clone();
            reset_btn.connect_clicked(move |_| {
                let mut c = cfg_cell.get();
                c.zoom = 1.0;
                c.pan_x = 0.0;
                c.pan_y = 0.0;
                c.mirror = false;
                c.opacity = 0.5;
                c.split_pos = 0.5;
                cfg_cell.set(c);
                cv.set_config(c);
                act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
            });
        }

        // Right Controls Panel
        let controls = ControlPanel::new(&window, Rc::clone(&on_action), Rc::clone(&syncing));

        let body = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(0)
            .vexpand(true)
            .build();
        body.append(&left_box);
        body.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));
        body.append(&controls.widget);

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content_box.append(&banner);
        content_box.append(&body);

        let footer_label = gtk4::Label::new(Some("Режим трансляции: Настоящая камера · Аудио: Микрофон"));
        footer_label.set_xalign(0.0);
        footer_label.set_margin_top(6);
        footer_label.set_margin_bottom(6);
        footer_label.set_margin_start(12);
        footer_label.set_margin_end(12);
        footer_label.add_css_class("caption");

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&content_box));
        toolbar_view.add_bottom_bar(&footer_label);
        window.set_content(Some(&toolbar_view));

        Self {
            window,
            status_pill,
            banner,
            loop_info_label,
            live_info_label,
            out_picture,
            compare_view,
            controls,
            footer_label,
            syncing,
            compare_cfg,
            mode_overlay_btn,
            mode_side_btn,
            mode_split_btn,
            opacity_scale,
            grid_btn,
            center_btn,
            guides_btn,
            guide_x_scale,
            guide_y_scale,
            mirror_btn,
            freeze_btn,
            zoom_scale,
            pan_x_scale,
            pan_y_scale,
        }
    }

    pub fn bind_paintables(
        &self,
        loop_p: Option<gdk::Paintable>,
        live_p: Option<gdk::Paintable>,
        out_p: Option<gdk::Paintable>,
    ) {
        self.compare_view.set_paintables(loop_p, live_p);
        self.out_picture.set_paintable(out_p.as_ref());
    }

    pub fn sync_state(&self, cfg: &Config, state: &AppState, devices: &DeviceInventory) {
        let prev = self.syncing.replace(true);

        self.compare_cfg.set(cfg.compare);
        self.compare_view.set_config(cfg.compare);
        if self.compare_view.is_frozen() != state.frozen_reference {
            self.compare_view.set_frozen(state.frozen_reference);
        }

        match cfg.compare.mode {
            CompareMode::Overlay => self.mode_overlay_btn.set_active(true),
            CompareMode::SideBySide => self.mode_side_btn.set_active(true),
            CompareMode::Split => self.mode_split_btn.set_active(true),
        }
        self.opacity_scale.set_value(cfg.compare.opacity);
        self.grid_btn.set_active(cfg.compare.show_grid);
        self.center_btn.set_active(cfg.compare.show_center);
        self.guides_btn.set_active(cfg.compare.show_guides);
        self.guide_x_scale.set_value(cfg.compare.guide_x);
        self.guide_y_scale.set_value(cfg.compare.guide_y);
        self.mirror_btn.set_active(cfg.compare.mirror);
        self.freeze_btn.set_active(state.frozen_reference);
        self.zoom_scale.set_value(cfg.compare.zoom);
        self.pan_x_scale.set_value(cfg.compare.pan_x);
        self.pan_y_scale.set_value(cfg.compare.pan_y);

        let mode_text = match state.mode {
            StreamMode::Loop => "Зацикленное видео",
            StreamMode::Live => "Настоящая камера",
            StreamMode::Custom { video, .. } => match video {
                VideoSource::Loop => "Custom (Видео: MP4)",
                VideoSource::Live => "Custom (Видео: Камера)",
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

        self.status_pill.set_label(&format!("● {mode_text}"));
        self.footer_label
            .set_label(&format!("Режим трансляции: {mode_text} · Аудио: {audio_text}"));

        let cam_label = cfg.real_camera.as_deref().unwrap_or("нет камеры");
        self.live_info_label.set_label(&format!(
            "LIVE · {} · {}×{} @ {} FPS",
            cam_label, cfg.video.width, cfg.video.height, cfg.video.fps
        ));

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

        self.controls.sync_state(cfg, state, devices);
        self.syncing.set(prev);
    }

    pub fn update_levels(&self, levels: &AudioLevels) {
        self.controls.update_levels(levels);
    }

    pub fn update_timecode(&self, file_name: &str, pos_ms: u64, dur_ms: Option<u64>, fps: u32) {
        let pos_s = pos_ms / 1000;
        let dur_str = dur_ms
            .map(|d| {
                let s = d / 1000;
                format!("{:02}:{:02}", s / 60, s % 60)
            })
            .unwrap_or_else(|| "--:--".into());
        self.loop_info_label.set_label(&format!(
            "{} · {:02}:{:02} / {} · {} FPS",
            file_name,
            pos_s / 60,
            pos_s % 60,
            dur_str,
            fps
        ));
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }
}

fn make_info_card(title: &str, detail: &gtk4::Label) -> gtk4::Box {
    let card = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(4)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(8)
        .margin_end(8)
        .valign(gtk4::Align::Center)
        .build();
    card.add_css_class("card");
    let heading = gtk4::Label::new(Some(title));
    heading.set_xalign(0.0);
    heading.add_css_class("caption-heading");
    card.append(&heading);
    card.append(detail);
    card
}

fn wire_compare_toggle(
    btn: &gtk4::ToggleButton,
    syncing: &Rc<Cell<bool>>,
    cfg_cell: &Rc<Cell<CompareConfig>>,
    cv: &CompareView,
    on_action: &Rc<dyn Fn(UiAction)>,
    apply: impl Fn(&mut CompareConfig, bool) + 'static,
) {
    let s = Rc::clone(syncing);
    let cell = Rc::clone(cfg_cell);
    let view = cv.clone();
    let act = Rc::clone(on_action);
    btn.connect_toggled(move |b| {
        if s.get() {
            return;
        }
        let mut c = cell.get();
        apply(&mut c, b.is_active());
        cell.set(c);
        view.set_config(c);
        act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
    });
}

fn wire_compare_scale(
    scale: &gtk4::Scale,
    syncing: &Rc<Cell<bool>>,
    cfg_cell: &Rc<Cell<CompareConfig>>,
    cv: &CompareView,
    on_action: &Rc<dyn Fn(UiAction)>,
    apply: impl Fn(&mut CompareConfig, f64) + 'static,
) {
    let s = Rc::clone(syncing);
    let cell = Rc::clone(cfg_cell);
    let view = cv.clone();
    let act = Rc::clone(on_action);
    scale.connect_value_changed(move |sc| {
        if s.get() {
            return;
        }
        let mut c = cell.get();
        apply(&mut c, sc.value());
        cell.set(c);
        view.set_config(c);
        act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
    });
}
