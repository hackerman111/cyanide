use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;

use crate::config::{CompareConfig, CompareMode, CompareTarget, Config};
use crate::gui::compare::{CompareView, match_green_strength};
use crate::gui::{ConfigPatch, UiAction};
use crate::state::{AppState, Command};

pub struct VideoWorkspace {
    pub widget: gtk4::Box,
    compare_view: CompareView,
    header_strip: gtk4::Box,
    loop_name_label: gtk4::Label,
    live_name_label: gtk4::Label,
    frozen_chip: gtk4::Label,
    match_chip: gtk4::Label,
    side_labels_overlay: gtk4::Box,
    fs_top_bar: gtk4::Box,
    fs_badge: gtk4::Label,
    dock_revealer: gtk4::Revealer,
    fullscreen_active: Rc<Cell<bool>>,
    syncing: Rc<Cell<bool>>,
    compare_cfg: Rc<Cell<CompareConfig>>,
    dur_ms: Rc<Cell<Option<u64>>>,
    scrub_timer: Rc<RefCell<Option<glib::SourceId>>>,
    mode_overlay_btn: gtk4::ToggleButton,
    mode_side_btn: gtk4::ToggleButton,
    mode_split_btn: gtk4::ToggleButton,
    opacity_box: gtk4::Box,
    opacity_0_btn: gtk4::Button,
    opacity_50_btn: gtk4::Button,
    opacity_100_btn: gtk4::Button,
    opacity_scale: gtk4::Scale,
    opacity_value_label: gtk4::Label,
    split_orient_box: gtk4::Box,
    split_vert_btn: gtk4::ToggleButton,
    split_horiz_btn: gtk4::ToggleButton,
    side_hint_label: gtk4::Label,
    grid_btn: gtk4::ToggleButton,
    center_btn: gtk4::ToggleButton,
    guides_btn: gtk4::ToggleButton,
    similarity_btn: gtk4::ToggleButton,
    freeze_btn: gtk4::ToggleButton,
    target_both_btn: gtk4::ToggleButton,
    target_loop_btn: gtk4::ToggleButton,
    target_live_btn: gtk4::ToggleButton,
    mirror_btn: gtk4::ToggleButton,
    transform_btn: gtk4::MenuButton,
    zoom_scale: gtk4::Scale,
    zoom_label: gtk4::Label,
    pan_x_scale: gtk4::Scale,
    pan_x_label: gtk4::Label,
    pan_y_scale: gtk4::Scale,
    pan_y_label: gtk4::Label,
    guide_x_scale: gtk4::Scale,
    guide_x_label: gtk4::Label,
    guide_y_scale: gtk4::Scale,
    guide_y_label: gtk4::Label,
    scrub_scale: gtk4::Scale,
    timecode_label: gtk4::Label,
    seek_start_btn: gtk4::Button,
    bookmark_btn: gtk4::Button,
}

impl VideoWorkspace {
    pub fn new(on_action: Rc<dyn Fn(UiAction)>, syncing: Rc<Cell<bool>>) -> Self {
        let compare_cfg = Rc::new(Cell::new(CompareConfig::default()));
        let fullscreen_active = Rc::new(Cell::new(false));
        let dur_ms: Rc<Cell<Option<u64>>> = Rc::new(Cell::new(None));
        let scrub_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

        let widget = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(8)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(8)
            .hexpand(true)
            .vexpand(true)
            .build();
        widget.set_size_request(420, -1);

        // --- Non-overlapping Top Layer Info Strip ---
        let header_strip = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .build();
        header_strip.add_css_class("cyanide-workspace-strip");

        let loop_tag = gtk4::Label::new(Some("Запись"));
        loop_tag.add_css_class("cyanide-layer-tag");
        loop_tag.add_css_class("loop");

        let loop_name_label = gtk4::Label::new(Some("Файл не выбран"));
        loop_name_label.add_css_class("cyanide-caption");
        loop_name_label.set_xalign(0.0);
        loop_name_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        loop_name_label.set_hexpand(true);

        let frozen_chip = gtk4::Label::new(Some("СТОП-КАДР"));
        frozen_chip.add_css_class("cyanide-hud-chip");
        frozen_chip.add_css_class("frozen-chip");
        frozen_chip.set_visible(false);

        let match_chip = gtk4::Label::new(Some("Схожесть 0%"));
        match_chip.add_css_class("cyanide-match-pill");
        match_chip.add_css_class("numeric");
        match_chip.set_visible(false);

        let live_tag = gtk4::Label::new(Some("Камера"));
        live_tag.add_css_class("cyanide-layer-tag");
        live_tag.add_css_class("live");

        let live_name_label = gtk4::Label::new(Some("Не выбрана"));
        live_name_label.add_css_class("cyanide-caption");
        live_name_label.set_xalign(1.0);
        live_name_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);

