use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::config::{Config, LagConfig, LoopReturnPolicy, StartupMode, VideoFormat};
use crate::devices::DeviceInventory;
use crate::gui::{ConfigPatch, UiAction};
use crate::hotkey::Hotkey;
use crate::state::{
    AppState, AudioLevels, AudioSource, Command, MicAction, StreamMode, VideoSource,
};

const RESOLUTIONS: &[(u32, u32, &str)] = &[
    (640, 480, "640×480"),
    (1280, 720, "1280×720"),
    (1920, 1080, "1920×1080"),
];
const FPS_OPTIONS: &[u32] = &[15, 24, 30, 60];

#[derive(Default)]
struct ComboBacking {
    keys: Vec<Option<String>>,
    labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MicComboEntry {
    Disabled,
    Device(String),
    MissingPlaceholder,
}

#[derive(Default)]
struct MicComboBacking {
    entries: Vec<MicComboEntry>,
    labels: Vec<String>,
}

pub struct ControlPanel {
    pub widget: gtk4::Box,
    syncing: Rc<Cell<bool>>,
    out_picture: gtk4::Picture,
    monitor_screen: gtk4::Box,
    device_badge: gtk4::Label,
    source_badge: gtk4::Label,
    rec_badge: gtk4::Label,
    monitor_status_label: gtk4::Label,
    source_loop_btn: gtk4::ToggleButton,
    source_live_btn: gtk4::ToggleButton,
    quick_mic_btn: gtk4::ToggleButton,
    audio_combo: adw::ComboRow,
    startup_combo: adw::ComboRow,
    hotkey_row: adw::ActionRow,
    hotkey_entry: gtk4::Entry,
    loop_file_row: adw::ActionRow,
    real_cam_combo: adw::ComboRow,
    virt_cam_combo: adw::ComboRow,
    real_mic_combo: adw::ComboRow,
    res_combo: adw::ComboRow,
    fps_combo: adw::ComboRow,
    loop_vol_scale: gtk4::Scale,
    loop_vol_label: gtk4::Label,
    live_vol_scale: gtk4::Scale,
    live_vol_label: gtk4::Label,
    loop_meter: gtk4::LevelBar,
    loop_db_label: gtk4::Label,
    live_meter: gtk4::LevelBar,
    live_db_label: gtk4::Label,
    out_meter: gtk4::LevelBar,
    out_db_label: gtk4::Label,
    av_delay_scale: gtk4::Scale,
    av_delay_label: gtk4::Label,
    crossfade_scale: gtk4::Scale,
    crossfade_label: gtk4::Label,
    return_combo: adw::ComboRow,
    bookmark_label: gtk4::Label,
    ping_pong_switch: adw::SwitchRow,
    lag_switch: adw::SwitchRow,
    lag_on_switch_row: adw::SwitchRow,
    lag_int_row: adw::ActionRow,
    lag_interval_scale: gtk4::Scale,
    lag_interval_label: gtk4::Label,
    lag_min_row: adw::ActionRow,
    lag_min_scale: gtk4::Scale,
    lag_min_label: gtk4::Label,
    lag_max_row: adw::ActionRow,
    lag_max_scale: gtk4::Scale,
    lag_max_label: gtk4::Label,
    lag_cfg: Rc<Cell<LagConfig>>,
    real_cam_paths: Rc<RefCell<ComboBacking>>,
    virt_cam_paths: Rc<RefCell<ComboBacking>>,
    real_mic_backing: Rc<RefCell<MicComboBacking>>,
}

impl ControlPanel {
    pub fn new(
        window: &adw::ApplicationWindow,
        on_action: Rc<dyn Fn(UiAction)>,
        syncing: Rc<Cell<bool>>,
    ) -> Self {
        let widget = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(8)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(12)
            .vexpand(true)
            .build();
        widget.set_size_request(360, -1);

        // === Fixed Top: Virtual Camera Program Monitor ===
        let monitor_card = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(8)
            .build();
        monitor_card.add_css_class("cyanide-monitor-card");

        let mon_header = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let mon_title = gtk4::Label::new(Some("Выход камеры"));
        mon_title.set_xalign(0.0);
        mon_title.set_hexpand(true);
        mon_title.add_css_class("cyanide-section-title");

        let device_badge = gtk4::Label::new(Some("VirtualCam"));
        device_badge.add_css_class("cyanide-status-pill");
        device_badge.add_css_class("live");

        let source_badge = gtk4::Label::new(Some("Камера"));
        source_badge.add_css_class("cyanide-status-pill");
        source_badge.add_css_class("live");

        let rec_badge = gtk4::Label::new(Some("● ЗАПИСЬ"));
        rec_badge.add_css_class("cyanide-status-pill");
        rec_badge.add_css_class("recording");
        rec_badge.set_visible(false);

        mon_header.append(&mon_title);
        mon_header.append(&device_badge);
        mon_header.append(&source_badge);
        mon_header.append(&rec_badge);
        monitor_card.append(&mon_header);

        let out_picture = gtk4::Picture::builder()
            .height_request(152)
            .can_shrink(true)
            .content_fit(gtk4::ContentFit::Contain)
            .hexpand(true)
            .build();
        let monitor_screen = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        monitor_screen.add_css_class("cyanide-monitor-screen");
        monitor_screen.add_css_class("live");
        monitor_screen.set_overflow(gtk4::Overflow::Hidden);
        monitor_screen.append(&out_picture);
        monitor_card.append(&monitor_screen);

        let monitor_status_label = gtk4::Label::new(Some("Видео: Камера · Звук: Микрофон"));
        monitor_status_label.set_xalign(0.0);
        monitor_status_label.set_wrap(true);
        monitor_status_label.add_css_class("cyanide-caption");
        monitor_card.append(&monitor_status_label);

        let quick_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let source_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        source_box.add_css_class("linked");
        source_box.set_hexpand(true);
        source_box.set_homogeneous(true);

        let source_loop_btn = gtk4::ToggleButton::with_label("Видео (MP4)");
        source_loop_btn.add_css_class("cyanide-source-btn");
        source_loop_btn.add_css_class("loop");
        source_loop_btn.set_tooltip_text(Some("Подать в виртуальную камеру видеофайл и его звук"));

        let source_live_btn = gtk4::ToggleButton::with_label("Камера");
        source_live_btn.add_css_class("cyanide-source-btn");
        source_live_btn.add_css_class("live");
        source_live_btn.set_tooltip_text(Some(
            "Подать в виртуальную камеру реальную камеру и микрофон",
        ));
        source_live_btn.set_group(Some(&source_loop_btn));
        source_live_btn.set_active(true);

        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            source_loop_btn.connect_toggled(move |b| {
                if !s.get() && b.is_active() {
                    act(UiAction::Command(Command::SetMode(StreamMode::Loop)));
                }
            });
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            source_live_btn.connect_toggled(move |b| {
                if !s.get() && b.is_active() {
                    act(UiAction::Command(Command::SetMode(StreamMode::Live)));
                }
            });
        }
        source_box.append(&source_loop_btn);
        source_box.append(&source_live_btn);

