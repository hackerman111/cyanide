use std::cell::Cell;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::prelude::*;

use crate::config::{CompareConfig, CompareMode, Config};
use crate::gui::compare::{CompareView, match_green_strength};
use crate::gui::{ConfigPatch, UiAction};
use crate::state::{AppState, Command};

pub struct VideoWorkspace {
    pub widget: gtk4::Box,
    compare_view: CompareView,
    loop_chip: gtk4::Label,
    live_chip: gtk4::Label,
    frozen_chip: gtk4::Label,
    match_chip: gtk4::Label,
    syncing: Rc<Cell<bool>>,
    compare_cfg: Rc<Cell<CompareConfig>>,
    mode_overlay_btn: gtk4::ToggleButton,
    mode_side_btn: gtk4::ToggleButton,
    mode_split_btn: gtk4::ToggleButton,
    opacity_box: gtk4::Box,
    opacity_scale: gtk4::Scale,
    opacity_value_label: gtk4::Label,
    grid_btn: gtk4::ToggleButton,
    center_btn: gtk4::ToggleButton,
    guides_btn: gtk4::ToggleButton,
    mirror_btn: gtk4::ToggleButton,
    freeze_btn: gtk4::ToggleButton,
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
}

impl VideoWorkspace {
    pub fn new(on_action: Rc<dyn Fn(UiAction)>, syncing: Rc<Cell<bool>>) -> Self {
        let compare_cfg = Rc::new(Cell::new(CompareConfig::default()));

        let widget = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(10)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(12)
            .margin_end(10)
            .hexpand(true)
            .vexpand(true)
            .build();

        // --- Viewport Canvas with Translucent Corner HUD ---
        let compare_view = CompareView::new();
        compare_view.set_size_request(540, 340);
        compare_view.set_interactive(true);

        let loop_chip = gtk4::Label::new(Some("LOOP · Нет MP4"));
        loop_chip.add_css_class("cyanide-hud-chip");
        loop_chip.add_css_class("loop-chip");
        loop_chip.add_css_class("numeric");
        loop_chip.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        loop_chip.set_max_width_chars(40);

        let frozen_chip = gtk4::Label::new(Some("СТОП-КАДР"));
        frozen_chip.add_css_class("cyanide-hud-chip");
        frozen_chip.add_css_class("frozen-chip");
        frozen_chip.set_visible(false);

        let match_chip = gtk4::Label::new(Some("Схожесть 0%"));
        match_chip.add_css_class("cyanide-match-pill");
        match_chip.add_css_class("numeric");

        let live_chip = gtk4::Label::new(Some("LIVE · камера не выбрана"));
        live_chip.add_css_class("cyanide-hud-chip");
        live_chip.add_css_class("live-chip");
        live_chip.add_css_class("numeric");
        live_chip.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        live_chip.set_max_width_chars(32);

        let hud_spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        hud_spacer.set_hexpand(true);

        let top_hud = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .margin_top(10)
            .margin_start(10)
            .margin_end(10)
            .valign(gtk4::Align::Start)
            .build();
        // Let mouse drags/scrolls pass through to CompareView.
        top_hud.set_can_target(false);
        top_hud.append(&loop_chip);
        top_hud.append(&hud_spacer);
        top_hud.append(&frozen_chip);
        top_hud.append(&match_chip);
        top_hud.append(&live_chip);

        let viewport_overlay = gtk4::Overlay::new();
        viewport_overlay.set_child(Some(&compare_view));
        viewport_overlay.add_overlay(&top_hud);
        viewport_overlay.set_overflow(gtk4::Overflow::Hidden);
        viewport_overlay.add_css_class("cyanide-viewport-frame");
        viewport_overlay.set_hexpand(true);
        viewport_overlay.set_vexpand(true);
        widget.append(&viewport_overlay);

        // --- Sleek Single-Row Studio Compare Toolbar ---
        let toolbar = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .build();
        toolbar.add_css_class("card");
        toolbar.add_css_class("cyanide-toolbar");

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
        toolbar.append(&mode_group);

        let opacity_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        opacity_box.set_valign(gtk4::Align::Center);
        let opacity_title = gtk4::Label::new(Some("Прозр."));
        opacity_title.add_css_class("caption");
        opacity_title.add_css_class("dim-label");
        let opacity_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1.0, 0.01);
        opacity_scale.set_value(0.5);
        opacity_scale.set_width_request(96);
        opacity_scale.set_draw_value(false);
        opacity_scale.set_tooltip_text(Some("Прозрачность слоя камеры (0–100%)"));
        let opacity_value_label = gtk4::Label::new(Some("50%"));
        opacity_value_label.add_css_class("caption");
        opacity_value_label.add_css_class("numeric");
        opacity_value_label.add_css_class("dim-label");
        opacity_value_label.set_width_chars(4);
        opacity_box.append(&opacity_title);
        opacity_box.append(&opacity_scale);
        opacity_box.append(&opacity_value_label);
        toolbar.append(&opacity_box);

