use std::cell::Cell;
use std::process::Command as ProcessCommand;
use std::rc::Rc;
use std::sync::Once;

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::{CompareMode, Config};
use crate::gui::UiAction;
use crate::gui::compare::{CompareView, match_green_strength};
use crate::gui::style::install_app_style;
use crate::hotkey::is_hyprland;
use crate::state::{AppState, Command, StreamMode, VideoSource};

pub const OVERLAY_TITLE: &str = "Cyanide Overlay";
pub const DEFAULT_OVERLAY_SIZE: (i32, i32) = (440, 260);
pub const DEFAULT_OVERLAY_OPACITY: f64 = 0.84;

static HYPR_RULES_INIT: Once = Once::new();

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayMetrics {
    pub size: (i32, i32),
    pub opacity: f64,
}

pub struct OverlayWindow {
    window: gtk4::Window,
    compare_view: CompareView,
    mode_btn: gtk4::Button,
    match_label: gtk4::Label,
    frozen_badge: gtk4::Label,
    freeze_btn: gtk4::ToggleButton,
    grid_enabled: Rc<Cell<bool>>,
    opacity_value: Rc<Cell<f64>>,
    syncing: Rc<Cell<bool>>,
    closed: Rc<Cell<bool>>,
}

impl OverlayWindow {
    pub fn new(
        app: &adw::Application,
        initial: OverlayMetrics,
        on_action: Rc<dyn Fn(UiAction)>,
        on_closed: Rc<dyn Fn(OverlayMetrics)>,
    ) -> Self {
        install_app_style();
        install_hyprland_float_rules();

        let width = initial.size.0.clamp(240, 1920);
        let height = initial.size.1.clamp(150, 1200);
        let init_opacity = initial.opacity.clamp(0.35, 1.0);

        let window = gtk4::Window::builder()
            .application(app)
            .title(OVERLAY_TITLE)
            .default_width(width)
            .default_height(height)
            .decorated(false)
            .resizable(true)
            .hide_on_close(false)
            .build();
        window.add_css_class("cyanide-overlay");

        let closed = Rc::new(Cell::new(false));
        let syncing = Rc::new(Cell::new(false));
        let grid_enabled = Rc::new(Cell::new(true));
        let opacity_value = Rc::new(Cell::new(init_opacity));

        let notify_closed = {
            let closed_flag = Rc::clone(&closed);
            let op_cell = Rc::clone(&opacity_value);
            let cb = Rc::clone(&on_closed);
            move |w: &gtk4::Window| {
                if closed_flag.replace(true) {
                    return;
                }
                let size = snapshot_window_size(w, (width, height));
                cb(OverlayMetrics {
                    size,
                    opacity: op_cell.get(),
                });
            }
        };

        {
            let notify = notify_closed.clone();
            window.connect_close_request(move |w| {
                notify(w);
                glib::Propagation::Proceed
            });
        }
        {
            let notify = notify_closed;
            window.connect_destroy(move |w| {
                notify(w);
            });
        }

        let compare_view = CompareView::new();
        compare_view.set_interactive(false);
        compare_view.set_match_highlight(false);
        compare_view.set_size_request(260, 150);

        // Top HUD badges (non-blocking for window drag)
        let match_label = gtk4::Label::new(Some("0%"));
        match_label.add_css_class("numeric");
        match_label.add_css_class("cyanide-match-pill");
        match_label.set_tooltip_text(Some("Схожесть live-камеры с видео"));
        match_label.set_visible(false);

        let frozen_badge = gtk4::Label::new(Some("СТОП-КАДР"));
        frozen_badge.add_css_class("cyanide-hud-chip");
        frozen_badge.add_css_class("frozen-chip");
        frozen_badge.set_visible(false);

        let top_spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        top_spacer.set_hexpand(true);

        let top_hud = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .margin_top(8)
            .margin_start(8)
            .margin_end(8)
            .valign(gtk4::Align::Start)
            .build();
        top_hud.set_can_target(false);
        top_hud.append(&match_label);
        top_hud.append(&top_spacer);
        top_hud.append(&frozen_badge);

        // Bottom Frosted Glass HUD Bar
        let opacity_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.35, 1.0, 0.02);
        opacity_scale.set_value(init_opacity);
        opacity_scale.set_width_request(72);
        opacity_scale.set_draw_value(false);
        opacity_scale.set_tooltip_text(Some("Прозрачность плавающего окна"));

        let freeze_btn = gtk4::ToggleButton::new();
        freeze_btn.set_icon_name("media-playback-pause-symbolic");
        freeze_btn.add_css_class("flat");
        freeze_btn.add_css_class("cyanide-freeze-active");
        freeze_btn.set_tooltip_text(Some("Заморозить эталонный кадр"));
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

        let grid_btn = gtk4::ToggleButton::new();
        grid_btn.set_icon_name("view-grid-symbolic");
        grid_btn.add_css_class("flat");
        grid_btn.set_active(true);
        grid_btn.set_tooltip_text(Some("Показать/скрыть сетку 3×3"));
        {
            let ge = Rc::clone(&grid_enabled);
            let act = Rc::clone(&on_action);
            grid_btn.connect_toggled(move |b| {
                ge.set(b.is_active());
                act(UiAction::RefreshOverlay);
            });
        }

        let mode_btn = gtk4::Button::with_label("LIVE");
        mode_btn.add_css_class("pill");
        mode_btn.add_css_class("cyanide-overlay-mode-live");
        mode_btn.set_tooltip_text(Some("Переключить режим Loop ↔ Live"));
        {
            let act = Rc::clone(&on_action);
            mode_btn.connect_clicked(move |_| act(UiAction::Command(Command::Toggle)));
        }

