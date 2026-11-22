use std::sync::Once;

use gtk4::gdk;
use libadwaita as adw;

static STYLE_INIT: Once = Once::new();

const APP_CSS: &str = "
/* --- Design System Tokens & Typography (12-15px, WCAG AA >= 4.5:1) --- */
window {
    font-size: 13px;
}

.cyanide-section-title {
    font-size: 14px;
    font-weight: 700;
    letter-spacing: 0.01em;
    color: #f3f4f6;
}

.cyanide-caption {
    font-size: 12px;
    color: #b8c2d0;
}

.cyanide-value-readout {
    font-size: 12px;
    font-weight: 600;
    color: #e2e8f0;
}

button, togglebutton, menubutton > button {
    min-height: 32px;
    border-radius: 8px;
}

button:focus-visible,
togglebutton:focus-visible,
menubutton > button:focus-visible,
scale:focus-visible slider,
entry:focus-visible,
row:focus-visible {
    outline: 2px solid #38bdf8;
    outline-offset: 2px;
}

.cyanide-primary-cta {
    min-height: 36px;
    padding: 6px 14px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 700;
}

/* --- Semantic State Pills --- */
.cyanide-status-pill {
    border-radius: 8px;
    padding: 4px 10px;
    font-weight: 700;
    font-size: 12px;
    letter-spacing: 0.01em;
    border: 1px solid rgba(255, 255, 255, 0.14);
    background-color: rgba(255, 255, 255, 0.06);
    color: #e5e7eb;
}
.cyanide-status-pill.live {
    background-color: rgba(34, 197, 94, 0.18);
    color: #86efac;
    border-color: rgba(74, 222, 128, 0.42);
}
.cyanide-status-pill.loop {
    background-color: rgba(168, 85, 247, 0.22);
    color: #e9d5ff;
    border-color: rgba(192, 132, 252, 0.46);
}
.cyanide-status-pill.custom {
    background-color: rgba(56, 189, 248, 0.18);
    color: #bae6fd;
    border-color: rgba(56, 189, 248, 0.42);
}
.cyanide-status-pill.recording {
    background-color: rgba(239, 68, 68, 0.26);
    color: #fecaca;
    border-color: rgba(248, 113, 113, 0.58);
}
.cyanide-status-pill.warn {
    background-color: rgba(245, 158, 11, 0.20);
    color: #fde68a;
    border-color: rgba(251, 191, 36, 0.45);
}
.cyanide-status-pill.muted {
    background-color: rgba(148, 163, 184, 0.16);
    color: #cbd5e1;
    border-color: rgba(148, 163, 184, 0.34);
}

/* --- Left Workspace: Header Strip, Viewport & Compare Dock --- */
.cyanide-workspace-strip {
    background-color: rgba(24, 28, 36, 0.92);
    border: 1px solid rgba(255, 255, 255, 0.10);
    border-radius: 10px;
    padding: 6px 12px;
}

.cyanide-layer-tag {
    font-size: 12px;
    font-weight: 700;
    padding: 2px 8px;
    border-radius: 6px;
}
.cyanide-layer-tag.loop {
    background-color: rgba(168, 85, 247, 0.22);
    color: #e9d5ff;
    border: 1px solid rgba(192, 132, 252, 0.42);
}
.cyanide-layer-tag.live {
    background-color: rgba(34, 197, 94, 0.18);
    color: #86efac;
    border: 1px solid rgba(74, 222, 128, 0.38);
}

.cyanide-viewport-frame {
    background-color: #0b0d12;
    border-radius: 12px;
    border: 1px solid rgba(255, 255, 255, 0.12);
}

.cyanide-hud-chip {
    background-color: rgba(12, 14, 20, 0.88);
    color: #f3f4f6;
    border: 1px solid rgba(255, 255, 255, 0.18);
    border-radius: 8px;
    padding: 4px 10px;
    font-size: 12px;
    font-weight: 600;
}
.cyanide-hud-chip.frozen-chip {
    background-color: rgba(245, 158, 11, 0.94);
    color: #140f02;
    border-color: rgba(251, 191, 36, 0.95);
    font-weight: 700;
}

.cyanide-match-pill {
    color: #f3f4f6;
    background-color: rgba(15, 18, 26, 0.88);
    border: 1px solid rgba(255, 255, 255, 0.20);
    border-radius: 8px;
    padding: 4px 10px;
    font-size: 12px;
    font-weight: 700;
}
.cyanide-match-pill.matched {
    color: #052812;
    background-color: rgba(52, 232, 118, 0.94);
    border-color: rgba(134, 239, 172, 0.98);
}

.cyanide-toolbar {
    border-radius: 12px;
    padding: 8px 12px;
    background-color: rgba(24, 28, 36, 0.95);
    border: 1px solid rgba(255, 255, 255, 0.10);
}