        let tb_spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        tb_spacer.set_hexpand(true);
        toolbar.append(&tb_spacer);

        let grid_btn = make_icon_toggle("view-grid-symbolic", "Сетка 3×3 (правило третей)");
        let center_btn = gtk4::ToggleButton::with_label("╋");
        center_btn.set_tooltip_text(Some("Центральные оси"));
        let guides_btn = gtk4::ToggleButton::with_label("Напр.");
        guides_btn.set_tooltip_text(Some("Пользовательские направляющие X/Y"));
        let mirror_btn = make_icon_toggle(
            "object-flip-horizontal-symbolic",
            "Отзеркалить предпросмотр по горизонтали",
        );
        let freeze_btn = make_icon_toggle(
            "media-playback-pause-symbolic",
            "Заморозить эталонный кадр MP4",
        );
        freeze_btn.add_css_class("cyanide-freeze-active");

        let aids_group = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        aids_group.add_css_class("linked");
        aids_group.append(&grid_btn);
        aids_group.append(&center_btn);
        aids_group.append(&guides_btn);
        aids_group.append(&mirror_btn);
        toolbar.append(&aids_group);
        toolbar.append(&freeze_btn);

        // --- Transform & Guides Popover ---
        let pop_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(8)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(12)
            .margin_end(12)
            .width_request(265)
            .build();

        let pop_heading = gtk4::Label::new(Some("Трансформация и направляющие"));
        pop_heading.set_xalign(0.0);
        pop_heading.add_css_class("heading");
        pop_box.append(&pop_heading);

        let (zoom_row, zoom_scale, zoom_label) =
            make_popover_slider("Зум", 1.0, 3.0, 0.05, 1.0, "1.0×");
        let (pan_x_row, pan_x_scale, pan_x_label) =
            make_popover_slider("Сдвиг X", -1.0, 1.0, 0.02, 0.0, "0%");
        let (pan_y_row, pan_y_scale, pan_y_label) =
            make_popover_slider("Сдвиг Y", -1.0, 1.0, 0.02, 0.0, "0%");
        let (guide_x_row, guide_x_scale, guide_x_label) =
            make_popover_slider("Напр. X", 0.0, 1.0, 0.01, 0.5, "50%");
        let (guide_y_row, guide_y_scale, guide_y_label) =
            make_popover_slider("Напр. Y", 0.0, 1.0, 0.01, 0.5, "50%");

        pop_box.append(&zoom_row);
        pop_box.append(&pan_x_row);
        pop_box.append(&pan_y_row);
        pop_box.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        pop_box.append(&guide_x_row);
        pop_box.append(&guide_y_row);

        let pop_footer = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let hint_lbl = gtk4::Label::new(Some("Колёсико — зум · 2× клик — сброс"));
        hint_lbl.add_css_class("caption");
        hint_lbl.add_css_class("dim-label");
        hint_lbl.set_hexpand(true);
        hint_lbl.set_xalign(0.0);
        let reset_btn = gtk4::Button::with_label("Сброс");
        reset_btn.add_css_class("flat");
        pop_footer.append(&hint_lbl);
        pop_footer.append(&reset_btn);
        pop_box.append(&pop_footer);

        let popover = gtk4::Popover::builder().child(&pop_box).build();
        let transform_btn = gtk4::MenuButton::builder()
            .label("1.0×")
            .popover(&popover)
            .tooltip_text("Зум, сдвиг и позиции направляющих")
            .build();
        transform_btn.add_css_class("numeric");
        toolbar.append(&transform_btn);

        widget.append(&toolbar);

        // --- Wire Interactive Canvas Callback ---
        {
            let act = Rc::clone(&on_action);
            let cfg_cell = Rc::clone(&compare_cfg);
            compare_view.connect_config_changed(move |c| {
                cfg_cell.set(c);
                act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
            });
        }