        let quick_mic_btn = gtk4::ToggleButton::new();
        let mic_btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        mic_btn_box.append(&gtk4::Image::from_icon_name(
            "audio-input-microphone-symbolic",
        ));
        mic_btn_box.append(&gtk4::Label::new(Some("Микр.")));
        quick_mic_btn.set_child(Some(&mic_btn_box));
        quick_mic_btn.set_tooltip_text(Some(
            "Включить/выключить виртуальный микрофон (плавный mute)",
        ));
        quick_mic_btn.set_active(true);
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            quick_mic_btn.connect_toggled(move |b| {
                if s.get() {
                    return;
                }
                let action = if b.is_active() {
                    MicAction::On
                } else {
                    MicAction::Off
                };
                act(UiAction::Command(Command::SetMic(action)));
            });
        }
        quick_row.append(&source_box);
        quick_row.append(&quick_mic_btn);
        monitor_card.append(&quick_row);
        widget.append(&monitor_card);

        // === 4-Tab Studio Switcher ===
        let stack = gtk4::Stack::builder()
            .transition_type(gtk4::StackTransitionType::Crossfade)
            .transition_duration(120)
            .vexpand(true)
            .build();

        let tab_strip = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(4)
            .homogeneous(true)
            .build();
        tab_strip.add_css_class("cyanide-tab-strip");

        let tab_stream = make_tab_button("Эфир", None);
        let tab_devices = make_tab_button("Устройства", Some(&tab_stream));
        let tab_audio = make_tab_button("Аудио", Some(&tab_stream));
        let tab_effects = make_tab_button("Петля", Some(&tab_stream));
        tab_stream.set_active(true);

        for (btn, name) in [
            (&tab_stream, "stream"),
            (&tab_devices, "devices"),
            (&tab_audio, "audio"),
            (&tab_effects, "effects"),
        ] {
            let st = stack.clone();
            btn.connect_toggled(move |b| {
                if b.is_active() {
                    st.set_visible_child_name(name);
                }
            });
            tab_strip.append(btn);
        }
        widget.append(&tab_strip);

        // === Page 1: Эфир (Loop File, Hotkey, Startup Mode) ===
        let stream_box = make_tab_page_box();

        let media_group = adw::PreferencesGroup::builder()
            .title("Видео для петли")
            .build();

        let loop_file_row = adw::ActionRow::builder()
            .title("Файл MP4")
            .subtitle("Не выбран")
            .build();
        let pick_btn = gtk4::Button::with_label("Выбрать");
        pick_btn.set_valign(gtk4::Align::Center);
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
        media_group.add(&loop_file_row);
        stream_box.append(&media_group);

        let control_group = adw::PreferencesGroup::builder().title("Управление").build();