        header_strip.append(&loop_tag);
        header_strip.append(&loop_name_label);
        header_strip.append(&frozen_chip);
        header_strip.append(&match_chip);
        header_strip.append(&live_tag);
        header_strip.append(&live_name_label);
        widget.append(&header_strip);

        // --- Main Video Canvas ---
        let compare_view = CompareView::new();
        compare_view.set_size_request(400, 260);
        compare_view.set_interactive(true);

        // Side-by-side explicit layer labels (only shown in SideBySide mode).
        let side_left_badge = gtk4::Label::new(Some("Запись"));
        side_left_badge.add_css_class("cyanide-layer-tag");
        side_left_badge.add_css_class("loop");
        let side_spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        side_spacer.set_hexpand(true);
        let side_right_badge = gtk4::Label::new(Some("Камера"));
        side_right_badge.add_css_class("cyanide-layer-tag");
        side_right_badge.add_css_class("live");

        let side_labels_overlay = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .margin_top(10)
            .margin_start(12)
            .margin_end(12)
            .valign(gtk4::Align::Start)
            .build();
        side_labels_overlay.set_can_target(false);
        side_labels_overlay.set_visible(false);
        side_labels_overlay.append(&side_left_badge);
        side_labels_overlay.append(&side_spacer);
        side_labels_overlay.append(&side_right_badge);

        // Minimal fullscreen HUD bar (only visible in fullscreen mode).
        let fs_badge = gtk4::Label::new(Some("Наложение · 50%"));
        fs_badge.add_css_class("cyanide-hud-chip");
        let fs_spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        fs_spacer.set_hexpand(true);
        let fs_exit_btn = gtk4::Button::with_label("Выйти из полноэкранного (Esc)");
        fs_exit_btn.add_css_class("cyanide-hud-chip");
        {
            let act = Rc::clone(&on_action);
            fs_exit_btn.connect_clicked(move |_| act(UiAction::ToggleFullscreen(false)));
        }
        let fs_top_bar = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .margin_top(12)
            .margin_start(12)
            .margin_end(12)
            .valign(gtk4::Align::Start)
            .build();
        fs_top_bar.set_visible(false);
        fs_top_bar.append(&fs_badge);
        fs_top_bar.append(&fs_spacer);
        fs_top_bar.append(&fs_exit_btn);

        let viewport_overlay = gtk4::Overlay::new();
        viewport_overlay.set_child(Some(&compare_view));
        viewport_overlay.add_overlay(&side_labels_overlay);
        viewport_overlay.add_overlay(&fs_top_bar);
        viewport_overlay.set_overflow(gtk4::Overflow::Hidden);
        viewport_overlay.add_css_class("cyanide-viewport-frame");
        viewport_overlay.set_hexpand(true);
        viewport_overlay.set_vexpand(true);
        widget.append(&viewport_overlay);

        // --- Studio Compare & Playback Dock ---
        let dock_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(8)
            .build();
        dock_box.add_css_class("cyanide-toolbar");

        // Row 1: Compare Mode + Visual Aids + Freeze
        let row1 = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .build();

        let mode_overlay_btn = gtk4::ToggleButton::with_label("Наложение");
        let mode_side_btn = gtk4::ToggleButton::with_label("Рядом");
        let mode_split_btn = gtk4::ToggleButton::with_label("Разделитель");
        mode_overlay_btn.set_tooltip_text(Some("Наложение камеры на видео (1)"));
        mode_side_btn.set_tooltip_text(Some("Два кадра рядом (2)"));
        mode_split_btn.set_tooltip_text(Some("Интерактивная граница сравнения (3)"));
        mode_side_btn.set_group(Some(&mode_overlay_btn));
        mode_split_btn.set_group(Some(&mode_overlay_btn));
        mode_overlay_btn.set_active(true);

