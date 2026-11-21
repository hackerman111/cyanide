use std::process::Command as ProcessCommand;
use std::rc::Rc;
use std::sync::Once;

use gtk4::gdk;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::{CompareMode, Config};
use crate::gui::UiAction;
use crate::gui::compare::{CompareView, match_green_strength};
use crate::hotkey::is_hyprland;
use crate::state::{AppState, Command, StreamMode, VideoSource};

pub const OVERLAY_TITLE: &str = "Cyanide Overlay";

const OVERLAY_CSS: &str = "
window.cyanide-overlay {
    background-color: transparent;
}
.cyanide-overlay-hud {
    background-color: rgba(15, 17, 22, 0.72);
    border-radius: 10px;
    padding: 4px 8px;
}
.cyanide-match-pill {
    color: rgba(255, 255, 255, 0.9);
    background-color: rgba(255, 255, 255, 0.14);
    border-radius: 999px;
    padding: 2px 8px;
    font-weight: 600;
}
.cyanide-match-pill.matched {
    color: #072812;
    background-color: rgba(52, 232, 118, 0.92);
}
";

static CSS_INIT: Once = Once::new();
static HYPR_RULES_INIT: Once = Once::new();

pub struct OverlayWindow {
    window: gtk4::Window,
    compare_view: CompareView,
    mode_btn: gtk4::Button,
    match_label: gtk4::Label,
}

impl OverlayWindow {
    pub fn new(
        app: &adw::Application,
        on_action: Rc<dyn Fn(UiAction)>,
        on_visibility: Rc<dyn Fn(bool)>,
    ) -> Self {
        install_css();
        install_hyprland_float_rules();

        let window = gtk4::Window::builder()
            .application(app)
            .title(OVERLAY_TITLE)
            .default_width(420)
            .default_height(250)
            .decorated(false)
            .resizable(true)
            .hide_on_close(true)
            .build();
        window.add_css_class("cyanide-overlay");

        let vis_cb = Rc::clone(&on_visibility);
        window.connect_visible_notify(move |w| {
            vis_cb(w.is_visible());
        });

        let compare_view = CompareView::new();
        compare_view.set_match_highlight(true);
        compare_view.set_size_request(260, 150);

        let match_label = gtk4::Label::new(Some("0%"));
        match_label.add_css_class("caption");
        match_label.add_css_class("numeric");
        match_label.add_css_class("cyanide-match-pill");
        match_label.set_tooltip_text(Some("Схожесть live-камеры с видео"));

        let mode_btn = gtk4::Button::with_label("Режим: LIVE");
        mode_btn.add_css_class("pill");
        mode_btn.add_css_class("suggested-action");
        {
            let act = Rc::clone(&on_action);
            mode_btn.connect_clicked(move |_| act(UiAction::Command(Command::Toggle)));
        }

        let close_btn = gtk4::Button::from_icon_name("window-close-symbolic");
        close_btn.add_css_class("circular");
        close_btn.add_css_class("flat");
        close_btn.set_tooltip_text(Some("Скрыть плавающее окно"));
        {
            let win = window.clone();
            close_btn.connect_clicked(move |_| win.set_visible(false));
        }

        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);

        let hud = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .valign(gtk4::Align::End)
            .build();
        hud.add_css_class("cyanide-overlay-hud");
        hud.append(&match_label);
        hud.append(&spacer);
        hud.append(&mode_btn);
        hud.append(&close_btn);

        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&compare_view));
        overlay.add_overlay(&hud);
        overlay.set_opacity(0.78);

        // WindowHandle lets the user drag the borderless floating window anywhere.
        let handle = gtk4::WindowHandle::new();
        handle.set_child(Some(&overlay));
        window.set_child(Some(&handle));

        Self {
            window,
            compare_view,
            mode_btn,
            match_label,
        }
    }

    pub fn bind_paintables(&self, loop_p: Option<gdk::Paintable>, live_p: Option<gdk::Paintable>) {
        self.compare_view.set_paintables(loop_p, live_p);
    }

    pub fn sync_state(&self, cfg: &Config, state: &AppState) {
        let mut cmp = cfg.compare;
        // Floating alignment overlay always shows blended camera + 3x3 grid.
        cmp.mode = CompareMode::Overlay;
        cmp.show_grid = true;
        self.compare_view.set_config(cmp);
        if self.compare_view.is_frozen() != state.frozen_reference {
            self.compare_view.set_frozen(state.frozen_reference);
        }

        let (label, is_live) = match state.mode {
            StreamMode::Live => ("Режим: LIVE", true),
            StreamMode::Loop => ("Режим: LOOP", false),
            StreamMode::Custom { video, .. } => match video {
                VideoSource::Live => ("Видео: LIVE", true),
                VideoSource::Loop => ("Видео: LOOP", false),
            },
        };
        self.mode_btn.set_label(label);
        if is_live {
            self.mode_btn.add_css_class("suggested-action");
        } else {
            self.mode_btn.remove_css_class("suggested-action");
        }
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
        self.window.present();
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }
}

fn install_css() {
    CSS_INIT.call_once(|| {
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(OVERLAY_CSS);
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
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