        let hotkey_row = adw::ActionRow::builder()
            .title("Системный хоткей")
            .subtitle("Переключение Видео ↔ Камера")
            .build();
        let hotkey_entry = gtk4::Entry::builder()
            .placeholder_text("SUPER+F8")
            .width_chars(8)
            .valign(gtk4::Align::Center)
            .build();
        let hk_record_btn = gtk4::ToggleButton::with_label("Задать");
        hk_record_btn.set_valign(gtk4::Align::Center);
        let clear_hk_btn = gtk4::Button::from_icon_name("edit-clear-symbolic");
        clear_hk_btn.set_valign(gtk4::Align::Center);
        clear_hk_btn.add_css_class("flat");
        clear_hk_btn.set_tooltip_text(Some("Убрать системный хоткей"));

        let hk_capturing = Rc::new(Cell::new(false));
        {
            let act = Rc::clone(&on_action);
            let row_ref = hotkey_row.clone();
            hotkey_entry.connect_activate(move |entry| {
                let raw = entry.text();
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    act(UiAction::UpdateConfig(ConfigPatch::Hotkey(None)));
                    return;
                }
                match Hotkey::parse(trimmed) {
                    Ok(hk) => {
                        let canon = hk.canonical();
                        entry.set_text(&canon);
                        act(UiAction::UpdateConfig(ConfigPatch::Hotkey(Some(canon))));
                    }
                    Err(err) => {
                        row_ref.set_subtitle(&err);
                    }
                }
            });
        }
        {
            let rec = Rc::clone(&hk_capturing);
            let act = Rc::clone(&on_action);
            hk_record_btn.connect_toggled(move |b| {
                let active = b.is_active();
                rec.set(active);
                b.set_label(if active {
                    "Нажмите…"
                } else {
                    "Задать"
                });
                act(UiAction::SetHotkeyCapture(active));
            });
        }
        {
            let act = Rc::clone(&on_action);
            let entry = hotkey_entry.clone();
            let rec_btn = hk_record_btn.clone();
            clear_hk_btn.connect_clicked(move |_| {
                if rec_btn.is_active() {
                    rec_btn.set_active(false);
                }
                entry.set_text("");
                act(UiAction::UpdateConfig(ConfigPatch::Hotkey(None)));
            });
        }

        let key_ctrl = gtk4::EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
        {
            let rec = Rc::clone(&hk_capturing);
            let rec_btn = hk_record_btn.clone();
            let entry = hotkey_entry.clone();
            let act = Rc::clone(&on_action);
            key_ctrl.connect_key_pressed(move |_, key, _, state| {
                if !rec.get() {
                    return glib::Propagation::Proceed;
                }
                if key == gdk::Key::Escape {
                    rec_btn.set_active(false);
                    return glib::Propagation::Stop;
                }
                if let Some(hk) = Hotkey::from_gdk(key, state) {
                    let canon = hk.canonical();
                    rec.set(false);
                    entry.set_text(&canon);
                    act(UiAction::UpdateConfig(ConfigPatch::Hotkey(Some(canon))));
                    rec_btn.set_active(false);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Stop
            });
        }
        window.add_controller(key_ctrl);

        let hk_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        hk_box.set_valign(gtk4::Align::Center);
        hk_box.append(&hotkey_entry);
        hk_box.append(&hk_record_btn);
        hk_box.append(&clear_hk_btn);
        hotkey_row.add_suffix(&hk_box);
        control_group.add(&hotkey_row);

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
        control_group.add(&startup_combo);
        stream_box.append(&control_group);

        stack.add_named(&wrap_scroll(&stream_box), Some("stream"));

        // === Page 2: Устройства (Cameras, Mic, Format) ===
        let devices_box = make_tab_page_box();

        let hw_group = adw::PreferencesGroup::builder()
            .title("Оборудование")
            .build();
        let refresh_btn = gtk4::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.add_css_class("flat");
        refresh_btn.set_tooltip_text(Some("Обновить список устройств"));
        {
            let act = Rc::clone(&on_action);
            refresh_btn.connect_clicked(move |_| act(UiAction::RefreshDevices));
        }
        hw_group.set_header_suffix(Some(&refresh_btn));

        let real_cam_paths = Rc::new(RefCell::new(ComboBacking::default()));
        let virt_cam_paths = Rc::new(RefCell::new(ComboBacking::default()));
        let real_mic_backing = Rc::new(RefCell::new(MicComboBacking::default()));

        let real_cam_combo = adw::ComboRow::builder().title("Реальная камера").build();
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let paths = Rc::clone(&real_cam_paths);
            real_cam_combo.connect_selected_notify(move |c| {
                if s.get() {
                    return;
                }
                if let Some(Some(p)) = paths.borrow().keys.get(c.selected() as usize) {
                    act(UiAction::UpdateConfig(ConfigPatch::RealCamera(Some(
                        p.clone(),
                    ))));
                }
            });
        }
        hw_group.add(&real_cam_combo);

        let virt_cam_combo = adw::ComboRow::builder().title("Виртуальная камера").build();
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let paths = Rc::clone(&virt_cam_paths);
            virt_cam_combo.connect_selected_notify(move |c| {
                if s.get() {
                    return;
                }
                if let Some(Some(p)) = paths.borrow().keys.get(c.selected() as usize) {
                    act(UiAction::UpdateConfig(ConfigPatch::VirtualCamera(Some(
                        p.clone(),
                    ))));
                }
            });
        }
        hw_group.add(&virt_cam_combo);

        let real_mic_combo = adw::ComboRow::builder().title("Реальный микрофон").build();
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let backing = Rc::clone(&real_mic_backing);
            real_mic_combo.connect_selected_notify(move |c| {
                if s.get() {
                    return;
                }
                match backing.borrow().entries.get(c.selected() as usize) {
                    Some(MicComboEntry::Disabled) => {
                        act(UiAction::UpdateConfig(ConfigPatch::RealMic(None)));
                    }
                    Some(MicComboEntry::Device(name)) => {
                        act(UiAction::UpdateConfig(ConfigPatch::RealMic(Some(
                            name.clone(),
                        ))));
                    }
                    Some(MicComboEntry::MissingPlaceholder) | None => {}
                }
            });
        }
        hw_group.add(&real_mic_combo);
        devices_box.append(&hw_group);

        let fmt_group = adw::PreferencesGroup::builder()
            .title("Формат вывода")
            .build();
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
        fmt_group.add(&res_combo);
        fmt_group.add(&fps_combo);
        devices_box.append(&fmt_group);

        stack.add_named(&wrap_scroll(&devices_box), Some("devices"));

        // === Page 3: Аудио (Audio Source + Compact VU Meters + Gains + A/V Sync) ===
        let audio_box = make_tab_page_box();

        let audio_route_group = adw::PreferencesGroup::builder()
            .title("Маршрутизация звука")
            .build();
        let audio_combo = adw::ComboRow::builder()
            .title("Источник в VirtualCamMic")
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
        audio_route_group.add(&audio_combo);
        audio_box.append(&audio_route_group);

        let vu_group = adw::PreferencesGroup::builder()
            .title("Уровни сигнала (VU)")
            .build();
        let vu_card = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(8)
            .build();
        vu_card.add_css_class("card");
        vu_card.add_css_class("cyanide-vu-card");

        let (loop_vu_row, loop_meter, loop_db_label) = make_vu_channel("Файл MP4");
        let (live_vu_row, live_meter, live_db_label) = make_vu_channel("Микрофон");
        let (out_vu_row, out_meter, out_db_label) = make_vu_channel("VirtualCamMic");
        vu_card.append(&loop_vu_row);
        vu_card.append(&live_vu_row);
        vu_card.append(&out_vu_row);
        vu_group.add(&vu_card);
        audio_box.append(&vu_group);

        let mix_group = adw::PreferencesGroup::builder()
            .title("Громкость и синхронизация")
            .build();

        let (loop_vol_row, loop_vol_scale, loop_vol_label) =
            make_labeled_slider_row("Громкость MP4", 0.0, 1.5, 0.05, 1.0, "100%");
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let lbl = loop_vol_label.clone();
            loop_vol_scale.connect_value_changed(move |sc| {
                let pct = (sc.value() * 100.0).round() as u32;
                lbl.set_label(&format!("{pct}%"));
                if !s.get() {
                    act(UiAction::UpdateConfig(ConfigPatch::LoopVolume(sc.value())));
                }
            });
        }
        mix_group.add(&loop_vol_row);

        let (live_vol_row, live_vol_scale, live_vol_label) =
            make_labeled_slider_row("Микрофон", 0.0, 1.5, 0.05, 1.0, "100%");
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let lbl = live_vol_label.clone();
            live_vol_scale.connect_value_changed(move |sc| {
                let pct = (sc.value() * 100.0).round() as u32;
                lbl.set_label(&format!("{pct}%"));
                if !s.get() {
                    act(UiAction::UpdateConfig(ConfigPatch::LiveVolume(sc.value())));
                }
            });
        }
        mix_group.add(&live_vol_row);

        let (av_row, av_delay_scale, av_delay_label) =
            make_labeled_slider_row("Задержка A/V", -500.0, 500.0, 10.0, 0.0, "+0 мс");
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
        mix_group.add(&av_row);

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
        mix_group.add(&cf_row);
        audio_box.append(&mix_group);

        stack.add_named(&wrap_scroll(&audio_box), Some("audio"));

        // === Page 4: Петля и лаг (Loop Playback + Bad Internet Mode) ===
        let effects_box = make_tab_page_box();

        let loop_group = adw::PreferencesGroup::builder()
            .title("Зацикленное видео")
            .build();
        let ping_pong_switch = adw::SwitchRow::builder()
            .title("Реверс-петля (вперёд ↔ назад)")
            .subtitle("Бесшовный откат видео в конце вместо скачка")
            .active(false)
            .build();
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            ping_pong_switch.connect_active_notify(move |sw| {
                if !s.get() {
                    act(UiAction::UpdateConfig(ConfigPatch::LoopPingPong(
                        sw.is_active(),
                    )));
                }
            });
        }
        let return_combo = adw::ComboRow::builder()
            .title("При возврате на MP4")
            .model(&gtk4::StringList::new(&[
                "Продолжать с текущей позиции",
                "Начинать с 00:00",
                "Переходить к сохранённой метке",
            ]))
            .build();
        let bookmark_label = gtk4::Label::new(Some("00:00"));
        bookmark_label.add_css_class("cyanide-kbd-chip");
        bookmark_label.add_css_class("numeric");
        bookmark_label.set_valign(gtk4::Align::Center);
        let set_bm_btn = gtk4::Button::with_label("Запомнить");
        set_bm_btn.set_valign(gtk4::Align::Center);
        set_bm_btn.set_tooltip_text(Some("Запомнить текущий кадр MP4 как точку возврата"));
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
        loop_group.add(&ping_pong_switch);
        loop_group.add(&return_combo);
        loop_group.add(&bm_row);
        effects_box.append(&loop_group);

        let lag_cfg = Rc::new(Cell::new(LagConfig::default()));
        let lag_group = adw::PreferencesGroup::builder()
            .title("Режим плохого интернета")
            .build();
        let lag_switch = adw::SwitchRow::builder()
            .title("Случайные подвисания")
            .subtitle("Периодические фризы в live и записанном видео")
            .active(false)
            .build();
        let lag_on_switch_row = adw::SwitchRow::builder()
            .title("Подвисание при смене режима")
            .subtitle("Короткий фриз при переключении Loop ↔ Live")
            .active(false)
            .build();
        let (lag_int_row, lag_interval_scale, lag_interval_label) =
            make_labeled_slider_row("Интервал", 500.0, 15_000.0, 250.0, 3000.0, "3.0 с");
        let (lag_min_row, lag_min_scale, lag_min_label) =
            make_labeled_slider_row("Мин. лаг", 100.0, 5000.0, 50.0, 100.0, "100 мс");
        let (lag_max_row, lag_max_scale, lag_max_label) =
            make_labeled_slider_row("Макс. лаг", 100.0, 5000.0, 50.0, 500.0, "500 мс");

        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let cell = Rc::clone(&lag_cfg);
            lag_switch.connect_active_notify(move |sw| {
                if s.get() {
                    return;
                }
                let mut l = cell.get();
                l.enabled = sw.is_active();
                cell.set(l);
                act(UiAction::UpdateConfig(ConfigPatch::Lag(l)));
            });
        }
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let cell = Rc::clone(&lag_cfg);
            lag_on_switch_row.connect_active_notify(move |sw| {
                if s.get() {
                    return;
                }
                let mut l = cell.get();
                l.freeze_on_switch = sw.is_active();
                cell.set(l);
                act(UiAction::UpdateConfig(ConfigPatch::Lag(l)));
            });
        }
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let cell = Rc::clone(&lag_cfg);
            let lbl = lag_interval_label.clone();
            lag_interval_scale.connect_value_changed(move |sc| {
                let ms = sc.value().round() as u32;
                lbl.set_label(&format!("{:.1} с", ms as f64 / 1000.0));
                if s.get() {
                    return;
                }
                let mut l = cell.get();
                l.interval_ms = ms;
                cell.set(l);
                act(UiAction::UpdateConfig(ConfigPatch::Lag(l)));
            });
        }
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let cell = Rc::clone(&lag_cfg);
            let lbl = lag_min_label.clone();
            lag_min_scale.connect_value_changed(move |sc| {
                let ms = sc.value().round() as u32;
                lbl.set_label(&format!("{ms} мс"));
                if s.get() {
                    return;
                }
                let mut l = cell.get();
                l.min_duration_ms = ms;
                if l.max_duration_ms < ms {
                    l.max_duration_ms = ms;
                }
                cell.set(l);
                act(UiAction::UpdateConfig(ConfigPatch::Lag(l)));
            });
        }
        {
            let act = Rc::clone(&on_action);
            let s = Rc::clone(&syncing);
            let cell = Rc::clone(&lag_cfg);
            let lbl = lag_max_label.clone();
            lag_max_scale.connect_value_changed(move |sc| {
                let ms = sc.value().round() as u32;
                lbl.set_label(&format!("{ms} мс"));
                if s.get() {
                    return;
                }
                let mut l = cell.get();
                l.max_duration_ms = ms;
                if l.min_duration_ms > ms {
                    l.min_duration_ms = ms;
                }
                cell.set(l);
                act(UiAction::UpdateConfig(ConfigPatch::Lag(l)));
            });
        }

        lag_group.add(&lag_switch);
        lag_group.add(&lag_on_switch_row);
        lag_group.add(&lag_int_row);
        lag_group.add(&lag_min_row);
        lag_group.add(&lag_max_row);
        effects_box.append(&lag_group);

        stack.add_named(&wrap_scroll(&effects_box), Some("effects"));
        widget.append(&stack);

        Self {
            widget,
            syncing,
            out_picture,
            monitor_screen,
            device_badge,
            source_badge,
            rec_badge,
            monitor_status_label,
            source_loop_btn,
            source_live_btn,
            quick_mic_btn,
            audio_combo,
            startup_combo,
            hotkey_row,
            hotkey_entry,
            loop_file_row,
            real_cam_combo,
            virt_cam_combo,
            real_mic_combo,
            res_combo,
            fps_combo,
            loop_vol_scale,
            loop_vol_label,
            live_vol_scale,
            live_vol_label,
            loop_meter,
            loop_db_label,
            live_meter,
            live_db_label,
            out_meter,
            out_db_label,
            av_delay_scale,
            av_delay_label,
            crossfade_scale,
            crossfade_label,
            return_combo,
            bookmark_label,
            ping_pong_switch,
            lag_switch,
            lag_on_switch_row,
            lag_int_row,
            lag_interval_scale,
            lag_interval_label,
            lag_min_row,
            lag_min_scale,
            lag_min_label,
            lag_max_row,
            lag_max_scale,
            lag_max_label,
            lag_cfg,
            real_cam_paths,
            virt_cam_paths,
            real_mic_backing,
        }
    }

    pub fn bind_output_paintable(&self, out_p: Option<gdk::Paintable>) {
        self.out_picture.set_paintable(out_p.as_ref());
    }

    pub fn sync_state(
        &self,
        cfg: &Config,
        state: &AppState,
        devices: &DeviceInventory,
        hotkey_error: Option<&str>,
    ) {
        let prev = self.syncing.replace(true);

        // Program monitor status styling: separate Device, Active Source, and Recording state.
        for cls in ["live", "loop", "warn"] {
            self.device_badge.remove_css_class(cls);
        }
        if state.health.output_warning.is_some() || cfg.virtual_camera.is_none() {
            self.device_badge.set_label("Заглушка");
            self.device_badge.add_css_class("warn");
            self.device_badge.set_tooltip_text(
                state
                    .health
                    .output_warning
                    .as_deref()
                    .or(Some("Устройство v4l2loopback не выбрано")),
            );
        } else {
            self.device_badge.set_label("VirtualCam");
            self.device_badge.add_css_class("live");
            self.device_badge.set_tooltip_text(
                cfg.virtual_camera
                    .as_deref()
                    .or(Some("Виртуальная камера активна")),
            );
        }

        for cls in ["live", "loop", "custom", "recording"] {
            self.source_badge.remove_css_class(cls);
            self.monitor_screen.remove_css_class(cls);
        }
        let (src_label, src_cls) = match state.mode.video() {
            VideoSource::Live => ("Камера", "live"),
            VideoSource::Loop => ("Видео", "loop"),
        };
        self.source_badge.set_label(src_label);
        self.source_badge.add_css_class(src_cls);
        self.monitor_screen.add_css_class(if state.recording {
            "recording"
        } else {
            src_cls
        });
        self.rec_badge.set_visible(state.recording);

        match state.mode.video() {
            VideoSource::Loop => self.source_loop_btn.set_active(true),
            VideoSource::Live => self.source_live_btn.set_active(true),
        }

        let audio_desc = if !state.virtual_mic_enabled {
            "выключен (Mute)"
        } else {
            match state.mode.audio() {
                AudioSource::File => "файл MP4",
                AudioSource::Mic => "микрофон",
                AudioSource::Mix => "смесь",
                AudioSource::Silence => "тишина",
            }
        };
        let mic_desc = if state.virtual_mic_enabled {
            "вкл."
        } else {
            "выкл."
        };
        let play_desc = if state.mode.video() == VideoSource::Loop {
            " · петля активна"
        } else {
            ""
        };
        self.monitor_status_label.set_label(&format!(
            "Видео: {src_label} · Звук: {audio_desc} · Микр.: {mic_desc}{play_desc}"
        ));

        self.quick_mic_btn.set_active(state.virtual_mic_enabled);
        if state.virtual_mic_enabled {
            self.quick_mic_btn.remove_css_class("destructive-action");
        } else {
            self.quick_mic_btn.add_css_class("destructive-action");
        }

        let audio_idx = match state.mode.audio() {
            AudioSource::File => 0,
            AudioSource::Mic => 1,
            AudioSource::Mix => 2,
            AudioSource::Silence => 3,
        };
        self.audio_combo.set_selected(audio_idx);
        self.startup_combo.set_selected(match cfg.startup_mode {
            StartupMode::Live => 0,
            StartupMode::Loop => 1,
        });

        let hk_text = cfg.hotkey.as_deref().unwrap_or("");
        if !self.hotkey_entry.has_focus() && self.hotkey_entry.text().as_str() != hk_text {
            self.hotkey_entry.set_text(hk_text);
        }
        let hk_subtitle = match hotkey_error {
            Some(err) => err,
            None if cfg.hotkey.is_some() => "Активен для переключения Видео ↔ Камера",
            None => "Нажмите «Задать» или введите, напр. SUPER+F8",
        };
        self.hotkey_row.set_subtitle(hk_subtitle);

        let file_label = cfg
            .loop_file
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Не выбран".into());
        self.loop_file_row.set_subtitle(&file_label);
        self.loop_file_row.set_tooltip_text(
            cfg.loop_file
                .as_deref()
                .map(|p| p.to_string_lossy().into_owned())
                .as_deref(),
        );

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
        sync_mic_combo(
            &self.real_mic_combo,
            &self.real_mic_backing,
            &devices
                .real_mics
                .iter()
                .map(|d| (d.name.clone(), d.label.clone()))
                .collect::<Vec<_>>(),
            cfg.real_mic.as_deref(),
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
        self.loop_vol_label
            .set_label(&format!("{}%", (cfg.loop_volume * 100.0).round() as u32));
        self.live_vol_scale.set_value(cfg.live_volume);
        self.live_vol_label
            .set_label(&format!("{}%", (cfg.live_volume * 100.0).round() as u32));
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
            .set_label(&format!("{:02}:{:02}", secs / 60, secs % 60));
        self.ping_pong_switch.set_active(cfg.loop_ping_pong);

        self.lag_cfg.set(cfg.lag);
        self.lag_switch.set_active(cfg.lag.enabled);
        self.lag_on_switch_row.set_active(cfg.lag.freeze_on_switch);
        self.lag_int_row.set_sensitive(cfg.lag.enabled);
        let any_lag = cfg.lag.enabled || cfg.lag.freeze_on_switch;
        self.lag_min_row.set_sensitive(any_lag);
        self.lag_max_row.set_sensitive(any_lag);
        self.lag_interval_scale
            .set_value(cfg.lag.interval_ms as f64);
        self.lag_interval_label
            .set_label(&format!("{:.1} с", cfg.lag.interval_ms as f64 / 1000.0));
        self.lag_min_scale.set_value(cfg.lag.min_duration_ms as f64);
        self.lag_min_label
            .set_label(&format!("{} мс", cfg.lag.min_duration_ms));
        self.lag_max_scale.set_value(cfg.lag.max_duration_ms as f64);
        self.lag_max_label
            .set_label(&format!("{} мс", cfg.lag.max_duration_ms));

        self.syncing.set(prev);
    }

    pub fn update_levels(&self, levels: &AudioLevels) {
        self.loop_meter.set_value(db_to_unit(levels.loop_db));
        self.loop_db_label.set_label(&format_db(levels.loop_db));
        self.live_meter.set_value(db_to_unit(levels.live_db));
        self.live_db_label.set_label(&format_db(levels.live_db));
        self.out_meter.set_value(db_to_unit(levels.out_db));
        self.out_db_label.set_label(&format_db(levels.out_db));
    }

    pub fn update_recording_ui(&self, recording: bool, _saving: bool, _elapsed_ms: u64) {
        self.rec_badge.set_visible(recording);
    }
}