.cyanide-toolbar button,
.cyanide-toolbar togglebutton,
.cyanide-toolbar menubutton > button {
    min-height: 32px;
    padding: 4px 8px;
    font-size: 13px;
}

.cyanide-freeze-active:checked {
    background-color: rgba(245, 158, 11, 0.28);
    color: #fde68a;
    border-color: rgba(251, 191, 36, 0.55);
}

/* --- Right Panel: Virtual Camera Monitor & Controls --- */
.cyanide-monitor-card {
    border-radius: 12px;
    padding: 12px;
    background-color: rgba(24, 28, 36, 0.95);
    border: 1px solid rgba(255, 255, 255, 0.10);
}

.cyanide-monitor-screen {
    background-color: #090b10;
    border-radius: 8px;
    border: 2px solid rgba(255, 255, 255, 0.12);
}
.cyanide-monitor-screen.live {
    border-color: rgba(74, 222, 128, 0.55);
}
.cyanide-monitor-screen.loop {
    border-color: rgba(192, 132, 252, 0.58);
}
.cyanide-monitor-screen.custom {
    border-color: rgba(56, 189, 248, 0.58);
}
.cyanide-monitor-screen.recording {
    border-color: rgba(248, 113, 113, 0.85);
}

.cyanide-source-btn {
    min-height: 36px;
    font-size: 13px;
    font-weight: 700;
}
.cyanide-source-btn.live:checked {
    background-color: rgba(34, 197, 94, 0.24);
    color: #86efac;
    border-color: rgba(74, 222, 128, 0.55);
}
.cyanide-source-btn.loop:checked {
    background-color: rgba(168, 85, 247, 0.28);
    color: #e9d5ff;
    border-color: rgba(192, 132, 252, 0.58);
}

/* --- Tabs --- */
.cyanide-tab-strip {
    padding: 4px;
    border-radius: 10px;
    background-color: rgba(255, 255, 255, 0.06);
}
.cyanide-tab-btn {
    border-radius: 8px;
    min-height: 32px;
    padding: 4px 8px;
    font-size: 13px;
    font-weight: 600;
    color: #cbd5e1;
    border: none;
    background: transparent;
    box-shadow: none;
}
.cyanide-tab-btn:hover {
    color: #f3f4f6;
    background-color: rgba(255, 255, 255, 0.06);
}
.cyanide-tab-btn:checked {
    background-color: rgba(56, 189, 248, 0.20);
    color: #f8fafc;
}

/* --- VU Meters --- */
.cyanide-vu-card {
    border-radius: 12px;
    padding: 12px;
}
.cyanide-vu-bar trough {
    min-height: 8px;
    border-radius: 999px;
}
.cyanide-vu-bar block.filled {
    border-radius: 999px;
}

/* --- Footer Status Strip --- */
.cyanide-footer {
    border-top: 1px solid rgba(255, 255, 255, 0.10);
    padding: 6px 12px;
    background-color: rgba(16, 19, 25, 0.96);
}
.cyanide-kbd-chip {
    background-color: rgba(255, 255, 255, 0.10);
    color: #f3f4f6;
    border: 1px solid rgba(255, 255, 255, 0.20);
    border-radius: 6px;
    padding: 2px 8px;
    font-size: 12px;
    font-weight: 700;
}
.cyanide-lag-chip {
    background-color: rgba(245, 158, 11, 0.22);
    color: #fde68a;
    border: 1px solid rgba(251, 191, 36, 0.45);
    border-radius: 8px;
    padding: 2px 8px;
    font-size: 12px;
    font-weight: 700;
}
.cyanide-diag-btn {
    min-height: 28px;
    padding: 2px 10px;
    font-size: 12px;
    font-weight: 600;
}
.cyanide-diag-btn.has-warning {
    background-color: rgba(245, 158, 11, 0.22);
    color: #fde68a;
    border: 1px solid rgba(251, 191, 36, 0.50);
}

/* --- Floating Overlay Window --- */
window.cyanide-overlay {
    background-color: transparent;
}
.cyanide-overlay-frame {
    background-color: rgba(11, 13, 18, 0.90);
    border-radius: 12px;
    border: 1px solid rgba(255, 255, 255, 0.18);
}
.cyanide-overlay-hud {
    background-color: rgba(12, 14, 20, 0.88);
    border: 1px solid rgba(255, 255, 255, 0.16);
    border-radius: 10px;
    padding: 4px 8px;
}
.cyanide-overlay-mode-live {
    background-color: rgba(34, 197, 94, 0.90);
    color: #052812;
    font-weight: 700;
}
.cyanide-overlay-mode-loop {
    background-color: rgba(168, 85, 247, 0.92);
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