        let mode_group = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        mode_group.add_css_class("linked");
        mode_group.append(&mode_overlay_btn);
        mode_group.append(&mode_side_btn);
        mode_group.append(&mode_split_btn);
        row1.append(&mode_group);

        let r1_spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        r1_spacer.set_hexpand(true);
        row1.append(&r1_spacer);

        let grid_btn = make_icon_toggle("view-grid-symbolic", "Сетка 3×3 (правило третей)");
        let center_btn = gtk4::ToggleButton::with_label("Центр");
        center_btn.set_tooltip_text(Some("Показать центральные оси кадра"));
        let guides_btn = gtk4::ToggleButton::with_label("Оси");
        guides_btn.set_tooltip_text(Some("Пользовательские направляющие X/Y"));
        let similarity_btn = gtk4::ToggleButton::with_label("Схож.");
        similarity_btn.set_tooltip_text(Some("Режим сравнения похожести (Запись ↔ Камера)"));

        let aids_group = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        aids_group.add_css_class("linked");
        aids_group.append(&grid_btn);
        aids_group.append(&center_btn);
        aids_group.append(&guides_btn);
        aids_group.append(&similarity_btn);
        row1.append(&aids_group);
        dock_box.append(&row1);

        // Row 2: Mode-specific options (Opacity / Split Orientation) + Freeze Frame
        let row2 = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .build();

        let opacity_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        opacity_box.set_valign(gtk4::Align::Center);
        let opacity_title = gtk4::Label::new(Some("Прозр. камеры:"));
        opacity_title.add_css_class("cyanide-caption");
        opacity_title.set_tooltip_text(Some(
            "Прозрачность слоя камеры (0% — запись, 100% — камера)",
        ));

        let opacity_0_btn = gtk4::Button::with_label("0%");
        let opacity_50_btn = gtk4::Button::with_label("50%");
        let opacity_100_btn = gtk4::Button::with_label("100%");
        opacity_0_btn.set_tooltip_text(Some("Только запись (0% камеры)"));
        opacity_50_btn.set_tooltip_text(Some("Половинное наложение (50%)"));
        opacity_100_btn.set_tooltip_text(Some("Только камера (100%)"));
        let op_presets = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        op_presets.add_css_class("linked");
        op_presets.append(&opacity_0_btn);
        op_presets.append(&opacity_50_btn);
        op_presets.append(&opacity_100_btn);