        // --- Wire Toolbar Toggles & Sliders ---
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
            &mirror_btn,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, a| c.mirror = a,
        );

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
            |c, v| c.zoom = v,
        );
        wire_compare_scale(
            &pan_x_scale,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, v| c.pan_x = v,
        );
        wire_compare_scale(
            &pan_y_scale,
            &syncing,
            &compare_cfg,
            &compare_view,
            &on_action,
            |c, v| c.pan_y = v,
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
                c.zoom = 1.0;
                c.pan_x = 0.0;
                c.pan_y = 0.0;
                c.mirror = false;
                c.opacity = 0.5;
                c.split_pos = 0.5;
                c.guide_x = 0.5;
                c.guide_y = 0.5;
                cfg_cell.set(c);
                cv.set_config(c);
                act(UiAction::UpdateConfig(ConfigPatch::Compare(c)));
            });
        }

        Self {
            widget,
            compare_view,
            loop_chip,
            live_chip,
            frozen_chip,
            match_chip,
            syncing,
            compare_cfg,
            mode_overlay_btn,
            mode_side_btn,
            mode_split_btn,
            opacity_box,
            opacity_scale,
            opacity_value_label,
            grid_btn,
            center_btn,
            guides_btn,
            mirror_btn,
            freeze_btn,
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
        }
    }

    pub fn bind_paintables(&self, loop_p: Option<gdk::Paintable>, live_p: Option<gdk::Paintable>) {
        self.compare_view.set_paintables(loop_p, live_p);
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
        self.opacity_box.set_sensitive(is_overlay);
        self.opacity_scale.set_value(cfg.compare.opacity);
        self.opacity_value_label.set_label(&format!(
            "{}%",
            (cfg.compare.opacity * 100.0).round() as u32
        ));

        self.grid_btn.set_active(cfg.compare.show_grid);
        self.center_btn.set_active(cfg.compare.show_center);
        self.guides_btn.set_active(cfg.compare.show_guides);
        self.mirror_btn.set_active(cfg.compare.mirror);
        self.freeze_btn.set_active(state.frozen_reference);
        self.frozen_chip.set_visible(state.frozen_reference);

        self.zoom_scale.set_value(cfg.compare.zoom);
        self.zoom_label
            .set_label(&format!("{:.2}×", cfg.compare.zoom));
        self.transform_btn
            .set_label(&format!("{:.1}×", cfg.compare.zoom));

        self.pan_x_scale.set_value(cfg.compare.pan_x);
        self.pan_x_label.set_label(&format!(
            "{:+}%",
            (cfg.compare.pan_x * 100.0).round() as i32
        ));
        self.pan_y_scale.set_value(cfg.compare.pan_y);
        self.pan_y_label.set_label(&format!(
            "{:+}%",
            (cfg.compare.pan_y * 100.0).round() as i32
        ));

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

        let cam_label = cfg.real_camera.as_deref().unwrap_or("нет камеры");
        self.live_chip.set_label(&format!(
            "LIVE · {} · {}p{}",
            cam_label, cfg.video.height, cfg.video.fps
        ));

        self.syncing.set(prev);
    }

    pub fn update_timecode(
        &self,
        file_name: &str,
        pos_ms: u64,
        dur_ms: Option<u64>,
        reversed: bool,
    ) {
        let pos_s = pos_ms / 1000;
        let dur_str = dur_ms
            .map(|d| {
                let s = d / 1000;
                format!("{:02}:{:02}", s / 60, s % 60)
            })
            .unwrap_or_else(|| "--:--".into());
        let dir = if reversed { " ◀" } else { "" };
        self.loop_chip.set_label(&format!(
            "LOOP · {} · {:02}:{:02}{} / {}",
            file_name,
            pos_s / 60,
            pos_s % 60,
            dir,
            dur_str
        ));
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
    title_lbl.set_width_chars(8);
    title_lbl.add_css_class("caption");

    let scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, min, max, step);
    scale.set_value(initial);
    scale.set_hexpand(true);
    scale.set_draw_value(false);

    let val_lbl = gtk4::Label::new(Some(initial_text));
    val_lbl.set_width_chars(5);
    val_lbl.set_xalign(1.0);
    val_lbl.add_css_class("caption");
    val_lbl.add_css_class("numeric");
    val_lbl.add_css_class("dim-label");

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