fn make_tab_button(label: &str, group: Option<&gtk4::ToggleButton>) -> gtk4::ToggleButton {
    let btn = gtk4::ToggleButton::with_label(label);
    btn.add_css_class("cyanide-tab-btn");
    if let Some(g) = group {
        btn.set_group(Some(g));
    }
    btn
}

fn make_tab_page_box() -> gtk4::Box {
    gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(1)
        .margin_end(1)
        .build()
}

fn wrap_scroll(child: &gtk4::Box) -> gtk4::ScrolledWindow {
    gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(child)
        .build()
}

fn make_vu_channel(title: &str) -> (gtk4::Box, gtk4::LevelBar, gtk4::Label) {
    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 3);
    let top = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let title_lbl = gtk4::Label::new(Some(title));
    title_lbl.set_xalign(0.0);
    title_lbl.set_hexpand(true);
    title_lbl.add_css_class("cyanide-caption");
    let db_lbl = gtk4::Label::new(Some("-∞"));
    db_lbl.add_css_class("cyanide-value-readout");
    db_lbl.add_css_class("numeric");
    db_lbl.set_width_chars(6);
    db_lbl.set_xalign(1.0);
    top.append(&title_lbl);
    top.append(&db_lbl);

    let bar = gtk4::LevelBar::builder()
        .min_value(0.0)
        .max_value(1.0)
        .hexpand(true)
        .build();
    bar.add_css_class("cyanide-vu-bar");

    container.append(&top);
    container.append(&bar);
    (container, bar, db_lbl)
}

