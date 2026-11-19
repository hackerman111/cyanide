use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::config::{Config, LoopReturnPolicy, StartupMode, VideoFormat};
use crate::devices::DeviceInventory;
use crate::gui::{ConfigPatch, UiAction};
use crate::state::{
    AppState, AudioLevels, AudioSource, Command, MicAction, StreamMode, VideoSource,
};

const RESOLUTIONS: &[(u32, u32, &str)] = &[
    (640, 480, "640×480"),
    (1280, 720, "1280×720"),
    (1920, 1080, "1920×1080"),
];
const FPS_OPTIONS: &[u32] = &[15, 24, 30, 60];

pub struct ControlPanel {
    pub widget: gtk4::ScrolledWindow,
    syncing: Rc<Cell<bool>>,
    video_loop_btn: gtk4::ToggleButton,
    video_live_btn: gtk4::ToggleButton,
    audio_combo: adw::ComboRow,
    mic_switch: adw::SwitchRow,
    startup_combo: adw::ComboRow,
    loop_file_row: adw::ActionRow,
    real_cam_combo: adw::ComboRow,
    virt_cam_combo: adw::ComboRow,
    real_mic_combo: adw::ComboRow,
    res_combo: adw::ComboRow,
    fps_combo: adw::ComboRow,
    loop_vol_scale: gtk4::Scale,
    live_vol_scale: gtk4::Scale,
    loop_meter: gtk4::LevelBar,
    live_meter: gtk4::LevelBar,
    out_meter: gtk4::LevelBar,
    av_delay_scale: gtk4::Scale,
    av_delay_label: gtk4::Label,
    crossfade_scale: gtk4::Scale,
    crossfade_label: gtk4::Label,
    return_combo: adw::ComboRow,
    bookmark_label: gtk4::Label,
    real_cam_paths: Rc<RefCell<Vec<String>>>,
    virt_cam_paths: Rc<RefCell<Vec<String>>>,
    real_mic_names: Rc<RefCell<Vec<String>>>,
}

impl ControlPanel {
    pub fn new(
        window: &adw::ApplicationWindow,
        on_action: Rc<dyn Fn(UiAction)>,
        syncing: Rc<Cell<bool>>,
    ) -> Self {
        let content = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();

        // --- Group 1: Virtual Devices & Stream Routing ---
        let routing_group = adw::PreferencesGroup::builder()
            .title("Виртуальные устройства")
            .build();

        let preset_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        preset_box.add_css_class("linked");
        let preset_loop = gtk4::Button::with_label("Режим Loop");
        let preset_live = gtk4::Button::with_label("Режим Live");
        {
            let act = Rc::clone(&on_action);
            preset_loop.connect_clicked(move |_| {
                act(UiAction::Command(Command::SetMode(StreamMode::Loop)))
            });
            let act = Rc::clone(&on_action);
            preset_live.connect_clicked(move |_| {
                act(UiAction::Command(Command::SetMode(StreamMode::Live)))
            });
        }
        preset_box.append(&preset_loop);
        preset_box.append(&preset_live);
        let preset_row = adw::ActionRow::builder()
            .title("Быстрое переключение")
            .build();
        preset_row.add_suffix(&preset_box);
        routing_group.add(&preset_row);

        let video_loop_btn = gtk4::ToggleButton::with_label("Зацикленное");
        let video_live_btn = gtk4::ToggleButton::with_label("Камера");
        video_live_btn.set_group(Some(&video_loop_btn));
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            video_loop_btn.connect_toggled(move |b| {
                if !s.get() && b.is_active() {
                    act(UiAction::Command(Command::SetVideo(VideoSource::Loop)));
                }
            });
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            video_live_btn.connect_toggled(move |b| {
                if !s.get() && b.is_active() {
                    act(UiAction::Command(Command::SetVideo(VideoSource::Live)));
                }
            });
        }
        let video_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        video_box.add_css_class("linked");
        video_box.append(&video_loop_btn);
        video_box.append(&video_live_btn);
        let video_row = adw::ActionRow::builder().title("Видеопоток").build();
        video_row.add_suffix(&video_box);
        routing_group.add(&video_row);