        let close_btn = gtk4::Button::from_icon_name("window-close-symbolic");
        close_btn.add_css_class("circular");
        close_btn.add_css_class("flat");
        close_btn.set_tooltip_text(Some("Закрыть плавающее окно"));
        {
            let win = window.clone();
            close_btn.connect_clicked(move |_| win.close());
        }

        let hud_spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        hud_spacer.set_hexpand(true);

        let bottom_hud = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .valign(gtk4::Align::End)
            .build();
        bottom_hud.add_css_class("cyanide-overlay-hud");
        bottom_hud.append(&opacity_scale);
        bottom_hud.append(&freeze_btn);
        bottom_hud.append(&grid_btn);
        bottom_hud.append(&hud_spacer);
        bottom_hud.append(&mode_btn);
        bottom_hud.append(&close_btn);

        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&compare_view));
        overlay.add_overlay(&top_hud);
        overlay.add_overlay(&bottom_hud);
        overlay.set_overflow(gtk4::Overflow::Hidden);
        overlay.add_css_class("cyanide-overlay-frame");
        overlay.set_opacity(init_opacity);

        {
            let ov_ref = overlay.clone();
            let op_cell = Rc::clone(&opacity_value);
            opacity_scale.connect_value_changed(move |sc| {
                let v = sc.value().clamp(0.35, 1.0);
                op_cell.set(v);
                ov_ref.set_opacity(v);
            });
        }

        // WindowHandle lets the user drag the borderless floating window from anywhere.
        let handle = gtk4::WindowHandle::new();
        handle.set_child(Some(&overlay));
        window.set_child(Some(&handle));

        Self {
            window,
            compare_view,
            mode_btn,
            match_label,
            frozen_badge,
            freeze_btn,
            grid_enabled,
            opacity_value,
            syncing,
            closed,
        }
    }

    pub fn bind_paintables(&self, loop_p: Option<gdk::Paintable>, live_p: Option<gdk::Paintable>) {
        self.compare_view.set_paintables(loop_p, live_p);
    }

    pub fn sync_state(&self, cfg: &Config, state: &AppState) {
        let prev = self.syncing.replace(true);

        let mut cmp = cfg.compare;
        // Floating alignment overlay always shows blended camera + optional 3x3 grid.
        cmp.mode = CompareMode::Overlay;
        cmp.show_grid = self.grid_enabled.get();
        self.compare_view.set_config(cmp);
        self.compare_view
            .set_match_highlight(cfg.compare.similarity);
        self.match_label.set_visible(cfg.compare.similarity);
        if !cfg.compare.similarity {
            self.set_similarity(0.0);
        }
        if self.compare_view.is_frozen() != state.frozen_reference {
            self.compare_view.set_frozen(state.frozen_reference);
        }
        self.freeze_btn.set_active(state.frozen_reference);
        self.frozen_badge.set_visible(state.frozen_reference);

        let (label, is_live) = match state.mode {
            StreamMode::Live => ("LIVE", true),
            StreamMode::Loop => ("LOOP", false),
            StreamMode::Custom { video, .. } => match video {
                VideoSource::Live => ("LIVE", true),
                VideoSource::Loop => ("LOOP", false),
            },
        };
        self.mode_btn.set_label(label);
        if is_live {
            self.mode_btn.add_css_class("cyanide-overlay-mode-live");
            self.mode_btn.remove_css_class("cyanide-overlay-mode-loop");
        } else {
            self.mode_btn.add_css_class("cyanide-overlay-mode-loop");
            self.mode_btn.remove_css_class("cyanide-overlay-mode-live");
        }

        self.syncing.set(prev);
    }

    pub fn set_similarity(&self, score: f32) {
        self.compare_view.set_similarity(score);
        let pct = (score.clamp(0.0, 1.0) * 100.0).round() as u32;
        self.match_label.set_label(&format!("{pct}%"));
        if match_green_strength(score) >= 0.45 {
            self.match_label.add_css_class("matched");
        } else {
            self.match_label.remove_css_class("matched");
        }
    }

    pub fn present(&self) {
        if self.closed.get() {
            return;
        }
        self.window.set_visible(true);
        self.window.present();
    }

    /// Cleanly destroys this overlay window and returns its latest size/opacity so
    /// reopening creates a fresh Wayland surface with the user's preferred dimensions.
    pub fn close_and_destroy(&self) -> OverlayMetrics {
        let metrics = OverlayMetrics {
            size: snapshot_window_size(&self.window, DEFAULT_OVERLAY_SIZE),
            opacity: self.opacity_value.get(),
        };
        self.closed.set(true);
        self.compare_view.set_paintables(None, None);
        self.window.destroy();
        metrics
    }

    pub fn is_visible(&self) -> bool {
        !self.closed.get() && self.window.is_visible()
    }
}

fn snapshot_window_size(window: &gtk4::Window, fallback: (i32, i32)) -> (i32, i32) {
    let w = window.width();
    let h = window.height();
    if w >= 160 && h >= 100 {
        (w, h)
    } else {
        fallback
    }
}

fn install_hyprland_float_rules() {
    if !is_hyprland() {
        return;
    }
    HYPR_RULES_INIT.call_once(|| {
        // Ensure the floating overlay opens floated and pinned across workspaces on Hyprland.
        let matcher = format!("match:title ^({OVERLAY_TITLE})$");
        for effect in ["float on", "pin on"] {
            let _ = ProcessCommand::new("hyprctl")
                .args(["keyword", "windowrule", &format!("{effect}, {matcher}")])
                .output();
        }
    });
}