        let opacity_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1.0, 0.01);
        opacity_scale.set_value(0.5);
        opacity_scale.set_width_request(72);
        opacity_scale.set_draw_value(false);
        opacity_scale.set_tooltip_text(Some("Прозрачность слоя камеры (Ctrl+колёсико над кадром)"));
        let opacity_value_label = gtk4::Label::new(Some("50%"));
        opacity_value_label.add_css_class("cyanide-value-readout");
        opacity_value_label.add_css_class("numeric");
        opacity_value_label.set_width_chars(4);

        opacity_box.append(&opacity_title);
        opacity_box.append(&op_presets);
        opacity_box.append(&opacity_scale);
        opacity_box.append(&opacity_value_label);
        row2.append(&opacity_box);

        let split_orient_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        split_orient_box.set_valign(gtk4::Align::Center);
        let split_title = gtk4::Label::new(Some("Граница:"));
        split_title.add_css_class("cyanide-caption");
        let split_vert_btn = gtk4::ToggleButton::with_label("Вертик.");
        let split_horiz_btn = gtk4::ToggleButton::with_label("Горизонт.");
        split_horiz_btn.set_group(Some(&split_vert_btn));
        split_vert_btn.set_active(true);
        let split_group = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        split_group.add_css_class("linked");
        split_group.append(&split_vert_btn);
        split_group.append(&split_horiz_btn);
        split_orient_box.append(&split_title);
        split_orient_box.append(&split_group);
        split_orient_box.set_visible(false);
        row2.append(&split_orient_box);

        let side_hint_label = gtk4::Label::new(Some("Синхронный масштаб обоих кадров"));
        side_hint_label.add_css_class("cyanide-caption");
        side_hint_label.set_visible(false);
        row2.append(&side_hint_label);

        let r2_spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        r2_spacer.set_hexpand(true);
        row2.append(&r2_spacer);

        let freeze_btn = gtk4::ToggleButton::new();
        let freeze_inner = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        freeze_inner.append(&gtk4::Image::from_icon_name(
            "media-playback-pause-symbolic",
        ));
        freeze_inner.append(&gtk4::Label::new(Some("Стоп-кадр")));
        freeze_btn.set_child(Some(&freeze_inner));
        freeze_btn.set_tooltip_text(Some("Заморозить эталонный кадр в сравнении (Пробел)"));
        freeze_btn.add_css_class("cyanide-freeze-active");
        row2.append(&freeze_btn);
        dock_box.append(&row2);

        // Row 3: Target Layer Alignment + MP4 Playback Scrubbing
        let row3 = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .build();

        let target_label = gtk4::Label::new(Some("Слой:"));
        target_label.add_css_class("cyanide-caption");
        let target_both_btn = gtk4::ToggleButton::with_label("Оба");
        let target_loop_btn = gtk4::ToggleButton::with_label("Запись");
        let target_live_btn = gtk4::ToggleButton::with_label("Камера");
        target_both_btn.set_tooltip_text(Some("Масштаб и сдвиг для обоих слоёв сразу"));
        target_loop_btn.set_tooltip_text(Some("Подогнать масштаб/сдвиг только для записи"));
        target_live_btn.set_tooltip_text(Some("Подогнать масштаб/сдвиг только для камеры"));
        target_loop_btn.set_group(Some(&target_both_btn));
        target_live_btn.set_group(Some(&target_both_btn));
        target_both_btn.set_active(true);

        let target_group = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        target_group.add_css_class("linked");
        target_group.append(&target_both_btn);
        target_group.append(&target_loop_btn);
        target_group.append(&target_live_btn);
        row3.append(&target_label);
        row3.append(&target_group);

        let mirror_btn = make_icon_toggle(
            "object-flip-horizontal-symbolic",
            "Отзеркалить выбранный слой по горизонтали",
        );
        row3.append(&mirror_btn);

        // Popover for Zoom, Pan X/Y, Guide X/Y
        let pop_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(8)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .width_request(280)
            .build();
        let pop_heading = gtk4::Label::new(Some("Положение, масштаб и оси"));
        pop_heading.set_xalign(0.0);
        pop_heading.add_css_class("cyanide-section-title");
        pop_box.append(&pop_heading);

        let (zoom_row, zoom_scale, zoom_label) =
            make_popover_slider("Масштаб", 0.5, 3.0, 0.05, 1.0, "1.0×");
        let (pan_x_row, pan_x_scale, pan_x_label) =
            make_popover_slider("Сдвиг X", -1.0, 1.0, 0.02, 0.0, "0%");
        let (pan_y_row, pan_y_scale, pan_y_label) =
            make_popover_slider("Сдвиг Y", -1.0, 1.0, 0.02, 0.0, "0%");
        let (guide_x_row, guide_x_scale, guide_x_label) =
            make_popover_slider("Ось X", 0.0, 1.0, 0.01, 0.5, "50%");
        let (guide_y_row, guide_y_scale, guide_y_label) =
            make_popover_slider("Ось Y", 0.0, 1.0, 0.01, 0.5, "50%");

        pop_box.append(&zoom_row);
        pop_box.append(&pan_x_row);
        pop_box.append(&pan_y_row);
        pop_box.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        pop_box.append(&guide_x_row);
        pop_box.append(&guide_y_row);

        let hint_lbl = gtk4::Label::new(Some(
            "Колёсико — масштаб · Ctrl+колёсико — прозр.\nСтрелки — сдвиг · 2× клик — сброс слоя",
        ));
        hint_lbl.add_css_class("cyanide-caption");
        hint_lbl.set_xalign(0.0);
        pop_box.append(&hint_lbl);

        let popover = gtk4::Popover::builder().child(&pop_box).build();
        let transform_btn = gtk4::MenuButton::builder()
            .label("1.0×")
            .popover(&popover)
            .tooltip_text("Точная настройка масштаба, сдвига и направляющих")
            .build();
        transform_btn.add_css_class("numeric");
        row3.append(&transform_btn);

        let reset_btn = gtk4::Button::with_label("Сброс");
        reset_btn.set_tooltip_text(Some("Сбросить масштаб, сдвиг и зеркало всех слоёв (R)"));
        row3.append(&reset_btn);

        row3.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

        let seek_start_btn = gtk4::Button::from_icon_name("media-skip-backward-symbolic");
        seek_start_btn.set_tooltip_text(Some("Перемотать запись в начало (00:00)"));
        row3.append(&seek_start_btn);

        let scrub_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1.0, 0.005);
        scrub_scale.set_hexpand(true);
        scrub_scale.set_width_request(72);
        scrub_scale.set_draw_value(false);
        scrub_scale.set_sensitive(false);
        scrub_scale.set_tooltip_text(Some("Перемотка зацикленного видео"));
        row3.append(&scrub_scale);

        let timecode_label = gtk4::Label::new(Some("00:00 / --:--"));
        timecode_label.add_css_class("cyanide-value-readout");
        timecode_label.add_css_class("numeric");
        timecode_label.set_width_chars(12);
        row3.append(&timecode_label);

        let bookmark_btn = gtk4::Button::with_label("Метка");
        bookmark_btn.set_tooltip_text(Some("Запомнить текущий кадр MP4 как точку возврата"));
        row3.append(&bookmark_btn);
        dock_box.append(&row3);

        let dock_revealer = gtk4::Revealer::builder()
            .transition_type(gtk4::RevealerTransitionType::SlideUp)
            .transition_duration(150)
            .reveal_child(true)
            .child(&dock_box)
            .build();
        widget.append(&dock_revealer);

        // Fullscreen pointer hover auto-reveal for dock controls.
        let motion = gtk4::EventControllerMotion::new();
        {
            let fs = Rc::clone(&fullscreen_active);
            let rev = dock_revealer.clone();
            let w_ref = widget.clone();
            motion.connect_motion(move |_, _, y| {
                if !fs.get() {
                    return;
                }
                let h = w_ref.height() as f64;
                let near_bottom = h <= 1.0 || y >= (h - 170.0);
                if rev.reveals_child() != near_bottom {
                    rev.set_reveal_child(near_bottom);
                }
            });
        }
        {
            let fs = Rc::clone(&fullscreen_active);
            let rev = dock_revealer.clone();
            motion.connect_leave(move |_| {
                if fs.get() {
                    rev.set_reveal_child(false);
                }
            });
        }
        widget.add_controller(motion);

        // --- Wire Interactive Canvas Callback ---
        {
            let act = Rc::clone(&on_action);
            let cfg_cell = Rc::clone(&compare_cfg);
            compare_view.connect_config_changed(move |c| {
                cfg_cell.set(c);
                act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
            });
        }

        // --- Wire Compare Mode & Aids ---
        wire_compare_toggle(
            &mode_overlay_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, active| {
                if active {
                    c.mode = CompareMode::Overlay;
                }
            },
        );
        wire_compare_toggle(
            &mode_side_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, active| {
                if active {
                    c.mode = CompareMode::SideBySide;
                }
            },
        );
        wire_compare_toggle(
            &mode_split_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, active| {
                if active {
                    c.mode = CompareMode::Split;
                }
            },
        );
        wire_compare_toggle(
            &split_vert_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, active| {
                if active {
                    c.split_vertical = true;
                }
            },
        );
        wire_compare_toggle(
            &split_horiz_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, active| {
                if active {
                    c.split_vertical = false;
                }
            },
        );
        wire_compare_toggle(
            &grid_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, a| c.show_grid = a,
        );
        wire_compare_toggle(
            &center_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, a| c.show_center = a,
        );
        wire_compare_toggle(
            &guides_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, a| c.show_guides = a,
        );
        wire_compare_toggle(
            &similarity_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, a| c.similarity = a,
        );

        // --- Wire Target Layer Selection & Transforms ---
        wire_compare_toggle(
            &target_both_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, active| {
                if active {
                    c.target = CompareTarget::Both;
                }
            },
        );
        wire_compare_toggle(
            &target_loop_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, active| {
                if active {
                    c.target = CompareTarget::Loop;
                }
            },
        );
        wire_compare_toggle(
            &target_live_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, active| {
                if active {
                    c.target = CompareTarget::Live;
                }
            },
        );
        wire_compare_toggle(
            &mirror_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, a| {
                let (zoom, px, py, _) = c.active_transform();
                c.set_active_transform(zoom, px, py, a);
            },
        );

        // --- Wire Opacity Presets & Sliders ---
        for (btn, preset) in [
            (&opacity_0_btn, 0.0),
            (&opacity_50_btn, 0.5),
            (&opacity_100_btn, 1.0),
        ] {
            let cell = Rc::clone(&compare_cfg);
            let cv = compare_view.clone();
            let act = Rc::clone(&on_action);
            btn.connect_clicked(move |_| {
                let mut c = cell.get();
                c.opacity = preset;
                cell.set(c);
                cv.set_config(c);
                act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
            });
        }

        wire_compare_scale(
            &opacity_scale,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, v| c.opacity = v,
        );
        wire_compare_scale(
            &zoom_scale,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, v| {
                let (_, px, py, mirror) = c.active_transform();
                c.set_active_transform(v, px, py, mirror);
            },
        );
        wire_compare_scale(
            &pan_x_scale,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, v| {
                let (zoom, _, py, mirror) = c.active_transform();
                c.set_active_transform(zoom, v, py, mirror);
            },
        );
        wire_compare_scale(
            &pan_y_scale,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, v| {
                let (zoom, px, _, mirror) = c.active_transform();
                c.set_active_transform(zoom, px, v, mirror);
            },
        );
        wire_compare_scale(
            &guide_x_scale,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, v| c.guide_x = v,
        );
        wire_compare_scale(
            &guide_y_scale,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, v| c.guide_y = v,
        );

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
                c.reset_transforms();
                c.opacity = 0.5;
                c.split_pos = 0.5;
                c.guide_x = 0.5;
                c.guide_y = 0.5;
                cfg_cell.set(c);
                cv.set_config(c);
                act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
            });
        }

        // --- Wire MP4 Playback Controls ---
        {
            let act = Rc::clone(&on_action);
            seek_start_btn.connect_clicked(move |_| act(UiAction::SeekLoopMs(0)));
        }
        {
            let act = Rc::clone(&on_action);
            bookmark_btn.connect_clicked(move |_| act(UiAction::SetReturnBookmarkCurrent));
        }
        {
            let s = Rc::clone(&syncing);
            let dur_cell = Rc::clone(&dur_ms);
            let timer_cell = Rc::clone(&scrub_timer);
            let act = Rc::clone(&on_action);
            let tc_lbl = timecode_label.clone();
            scrub_scale.connect_value_changed(move |sc| {
                if s.get() {
                    return;
                }
                let Some(total) = dur_cell.get().filter(|&d| d > 0) else {
                    return;
                };
                let target_ms = ((sc.value().clamp(0.0, 1.0)) * total as f64).round() as u64;
                let pos_s = target_ms / 1000;
                let tot_s = total / 1000;
                tc_lbl.set_label(&format!(
                    "{:02}:{:02} / {:02}:{:02}",
                    pos_s / 60,
                    pos_s % 60,
                    tot_s / 60,
                    tot_s % 60
                ));
                if let Some(prev_id) = timer_cell.borrow_mut().take() {
                    prev_id.remove();
                }
                let act_cb = Rc::clone(&act);
                let t_slot = Rc::clone(&timer_cell);
                let id = glib::timeout_add_local_once(Duration::from_millis(45), move || {
                    *t_slot.borrow_mut() = None;
                    act_cb(UiAction::SeekLoopMs(target_ms));
                });
                *timer_cell.borrow_mut() = Some(id);
            });
        }

        Self {
            widget,
            compare_view,
            header_strip,
            loop_name_label,
            live_name_label,
            frozen_chip,
            match_chip,
            side_labels_overlay,
            fs_top_bar,
            fs_badge,
            dock_revealer,
            fullscreen_active,
            syncing,
            compare_cfg,
            dur_ms,
            scrub_timer,
            mode_overlay_btn,
            mode_side_btn,
            mode_split_btn,
            opacity_box,
            opacity_0_btn,
            opacity_50_btn,
            opacity_100_btn,
            opacity_scale,
            opacity_value_label,
            split_orient_box,
            split_vert_btn,
            split_horiz_btn,
            side_hint_label,
            grid_btn,
            center_btn,
            guides_btn,
            similarity_btn,
            freeze_btn,
            target_both_btn,
            target_loop_btn,
            target_live_btn,
            mirror_btn,
            transform_btn,
            zoom_scale,
            zoom_label,
            pan_x_scale,
            pan_x_label,
            pan_y_scale,
            pan_y_label,
            guide_x_scale,
            guide_x_label,
            guide_y_scale,
            guide_y_label,
            scrub_scale,
            timecode_label,
            seek_start_btn,
            bookmark_btn,
        }
    }

    pub fn bind_paintables(&self, loop_p: Option<gdk::Paintable>, live_p: Option<gdk::Paintable>) {
        self.compare_view.set_paintables(loop_p, live_p);
    }

    pub fn set_fullscreen(&self, fullscreen: bool) {
        self.fullscreen_active.set(fullscreen);
        self.header_strip.set_visible(!fullscreen);
        self.fs_top_bar.set_visible(fullscreen);
        self.dock_revealer.set_reveal_child(!fullscreen);
        if fullscreen {
            self.compare_view.grab_focus();
        }
    }

    pub fn sync_state(&self, cfg: &Config, state: &AppState) {
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

        let is_overlay = cfg.compare.mode == CompareMode::Overlay;
        let is_split = cfg.compare.mode == CompareMode::Split;
        let is_side = cfg.compare.mode == CompareMode::SideBySide;
        self.opacity_box.set_visible(is_overlay);
        self.split_orient_box.set_visible(is_split);
        self.side_hint_label.set_visible(is_side);
        self.side_labels_overlay.set_visible(is_side);

        self.opacity_scale.set_value(cfg.compare.opacity);
        let op_pct = (cfg.compare.opacity * 100.0).round() as u32;
        self.opacity_value_label.set_label(&format!("{op_pct}%"));
        for (btn, target) in [
            (&self.opacity_0_btn, 0.0),
            (&self.opacity_50_btn, 0.5),
            (&self.opacity_100_btn, 1.0),
        ] {
            if (cfg.compare.opacity - target).abs() <= 0.02 {
                btn.add_css_class("suggested-action");
            } else {
                btn.remove_css_class("suggested-action");
            }
        }

        if cfg.compare.split_vertical {
            self.split_vert_btn.set_active(true);
        } else {
            self.split_horiz_btn.set_active(true);
        }

        let fs_text = match cfg.compare.mode {
            CompareMode::Overlay => format!("Наложение · Камера {op_pct}%"),
            CompareMode::SideBySide => "Рядом · Запись | Камера".to_string(),
            CompareMode::Split => {
                if cfg.compare.split_vertical {
                    "Разделитель · Вертикальный".to_string()
                } else {
                    "Разделитель · Горизонтальный".to_string()
                }
            }
        };
        self.fs_badge.set_label(&fs_text);

        self.grid_btn.set_active(cfg.compare.show_grid);
        self.center_btn.set_active(cfg.compare.show_center);
        self.guides_btn.set_active(cfg.compare.show_guides);
        self.similarity_btn.set_active(cfg.compare.similarity);
        self.match_chip.set_visible(cfg.compare.similarity);
        self.compare_view
            .set_match_highlight(cfg.compare.similarity);
        if !cfg.compare.similarity {
            self.update_similarity(0.0);
        }
        self.freeze_btn.set_active(state.frozen_reference);
        self.frozen_chip.set_visible(state.frozen_reference);

        match cfg.compare.target {
            CompareTarget::Both => self.target_both_btn.set_active(true),
            CompareTarget::Loop => self.target_loop_btn.set_active(true),
            CompareTarget::Live => self.target_live_btn.set_active(true),
        }

        let (zoom, pan_x, pan_y, mirror) = cfg.compare.active_transform();
        let min_zoom = if cfg.compare.target == CompareTarget::Both {
            1.0
        } else {
            0.5
        };
        self.zoom_scale.set_range(min_zoom, 3.0);
        self.mirror_btn.set_active(mirror);
        self.zoom_scale.set_value(zoom);
        self.zoom_label.set_label(&format!("{zoom:.2}×"));
        self.transform_btn.set_label(&format!("{zoom:.1}×"));

        self.pan_x_scale.set_value(pan_x);
        self.pan_x_label
            .set_label(&format!("{:+}%", (pan_x * 100.0).round() as i32));
        self.pan_y_scale.set_value(pan_y);
        self.pan_y_label
            .set_label(&format!("{:+}%", (pan_y * 100.0).round() as i32));

        self.guide_x_scale.set_value(cfg.compare.guide_x);
        self.guide_x_label.set_label(&format!(
            "{}%",
            (cfg.compare.guide_x * 100.0).round() as u32
        ));
        self.guide_y_scale.set_value(cfg.compare.guide_y);
        self.guide_y_label.set_label(&format!(
            "{}%",
            (cfg.compare.guide_y * 100.0).round() as u32
        ));

        let loop_label = cfg
            .loop_file
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Файл не выбран".into());
        self.loop_name_label.set_label(&loop_label);
        self.loop_name_label.set_tooltip_text(
            cfg.loop_file
                .as_deref()
                .map(|p| p.to_string_lossy().into_owned())
                .as_deref(),
        );
        let has_loop = cfg.loop_file.is_some();
        self.seek_start_btn.set_sensitive(has_loop);
        self.bookmark_btn.set_sensitive(has_loop);

        let cam_label = cfg.real_camera.as_deref().unwrap_or("Не выбрана");
        self.live_name_label.set_label(cam_label);
        self.live_name_label.set_tooltip_text(Some(cam_label));

        self.syncing.set(prev);
    }

    pub fn update_timecode(
        &self,
        _file_name: &str,
        pos_ms: u64,
        dur_ms: Option<u64>,
        reversed: bool,
    ) {
        self.dur_ms.set(dur_ms);
        let pos_s = pos_ms / 1000;
        let dur_str = dur_ms
            .map(|d| {
                let s = d / 1000;
                format!("{:02}:{:02}", s / 60, s % 60)
            })
            .unwrap_or_else(|| "--:--".into());
        let dir = if reversed { " ◀" } else { "" };
        self.timecode_label.set_label(&format!(
            "{:02}:{:02}{} / {}",
            pos_s / 60,
            pos_s % 60,
            dir,
            dur_str
        ));

        let can_scrub = dur_ms.is_some_and(|d| d > 0) && self.scrub_timer.borrow().is_none();
        self.scrub_scale
            .set_sensitive(dur_ms.is_some_and(|d| d > 0));
        if can_scrub && let Some(total) = dur_ms {
            let prev = self.syncing.replace(true);
            let frac = (pos_ms as f64 / total as f64).clamp(0.0, 1.0);
            self.scrub_scale.set_value(frac);
            self.syncing.set(prev);
        }
    }

    pub fn update_similarity(&self, score: f32) {
        self.compare_view.set_similarity(score);
        let pct = (score.clamp(0.0, 1.0) * 100.0).round() as u32;
        self.match_chip.set_label(&format!("Схожесть {pct}%"));
        if match_green_strength(score) >= 0.45 {
            self.match_chip.add_css_class("matched");
        } else {
            self.match_chip.remove_css_class("matched");
        }
    }
}

fn make_icon_toggle(icon_name: &str, tooltip: &str) -> gtk4::ToggleButton {
    let btn = gtk4::ToggleButton::new();
    btn.set_icon_name(icon_name);
    btn.set_tooltip_text(Some(tooltip));
    btn
}

fn make_popover_slider(
    title: &str,
    min: f64,
    max: f64,
    step: f64,
    initial: f64,
    initial_text: &str,
) -> (gtk4::Box, gtk4::Scale, gtk4::Label) {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let title_lbl = gtk4::Label::new(Some(title));
    title_lbl.set_xalign(0.0);
    title_lbl.set_width_chars(9);
    title_lbl.add_css_class("cyanide-caption");

    let scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, min, max, step);
    scale.set_value(initial);
    scale.set_hexpand(true);
    scale.set_draw_value(false);

    let val_lbl = gtk4::Label::new(Some(initial_text));
    val_lbl.set_width_chars(5);
    val_lbl.set_xalign(1.0);
    val_lbl.add_css_class("cyanide-value-readout");
    val_lbl.add_css_class("numeric");

    row.append(&title_lbl);
    row.append(&scale);
    row.append(&val_lbl);
    (row, scale, val_lbl)
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