        let audio_combo = adw::ComboRow::builder()
            .title("Источник аудио")
            .model(&gtk4::StringList::new(&[
                "Файл MP4",
                "Настоящий микрофон",
                "Смесь (MP4 + микрофон)",
                "Тишина",
            ]))
            .build();
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            audio_combo.connect_selected_notify(move |c| {
                if s.get() {
                    return;
                }
                let src = match c.selected() {
                    0 => AudioSource::File,
                    1 => AudioSource::Mic,
                    2 => AudioSource::Mix,
                    _ => AudioSource::Silence,
                };
                act(UiAction::Command(Command::SetAudio(src)));
            });
        }
        routing_group.add(&audio_combo);

        let mic_switch = adw::SwitchRow::builder()
            .title("Виртуальный микрофон")
            .subtitle("Плавный mute без отключения VirtualCamMic")
            .active(true)
            .build();
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            mic_switch.connect_active_notify(move |sw| {
                if s.get() {
                    return;
                }
                let action = if sw.is_active() {
                    MicAction::On
                } else {
                    MicAction::Off
                };
                act(UiAction::Command(Command::SetMic(action)));
            });
        }
        routing_group.add(&mic_switch);

        let startup_combo = adw::ComboRow::builder()
            .title("Режим при старте")
            .model(&gtk4::StringList::new(&[
                "Настоящая камера",
                "Зацикленное видео",
            ]))
            .build();
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            startup_combo.connect_selected_notify(move |c| {
                if s.get() {
                    return;
                }
                let mode = if c.selected() == 0 {
                    StartupMode::Live
                } else {
                    StartupMode::Loop
                };
                act(UiAction::UpdateConfig(ConfigPatch::StartupMode(mode)));
            });
        }
        routing_group.add(&startup_combo);
        content.append(&routing_group);

        // --- Group 2: Sources & Format ---
        let src_group = adw::PreferencesGroup::builder()
            .title("Источники и формат")
            .build();

        let refresh_btn = gtk4::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some("Обновить список устройств"));
        {
            let act = Rc::clone(&on_action);
            refresh_btn.connect_clicked(move |_| act(UiAction::RefreshDevices));
        }
        src_group.set_header_suffix(Some(&refresh_btn));

        let loop_file_row = adw::ActionRow::builder()
            .title("Файл MP4")
            .subtitle("Не выбран")
            .build();
        let pick_btn = gtk4::Button::with_label("Выбрать");
        {
            let win = window.clone();
            let act = Rc::clone(&on_action);
            pick_btn.connect_clicked(move |_| {
                let filter = gtk4::FileFilter::new();
                filter.set_name(Some("Видео (MP4, MKV, WebM, MOV)"));
                for pat in ["*.mp4", "*.mkv", "*.webm", "*.mov", "*.MP4"] {
                    filter.add_pattern(pat);
                }
                let filters = gtk4::gio::ListStore::new::<gtk4::FileFilter>();
                filters.append(&filter);
                let dialog = gtk4::FileDialog::builder()
                    .title("Выберите видео для зацикливания")
                    .filters(&filters)
                    .build();
                let act_cb = Rc::clone(&act);
                dialog.open(Some(&win), gtk4::gio::Cancellable::NONE, move |res| {
                    if let Ok(file) = res
                        && let Some(path) = file.path()
                    {
                        act_cb(UiAction::PickLoopFile(path));
                    }
                });
            });
        }
        loop_file_row.add_suffix(&pick_btn);
        src_group.add(&loop_file_row);

        let real_cam_paths = Rc::new(RefCell::new(Vec::<String>::new()));
        let virt_cam_paths = Rc::new(RefCell::new(Vec::<String>::new()));
        let real_mic_names = Rc::new(RefCell::new(Vec::<String>::new()));

        let real_cam_combo = adw::ComboRow::builder().title("Реальная камера").build();
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let paths = Rc::clone(&real_cam_paths);
            real_cam_combo.connect_selected_notify(move |c| {
                if s.get() {
                    return;
                }
                if let Some(p) = paths.borrow().get(c.selected() as usize) {
                    act(UiAction::UpdateConfig(ConfigPatch::RealCamera(Some(
                        p.clone(),
                    ))));
                }
            });
        }
        src_group.add(&real_cam_combo);

        let virt_cam_combo = adw::ComboRow::builder().title("Виртуальная камера").build();
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let paths = Rc::clone(&virt_cam_paths);
            virt_cam_combo.connect_selected_notify(move |c| {
                if s.get() {
                    return;
                }
                if let Some(p) = paths.borrow().get(c.selected() as usize) {
                    act(UiAction::UpdateConfig(ConfigPatch::VirtualCamera(Some(
                        p.clone(),
                    ))));
                }
            });
        }
        src_group.add(&virt_cam_combo);

        let real_mic_combo = adw::ComboRow::builder().title("Реальный микрофон").build();
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let names = Rc::clone(&real_mic_names);
            real_mic_combo.connect_selected_notify(move |c| {
                if s.get() {
                    return;
                }
                if let Some(n) = names.borrow().get(c.selected() as usize) {
                    act(UiAction::UpdateConfig(ConfigPatch::RealMic(Some(
                        n.clone(),
                    ))));
                }
            });
        }
        src_group.add(&real_mic_combo);

        let res_labels: Vec<&str> = RESOLUTIONS.iter().map(|r| r.2).collect();
        let res_combo = adw::ComboRow::builder()
            .title("Разрешение")
            .model(&gtk4::StringList::new(&res_labels))
            .build();
        let fps_labels: Vec<String> = FPS_OPTIONS.iter().map(|f| format!("{f} FPS")).collect();
        let fps_refs: Vec<&str> = fps_labels.iter().map(String::as_str).collect();
        let fps_combo = adw::ComboRow::builder()
            .title("Частота кадров")
            .model(&gtk4::StringList::new(&fps_refs))
            .build();
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let fps_c = fps_combo.clone();
            res_combo.connect_selected_notify(move |c| {
                if s.get() {
                    return;
                }
                if let Some(&(w, h, _)) = RESOLUTIONS.get(c.selected() as usize) {
                    let fps = FPS_OPTIONS
                        .get(fps_c.selected() as usize)
                        .copied()
                        .unwrap_or(30);
                    act(UiAction::UpdateConfig(ConfigPatch::VideoFormat(
                        VideoFormat {
                            width: w,
                            height: h,
                            fps,
                        },
                    )));
                }
            });
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let res_c = res_combo.clone();
            fps_combo.connect_selected_notify(move |c| {
                if s.get() {
                    return;
                }
                if let Some(&fps) = FPS_OPTIONS.get(c.selected() as usize) {
                    let &(w, h, _) = RESOLUTIONS
                        .get(res_c.selected() as usize)
                        .unwrap_or(&RESOLUTIONS[1]);
                    act(UiAction::UpdateConfig(ConfigPatch::VideoFormat(
                        VideoFormat {
                            width: w,
                            height: h,
                            fps,
                        },
                    )));
                }
            });
        }
        src_group.add(&res_combo);
        src_group.add(&fps_combo);
        content.append(&src_group);

        // --- Group 3: Audio, Meters & A/V Sync ---
        let audio_group = adw::PreferencesGroup::builder()
            .title("Аудио и синхронизация")
            .build();

        let (loop_vol_row, loop_vol_scale) = make_slider_row("Громкость MP4", 0.0, 1.5, 0.05, 1.0);
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            loop_vol_scale.connect_value_changed(move |sc| {
                if !s.get() {
                    act(UiAction::UpdateConfig(ConfigPatch::LoopVolume(sc.value())));
                }
            });
        }
        audio_group.add(&loop_vol_row);

        let (live_vol_row, live_vol_scale) =
            make_slider_row("Громкость микрофона", 0.0, 1.5, 0.05, 1.0);
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            live_vol_scale.connect_value_changed(move |sc| {
                if !s.get() {
                    act(UiAction::UpdateConfig(ConfigPatch::LiveVolume(sc.value())));
                }
            });
        }
        audio_group.add(&live_vol_row);

        let (loop_meter_row, loop_meter) = make_meter_row("Уровень MP4");
        let (live_meter_row, live_meter) = make_meter_row("Уровень микрофона");
        let (out_meter_row, out_meter) = make_meter_row("Выход VirtualCamMic");
        audio_group.add(&loop_meter_row);
        audio_group.add(&live_meter_row);
        audio_group.add(&out_meter_row);

        let (av_row, av_delay_scale, av_delay_label) =
            make_labeled_slider_row("Задержка A/V", -500.0, 500.0, 10.0, 0.0, "0 мс");
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let lbl = av_delay_label.clone();
            av_delay_scale.connect_value_changed(move |sc| {
                let ms = sc.value().round() as i32;
                lbl.set_label(&format!("{ms:+} мс"));
                if !s.get() {
                    act(UiAction::UpdateConfig(ConfigPatch::AvDelayMs(ms)));
                }
            });
        }
        audio_group.add(&av_row);

        let (cf_row, crossfade_scale, crossfade_label) =
            make_labeled_slider_row("Кроссфейд", 10.0, 30.0, 5.0, 20.0, "20 мс");
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let lbl = crossfade_label.clone();
            crossfade_scale.connect_value_changed(move |sc| {
                let ms = sc.value().round() as u32;
                lbl.set_label(&format!("{ms} мс"));
                if !s.get() {
                    act(UiAction::UpdateConfig(ConfigPatch::CrossfadeMs(ms)));
                }
            });
        }
        audio_group.add(&cf_row);
        content.append(&audio_group);

        // --- Group 4: Loop Return Point ---
        let loop_group = adw::PreferencesGroup::builder()
            .title("Возврат к зацикленному видео")
            .build();
        let return_combo = adw::ComboRow::builder()
            .title("При возврате на MP4")
            .model(&gtk4::StringList::new(&[
                "Продолжать с текущей позиции",
                "Начинать с 00:00",
                "Переходить к сохранённой метке",
            ]))
            .build();
        let bookmark_label = gtk4::Label::new(Some("Метка: 00:00"));
        bookmark_label.add_css_class("dim-label");
        bookmark_label.add_css_class("numeric");
        let set_bm_btn = gtk4::Button::with_label("Запомнить текущий кадр");
        {
            let act = Rc::clone(&on_action);
            set_bm_btn.connect_clicked(move |_| act(UiAction::SetReturnBookmarkCurrent));
        }
        let bm_row = adw::ActionRow::builder().title("Точка возврата").build();
        bm_row.add_suffix(&bookmark_label);
        bm_row.add_suffix(&set_bm_btn);
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            return_combo.connect_selected_notify(move |c| {
                if s.get() {
                    return;
                }
                let policy = match c.selected() {
                    0 => LoopReturnPolicy::Continue,
                    1 => LoopReturnPolicy::Restart,
                    _ => LoopReturnPolicy::Bookmark { position_ms: 0 },
                };
                act(UiAction::UpdateConfig(ConfigPatch::LoopReturn(policy)));
            });
        }
        loop_group.add(&return_combo);
        loop_group.add(&bm_row);
        content.append(&loop_group);

        let widget = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .min_content_width(370)
            .child(&content)
            .build();

        Self {
            widget,
            syncing,
            video_loop_btn,
            video_live_btn,
            audio_combo,
            mic_switch,
            startup_combo,
            loop_file_row,
            real_cam_combo,
            virt_cam_combo,
            real_mic_combo,
            res_combo,
            fps_combo,
            loop_vol_scale,
            live_vol_scale,
            loop_meter,
            live_meter,
            out_meter,
            av_delay_scale,
            av_delay_label,
            crossfade_scale,
            crossfade_label,
            return_combo,
            bookmark_label,
            real_cam_paths,
            virt_cam_paths,
            real_mic_names,
        }
    }

    pub fn sync_state(&self, cfg: &Config, state: &AppState, devices: &DeviceInventory) {
        let prev = self.syncing.replace(true);

        match state.mode.video() {
            VideoSource::Loop => self.video_loop_btn.set_active(true),
            VideoSource::Live => self.video_live_btn.set_active(true),
        }
        let audio_idx = match state.mode.audio() {
            AudioSource::File => 0,
            AudioSource::Mic => 1,
            AudioSource::Mix => 2,
            AudioSource::Silence => 3,
        };
        self.audio_combo.set_selected(audio_idx);
        self.mic_switch.set_active(state.virtual_mic_enabled);
        self.startup_combo.set_selected(match cfg.startup_mode {
            StartupMode::Live => 0,
            StartupMode::Loop => 1,
        });

        let file_label = cfg
            .loop_file
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Не выбран".into());
        self.loop_file_row.set_subtitle(&file_label);

        sync_combo(
            &self.real_cam_combo,
            &self.real_cam_paths,
            devices
                .real_cameras
                .iter()
                .map(|d| (d.path.clone(), format!("{} ({})", d.name, d.path)))
                .collect(),
            cfg.real_camera.as_deref(),
            "Нет камер",
        );
        sync_combo(
            &self.virt_cam_combo,
            &self.virt_cam_paths,
            devices
                .loopback_cameras
                .iter()
                .map(|d| (d.path.clone(), format!("{} ({})", d.name, d.path)))
                .collect(),
            cfg.virtual_camera.as_deref(),
            "Нет v4l2loopback",
        );
        sync_combo(
            &self.real_mic_combo,
            &self.real_mic_names,
            devices
                .real_mics
                .iter()
                .map(|d| (d.name.clone(), d.label.clone()))
                .collect(),
            cfg.real_mic.as_deref(),
            "Системный по умолчанию",
        );

        let res_idx = RESOLUTIONS
            .iter()
            .position(|&(w, h, _)| w == cfg.video.width && h == cfg.video.height)
            .unwrap_or(1) as u32;
        self.res_combo.set_selected(res_idx);
        let fps_idx = FPS_OPTIONS
            .iter()
            .position(|&f| f == cfg.video.fps)
            .unwrap_or(2) as u32;
        self.fps_combo.set_selected(fps_idx);

        self.loop_vol_scale.set_value(cfg.loop_volume);
        self.live_vol_scale.set_value(cfg.live_volume);
        self.av_delay_scale.set_value(cfg.av_delay_ms as f64);
        self.av_delay_label
            .set_label(&format!("{:+} мс", cfg.av_delay_ms));
        self.crossfade_scale.set_value(cfg.crossfade_ms as f64);
        self.crossfade_label
            .set_label(&format!("{} мс", cfg.crossfade_ms));

        let (ret_idx, bm_ms) = match cfg.loop_return {
            LoopReturnPolicy::Continue => (0, 0),
            LoopReturnPolicy::Restart => (1, 0),
            LoopReturnPolicy::Bookmark { position_ms } => (2, position_ms),
        };
        self.return_combo.set_selected(ret_idx);
        let secs = bm_ms / 1000;
        self.bookmark_label
            .set_label(&format!("Метка: {:02}:{:02}", secs / 60, secs % 60));

        self.syncing.set(prev);
    }

    pub fn update_levels(&self, levels: &AudioLevels) {
        self.loop_meter.set_value(db_to_unit(levels.loop_db));
        self.live_meter.set_value(db_to_unit(levels.live_db));
        self.out_meter.set_value(db_to_unit(levels.out_db));
    }
}