fn db_to_unit(db: f64) -> f64 {
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn format_db(db: f64) -> String {
    if !db.is_finite() || db <= -59.5 {
        "-∞".into()
    } else {
        format!("{:.0} дБ", db.clamp(-60.0, 0.0))
    }
}

fn sync_combo(
    combo: &adw::ComboRow,
    backing: &RefCell<ComboBacking>,
    items: Vec<(String, String)>,
    selected: Option<&str>,
    empty_label: &str,
) {
    let (keys, labels, idx) = compute_combo_entries(&items, selected, empty_label);
    let mut state = backing.borrow_mut();
    if state.labels != labels {
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        combo.set_model(Some(&gtk4::StringList::new(&refs)));
        state.labels = labels;
    }
    state.keys = keys;
    if combo.selected() != idx {
        combo.set_selected(idx);
    }
}

fn sync_mic_combo(
    combo: &adw::ComboRow,
    backing: &RefCell<MicComboBacking>,
    items: &[(String, String)],
    selected: Option<&str>,
) {
    let (entries, labels, idx) = compute_optional_mic_entries(items, selected);
    let mut state = backing.borrow_mut();
    if state.labels != labels {
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        combo.set_model(Some(&gtk4::StringList::new(&refs)));
        state.labels = labels;
    }
    state.entries = entries;
    if combo.selected() != idx {
        combo.set_selected(idx);
    }
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
    scale.set_width_request(125);
    scale.set_draw_value(false);
    let label = gtk4::Label::new(Some(initial_text));
    label.set_width_chars(7);
    label.set_xalign(1.0);
    label.add_css_class("cyanide-value-readout");
    label.add_css_class("numeric");
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&label);
    row.add_suffix(&scale);
    (row, scale, label)
}

