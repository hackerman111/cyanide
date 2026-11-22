# Cyanide Studio UX/UI Redesign Implementation Plan

> **For Antigravity:** Implementing the approved UX/UI redesign across Stage 3 (Implementation) and Stage 4 (Verification).

**Goal:** Redesign Cyanide Studio's GTK4/libadwaita interface for unambiguous source control, resizable/persistent two-pane layout, enhanced multi-layer image alignment & fullscreen compare mode, unified visual system, and responsive 1366×768–2560×1440 ergonomics without breaking GStreamer/PipeWire logic.

**Architecture:** Extend `Config` with `UiLayoutConfig` and per-layer/oriented `CompareConfig` fields; expose `seek_loop_ms` and explicit `add-borders` in `MediaPipeline`; enhance `CompareView` with target-layer transforms, vertical/horizontal split, `Ctrl+scroll` opacity, and keyboard/drag controls; refactor `MainWindow`, `VideoWorkspace`, `ControlPanel`, and `APP_CSS` to eliminate duplicate controls and establish a consistent 4/8px, WCAG AA design system.

**Tech Stack:** Rust 2024, GTK 4.14+, libadwaita 1.5+, GStreamer 1.24+.

---

### Task 1: Config, Layout Persistence & Compare Data Model
- Modify `src/config.rs` and `config.example.toml`:
  - Add `UiLayoutConfig` (`window_width`, `window_height`, `split_position`, `sidebar_visible`) with sanitization bounds.
  - Add `CompareTarget` (`Both`, `Loop`, `Live`), `LayerTransform` (`zoom`, `pan_x`, `pan_y`, `mirror`), `split_vertical: bool`, `loop_transform`, and `live_transform` to `CompareConfig` with `#[serde(default)]` and sanitization.
  - Add unit tests for defaults, bounds clamping, and TOML roundtrip.

### Task 2: Pipeline MP4 Seek & Aspect Ratio Guard
- Modify `src/pipeline/video.rs` and `src/pipeline/mod.rs`:
  - Explicitly set `add-borders = true` on `loop_scale` and `live_scale`.
  - Expose `MediaPipeline::seek_loop_ms(&self, position_ms: u64) -> bool`.

### Task 3: `CompareView` Alignment & Interaction Upgrades
- Modify `src/gui/compare.rs`:
  - Compose view-level (`Both`) and per-layer (`Loop` / `Live`) transforms in `draw_transformed`.
  - Support vertical and horizontal split orientation (`split_clip_rects_oriented`, `draw_split_handle_oriented`).
  - Support `Ctrl+Scroll` opacity adjustment in `Overlay` mode, smart drag (split handle vs active target pan), and focusable keyboard navigation (`Arrow` keys, `+/-`).
  - Add unit tests for oriented split rects and composed layer transforms.

### Task 4: Design System (`src/gui/style.rs`)
- Update `APP_CSS` with consistent surface tokens, WCAG AA text contrast (>= 4.5:1), 12–16px typography scale, 4/8px spacing, 32–36px button hit targets, `:focus-visible` rings, and clear state colors (Live, Loop, Custom, Recording, Warning).

### Task 5: Left Workspace (`src/gui/workspace.rs`)
- Redesign `VideoWorkspace` with:
  - Non-obscuring top source header strip (`Запись: ...` / `Камера: ...`, optional similarity & freeze badges).
  - Fullscreen compare HUD mode (`Revealer` on hover + minimal mode/opacity badge).
  - 2-row compact studio dock: mode selector, inline `0%/50%/100%` opacity presets + slider, `Вертик./Горизонт.` split toggle, visual aids + freeze, target layer selector (`Оба / Запись / Камера`), zoom/mirror/reset + fine position popover, and MP4 timeline scrubber + seek-to-start + bookmark button.

### Task 6: Right Panel & Main Window (`src/gui/controls.rs`, `src/gui/mod.rs`, `src/gui/overlay.rs`, `src/main.rs`)
- Eliminate duplicate controls (`mic_switch` duplicate, `record_btn` duplicate, confusing `preset_*` vs `video_*` split).
- Restructure `MainWindow` with `HeaderBar` (device status pill, source pill, primary toggle CTA, record, overlay, fullscreen, sidebar toggle), `gtk4::Paned` persistent divider, `ControlPanel` (prominent virtual output monitor card + single source switcher + mic mute + 4 deduplicated tabs), and compact `Statusbar` with expandable Diagnostics popover.
- Wire keyboard shortcuts (`1/2/3`, `Space`, `R`, `F11`, `Esc`) and layout persistence on close/resize.

### Task 7: Verification
- Run `make check` (`cargo fmt --all -- --check`, `cargo check --workspace --all-targets`, `cargo test --workspace`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`).
- Run the GUI with `--test-sources` under Wayland/headless or inspect layout sizes (`1366×768`, `1920×1080`, `2560×1440`) and verify all 6 scenarios and interactive flows.