fn db_to_unit(db: f64) -> f64 {
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn sync_combo(
    combo: &adw::ComboRow,
    backing: &RefCell<Vec<String>>,
    items: Vec<(String, String)>,
    selected: Option<&str>,
    empty_label: &str,
) {
    let mut keys = Vec::with_capacity(items.len());
    let mut labels = Vec::with_capacity(items.len().max(1));
    for (k, l) in items {
        keys.push(k);
        labels.push(l);
    }
    if labels.is_empty() {
        labels.push(empty_label.to_string());
    }
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    combo.set_model(Some(&gtk4::StringList::new(&refs)));
    let idx = selected
        .and_then(|sel| keys.iter().position(|k| k == sel))
        .unwrap_or(0) as u32;
    *backing.borrow_mut() = keys;
    combo.set_selected(idx);
}

fn make_slider_row(
    title: &str,
    min: f64,
    max: f64,
    step: f64,
    initial: f64,
) -> (adw::ActionRow, gtk4::Scale) {
    let scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, min, max, step);
    scale.set_value(initial);
    scale.set_width_request(150);
    scale.set_draw_value(false);
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&scale);
    (row, scale)
}

fn make_labeled_slider_row(
    title: &str,
    min: f64,
    max: f64,
    step: f64,
    initial: f64,
    initial_text: &str,
) -> (adw::ActionRow, gtk4::Scale, gtk4::Label) {
    let scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, min, max, step);
    scale.set_value(initial);
    scale.set_width_request(130);
    scale.set_draw_value(false);
    let label = gtk4::Label::new(Some(initial_text));
    label.set_width_chars(7);
    label.add_css_class("numeric");
    label.add_css_class("dim-label");
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&label);
    row.add_suffix(&scale);
    (row, scale, label)
}

fn make_meter_row(title: &str) -> (adw::ActionRow, gtk4::LevelBar) {
    let bar = gtk4::LevelBar::builder()
        .min_value(0.0)
        .max_value(1.0)
        .width_request(150)
        .valign(gtk4::Align::Center)
        .build();
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&bar);
    (row, bar)
}