pub(crate) fn compute_combo_entries(
    items: &[(String, String)],
    selected: Option<&str>,
    empty_label: &str,
) -> (Vec<Option<String>>, Vec<String>, u32) {
    if let Some(sel) = selected {
        if let Some(pos) = items.iter().position(|(k, _)| k == sel) {
            let keys = items.iter().map(|(k, _)| Some(k.clone())).collect();
            let labels = items.iter().map(|(_, l)| l.clone()).collect();
            return (keys, labels, pos as u32);
        }
        // Configured device is absent: prepend a placeholder so index 0 does not trap selection.
        let mut keys = Vec::with_capacity(items.len() + 1);
        let mut labels = Vec::with_capacity(items.len() + 1);
        keys.push(None);
        labels.push(format!("(отключено) {sel}"));
        for (k, l) in items {
            keys.push(Some(k.clone()));
            labels.push(l.clone());
        }
        return (keys, labels, 0);
    }

    if items.is_empty() {
        return (vec![None], vec![empty_label.to_string()], 0);
    }

    let keys = items.iter().map(|(k, _)| Some(k.clone())).collect();
    let labels = items.iter().map(|(_, l)| l.clone()).collect();
    (keys, labels, 0)
}

pub(crate) fn compute_optional_mic_entries(
    items: &[(String, String)],
    selected: Option<&str>,
) -> (Vec<MicComboEntry>, Vec<String>, u32) {
    let mut entries = Vec::with_capacity(items.len() + 2);
    let mut labels = Vec::with_capacity(items.len() + 2);
    entries.push(MicComboEntry::Disabled);
    labels.push("Не передавать (выкл.)".to_string());

    let mut idx = 0u32;
    if let Some(sel) = selected {
        if let Some(pos) = items.iter().position(|(k, _)| k == sel) {
            idx = (pos + 1) as u32;
        } else {
            entries.push(MicComboEntry::MissingPlaceholder);
            labels.push(format!("(отключено) {sel}"));
            idx = 1;
        }
    }

    for (k, l) in items {
        entries.push(MicComboEntry::Device(k.clone()));
        labels.push(l.clone());
    }
    (entries, labels, idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_combo_model_avoids_churn_and_preserves_missing_selection() {
        let items = vec![
            ("/dev/video0".into(), "HD Camera (/dev/video0)".into()),
            ("/dev/video2".into(), "USB Cam (/dev/video2)".into()),
        ];
        let (keys, labels, idx) = compute_combo_entries(&items, Some("/dev/video2"), "Нет камер");
        assert_eq!(
            keys,
            vec![Some("/dev/video0".into()), Some("/dev/video2".into())]
        );
        assert_eq!(labels.len(), 2);
        assert_eq!(idx, 1);

        // Unplugged/missing configured device gets a dedicated index-0 placeholder so
        // picking /dev/video0 (index 1) fires selected-notify instead of being trapped!
        let (keys, labels, idx) = compute_combo_entries(&items, Some("/dev/video99"), "Нет камер");
        assert_eq!(idx, 0);
        assert_eq!(keys[0], None);
        assert_eq!(keys[1], Some("/dev/video0".into()));
        assert!(labels[0].contains("/dev/video99"));
    }

    #[test]
    fn compute_optional_mic_defaults_to_disabled_and_handles_missing() {
        let items = vec![
            ("alsa_input.usb".into(), "USB Mic".into()),
            ("alsa_input.pci".into(), "Built-in Mic".into()),
        ];

        let (entries, labels, idx) = compute_optional_mic_entries(&items, None);
        assert_eq!(idx, 0);
        assert_eq!(entries[0], MicComboEntry::Disabled);
        assert_eq!(entries[1], MicComboEntry::Device("alsa_input.usb".into()));
        assert_eq!(labels.len(), 3);

        let (entries, _, idx) = compute_optional_mic_entries(&items, Some("alsa_input.pci"));
        assert_eq!(idx, 2);
        assert_eq!(entries[2], MicComboEntry::Device("alsa_input.pci".into()));

        let (entries, labels, idx) = compute_optional_mic_entries(&items, Some("missing_mic"));
        assert_eq!(idx, 1);
        assert_eq!(entries[0], MicComboEntry::Disabled);
        assert_eq!(entries[1], MicComboEntry::MissingPlaceholder);
        assert!(labels[1].contains("missing_mic"));
    }
}
