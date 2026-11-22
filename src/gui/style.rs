use std::sync::Once;

use gtk4::gdk;
use libadwaita as adw;

static STYLE_INIT: Once = Once::new();

const APP_CSS: &str = "
/* --- Studio Theme Accents & Pills --- */
.cyanide-status-pill {
    border-radius: 999px;
    padding: 3px 10px;
    font-weight: 700;
    font-size: 0.84em;
    letter-spacing: 0.02em;
    border: 1px solid transparent;
}
.cyanide-status-pill.live {
    background-color: rgba(46, 204, 113, 0.16);
    color: #4ade80;
    border-color: rgba(74, 222, 128, 0.32);
}
.cyanide-status-pill.loop {
    background-color: rgba(168, 85, 247, 0.18);
    color: #c084fc;
    border-color: rgba(192, 132, 252, 0.34);
}
.cyanide-status-pill.custom {
    background-color: rgba(56, 189, 248, 0.16);
    color: #38bdf8;
    border-color: rgba(56, 189, 248, 0.32);
}
.cyanide-status-pill.recording {
    background-color: rgba(239, 68, 68, 0.22);
    color: #f87171;
    border-color: rgba(248, 113, 113, 0.42);
}
.cyanide-status-pill.muted {
    background-color: rgba(245, 158, 11, 0.16);
    color: #fbbf24;
    border-color: rgba(251, 191, 36, 0.32);
}

/* --- Main Video Canvas Frame & Floating HUD Chips --- */
.cyanide-viewport-frame {
    background-color: #0b0d12;
    border-radius: 14px;
    border: 1px solid rgba(255, 255, 255, 0.09);
}

.cyanide-hud-chip {
    background-color: rgba(12, 14, 20, 0.78);
    color: rgba(245, 247, 252, 0.92);
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 999px;
    padding: 3px 10px;
    font-size: 0.82em;
    font-weight: 600;
}
.cyanide-hud-chip.loop-chip {
    border-color: rgba(192, 132, 252, 0.38);
}
.cyanide-hud-chip.live-chip {
    border-color: rgba(74, 222, 128, 0.36);
}
.cyanide-hud-chip.frozen-chip {
    background-color: rgba(245, 158, 11, 0.88);
    color: #140f02;
    border-color: rgba(251, 191, 36, 0.95);
    font-weight: 700;
}

.cyanide-match-pill {
    color: rgba(255, 255, 255, 0.92);
    background-color: rgba(15, 18, 26, 0.80);
    border: 1px solid rgba(255, 255, 255, 0.15);
    border-radius: 999px;
    padding: 3px 10px;
    font-size: 0.82em;
    font-weight: 700;
}
.cyanide-match-pill.matched {
    color: #052812;
    background-color: rgba(52, 232, 118, 0.92);
    border-color: rgba(134, 239, 172, 0.95);
}

/* --- Compare Toolbar --- */
.cyanide-toolbar {
    border-radius: 12px;
    padding: 6px 10px;
}

.cyanide-freeze-active:checked {
    background-color: rgba(245, 158, 11, 0.25);
    color: #fbbf24;
}

/* --- Right Studio Dock: Program Monitor --- */
.cyanide-monitor-card {
    border-radius: 14px;
    padding: 10px;
}

.cyanide-monitor-screen {
    background-color: #090b10;
    border-radius: 10px;
    border: 2px solid rgba(255, 255, 255, 0.08);
}
.cyanide-monitor-screen.live {
    border-color: rgba(74, 222, 128, 0.45);
}
.cyanide-monitor-screen.loop {
    border-color: rgba(192, 132, 252, 0.48);
}
.cyanide-monitor-screen.custom {
    border-color: rgba(56, 189, 248, 0.48);
}
.cyanide-monitor-screen.recording {
    border-color: rgba(248, 113, 113, 0.72);
}

.cyanide-mode-live:checked {
    background-color: rgba(46, 204, 113, 0.22);
    color: #4ade80;
}
.cyanide-mode-loop:checked {
    background-color: rgba(168, 85, 247, 0.24);
    color: #d8b4fe;
}

/* --- Compact Tab Strip --- */
.cyanide-tab-strip {
    padding: 3px;
    border-radius: 10px;
    background-color: rgba(255, 255, 255, 0.05);
}
.cyanide-tab-btn {
    border-radius: 8px;
    padding: 5px 8px;
    font-size: 0.86em;
    font-weight: 600;
    border: none;
    background: transparent;
    box-shadow: none;
}
.cyanide-tab-btn:checked {
    background-color: rgba(255, 255, 255, 0.12);
    color: @window_fg_color;
}

/* --- VU Meters --- */
.cyanide-vu-card {
    border-radius: 12px;
    padding: 10px 12px;
}
.cyanide-vu-bar trough {
    min-height: 7px;
    border-radius: 999px;
}
.cyanide-vu-bar block.filled {
    border-radius: 999px;
}

/* --- Footer Status Strip --- */
.cyanide-footer {
    border-top: 1px solid rgba(255, 255, 255, 0.07);
    padding: 5px 12px;
}
.cyanide-kbd-chip {
    background-color: rgba(255, 255, 255, 0.08);
    border: 1px solid rgba(255, 255, 255, 0.14);
    border-radius: 6px;
    padding: 1px 7px;
    font-size: 0.80em;
    font-weight: 700;
}
.cyanide-lag-chip {
    background-color: rgba(245, 158, 11, 0.16);
    color: #fbbf24;
    border: 1px solid rgba(251, 191, 36, 0.34);
    border-radius: 999px;
    padding: 1px 8px;
    font-size: 0.80em;
    font-weight: 700;
}

/* --- Floating Overlay Window --- */
window.cyanide-overlay {
    background-color: transparent;
}
.cyanide-overlay-frame {
    background-color: rgba(11, 13, 18, 0.88);
    border-radius: 14px;
    border: 1px solid rgba(255, 255, 255, 0.16);
}
.cyanide-overlay-hud {
    background-color: rgba(12, 14, 20, 0.84);
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 12px;
    padding: 4px 8px;
}
.cyanide-overlay-mode-live {
    background-color: rgba(46, 204, 113, 0.86);
    color: #052812;
    font-weight: 700;
}
.cyanide-overlay-mode-loop {
    background-color: rgba(168, 85, 247, 0.88);
    color: #faf5ff;
    font-weight: 700;
}
";

pub fn install_app_style() {
    STYLE_INIT.call_once(|| {
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::PreferDark);
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(APP_CSS);
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}
