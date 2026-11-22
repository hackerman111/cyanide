use std::cell::{Cell, RefCell};

use gtk4::gdk;
use gtk4::glib;
use gtk4::graphene;
use gtk4::gsk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::config::{CompareConfig, CompareMode, CompareTarget, LayerTransform};

pub type Rect4 = (f32, f32, f32, f32);

pub fn fit_aspect_rect(bounds: (f32, f32), aspect: f32) -> Rect4 {
    let (bw, bh) = bounds;
    if bw <= 0.0 || bh <= 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let ratio = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        16.0 / 9.0
    };
    let box_ratio = bw / bh;
    if box_ratio > ratio {
        let h = bh;
        let w = h * ratio;
        ((bw - w) * 0.5, 0.0, w, h)
    } else {
        let w = bw;
        let h = w / ratio;
        (0.0, (bh - h) * 0.5, w, h)
    }
}

pub fn split_clip_rects(w: f32, h: f32, ratio: f32) -> (Rect4, Rect4) {
    split_clip_rects_oriented(w, h, ratio, true)
}

pub fn split_clip_rects_oriented(w: f32, h: f32, ratio: f32, vertical: bool) -> (Rect4, Rect4) {
    let clamped = ratio.clamp(0.05, 0.95);
    if vertical {
        let split_x = w * clamped;
        ((0.0, 0.0, split_x, h), (split_x, 0.0, w - split_x, h))
    } else {
        let split_y = h * clamped;
        ((0.0, 0.0, w, split_y), (0.0, split_y, w, h - split_y))
    }
}

/// Maps frame similarity `[0.0, 1.0]` to green glow intensity `[0.0, 1.0]`.
pub fn match_green_strength(similarity: f32) -> f32 {
    if !similarity.is_finite() {
        return 0.0;
    }
    ((similarity - 0.60) / 0.25).clamp(0.0, 1.0)
}

/// Returns `(x, y, width, height)` 1px line rectangles for active grid, center axis, and custom guides.
pub fn grid_segments(w: f32, h: f32, cfg: &CompareConfig) -> Vec<Rect4> {
    let mut segs = Vec::with_capacity(8);
    if w <= 0.0 || h <= 0.0 {
        return segs;
    }
    if cfg.show_grid {
        for i in 1..=2 {
            let frac = (i as f32) / 3.0;
            segs.push((w * frac, 0.0, 1.0, h));
            segs.push((0.0, h * frac, w, 1.0));
        }
    }
    if cfg.show_center {
        segs.push((w * 0.5, 0.0, 1.0, h));
        segs.push((0.0, h * 0.5, w, 1.0));
    }
    if cfg.show_guides {
        let gx = w * (cfg.guide_x as f32).clamp(0.0, 1.0);
        let gy = h * (cfg.guide_y as f32).clamp(0.0, 1.0);
        segs.push((gx, 0.0, 1.5, h));
        segs.push((0.0, gy, w, 1.5));
    }
    segs
}

struct BoundPaintable {
    paintable: gdk::Paintable,
    contents_sig: glib::SignalHandlerId,
    size_sig: glib::SignalHandlerId,
}

impl BoundPaintable {
    fn disconnect(self) {
        self.paintable.disconnect(self.contents_sig);
        self.paintable.disconnect(self.size_sig);
    }
}

type ConfigCallback = Box<dyn Fn(CompareConfig)>;

mod imp {
    use super::*;

    pub struct CompareView {
        pub(super) loop_bound: RefCell<Option<BoundPaintable>>,
        pub(super) live_bound: RefCell<Option<BoundPaintable>>,
        pub(super) frozen_frame: RefCell<Option<gdk::Paintable>>,
        pub(super) cfg: Cell<CompareConfig>,
        pub(super) interactive: Cell<bool>,
        pub(super) highlight_match: Cell<bool>,
        pub(super) similarity: Cell<f32>,
        pub(super) drag_start_x: Cell<f64>,
        pub(super) drag_start_y: Cell<f64>,
        pub(super) dragging_split: Cell<bool>,
        pub(super) pan_start_x: Cell<f64>,
        pub(super) pan_start_y: Cell<f64>,
        pub(super) on_config_changed: RefCell<Option<ConfigCallback>>,
    }

    impl Default for CompareView {
        fn default() -> Self {
            Self {
                loop_bound: RefCell::new(None),
                live_bound: RefCell::new(None),
                frozen_frame: RefCell::new(None),
                cfg: Cell::new(CompareConfig::default()),
                interactive: Cell::new(true),
                highlight_match: Cell::new(false),
                similarity: Cell::new(0.0),
                drag_start_x: Cell::new(0.0),
                drag_start_y: Cell::new(0.0),
                dragging_split: Cell::new(false),
                pan_start_x: Cell::new(0.0),
                pan_start_y: Cell::new(0.0),
                on_config_changed: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CompareView {
        const NAME: &'static str = "CyanideCompareView";
        type Type = super::CompareView;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for CompareView {
        fn constructed(&self) {
            self.parent_constructed();
            let widget = self.obj();
            widget.set_hexpand(true);
            widget.set_vexpand(true);
            widget.set_overflow(gtk4::Overflow::Hidden);
            widget.set_focusable(true);

            // Drag: moves split divider in Split mode (near divider or default zoom), or pans active layer.
            let drag = gtk4::GestureDrag::new();
            let weak_begin = widget.downgrade();
            drag.connect_drag_begin(move |_, start_x, start_y| {
                let Some(w) = weak_begin.upgrade() else {
                    return;
                };
                if !w.imp().interactive.get() {
                    return;
                }
                w.grab_focus();
                let cfg = w.imp().cfg.get();
                let (az, apx, apy, _) = cfg.active_transform();
                w.imp().drag_start_x.set(start_x);
                w.imp().drag_start_y.set(start_y);
                w.imp().pan_start_x.set(apx);
                w.imp().pan_start_y.set(apy);

                let hit_split = if cfg.mode == CompareMode::Split {
                    let vw = w.width() as f64;
                    let vh = w.height() as f64;
                    let dist = if cfg.split_vertical {
                        (start_x - vw * cfg.split_pos).abs()
                    } else {
                        (start_y - vh * cfg.split_pos).abs()
                    };
                    dist <= 28.0 || (cfg.target == CompareTarget::Both && az <= 1.01)
                } else {
                    false
                };
                w.imp().dragging_split.set(hit_split);
                if hit_split {
                    w.update_split_from_pos(start_x, start_y);
                }
            });
            let weak_update = widget.downgrade();
            drag.connect_drag_update(move |_, offset_x, offset_y| {
                let Some(w) = weak_update.upgrade() else {
                    return;
                };
                if !w.imp().interactive.get() {
                    return;
                }
                let cfg = w.imp().cfg.get();
                if w.imp().dragging_split.get() {
                    let x = w.imp().drag_start_x.get() + offset_x;
                    let y = w.imp().drag_start_y.get() + offset_y;
                    w.update_split_from_pos(x, y);
                } else {
                    let (az, _, _, _) = cfg.active_transform();
                    if cfg.target != CompareTarget::Both || az > 1.01 {
                        w.update_pan_from_offset(offset_x, offset_y);
                    }
                }
            });
            widget.add_controller(drag);

            // Mouse wheel: zoom active layer, or Ctrl+Scroll adjusts opacity in Overlay mode.
            let scroll =
                gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
            let weak_scroll = widget.downgrade();
            scroll.connect_scroll(move |ctrl, _, dy| {
                let Some(w) = weak_scroll.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if !w.imp().interactive.get() {
                    return glib::Propagation::Proceed;
                }
                let mut cfg = w.imp().cfg.get();
                let mods = ctrl.current_event_state();
                if mods.contains(gdk::ModifierType::CONTROL_MASK)
                    && cfg.mode == CompareMode::Overlay
                {
                    let delta = if dy < 0.0 { 0.05 } else { -0.05 };
                    let next_op = (cfg.opacity + delta).clamp(0.0, 1.0);
                    if (next_op - cfg.opacity).abs() < 0.001 {
                        return glib::Propagation::Stop;
                    }
                    cfg.opacity = next_op;
                    w.apply_interactive_config(cfg);
                    return glib::Propagation::Stop;
                }

                let step = if dy < 0.0 { 0.12 } else { -0.12 };
                let (zoom, mut px, mut py, mirror) = cfg.active_transform();
                let (min_z, max_z) = if cfg.target == CompareTarget::Both {
                    (1.0, 3.0)
                } else {
                    (0.5, 3.0)
                };
                let next_zoom = (zoom + step).clamp(min_z, max_z);
                if (next_zoom - zoom).abs() < 0.001 {
                    return glib::Propagation::Stop;
                }
                let final_zoom = if cfg.target == CompareTarget::Both && next_zoom <= 1.001 {
                    px = 0.0;
                    py = 0.0;
                    1.0
                } else {
                    next_zoom
                };
                cfg.set_active_transform(final_zoom, px, py, mirror);
                w.apply_interactive_config(cfg);
                glib::Propagation::Stop
            });
            widget.add_controller(scroll);

            // Double-click resets active layer transform.
            let click = gtk4::GestureClick::new();
            let weak_click = widget.downgrade();
            click.connect_pressed(move |_, n_press, _, _| {
                if n_press != 2 {
                    return;
                }
                let Some(w) = weak_click.upgrade() else {
                    return;
                };
                if !w.imp().interactive.get() {
                    return;
                }
                let mut cfg = w.imp().cfg.get();
                let (zoom, px, py, mirror) = cfg.active_transform();
                if (zoom - 1.0).abs() > 0.001 || px.abs() > 0.001 || py.abs() > 0.001 || mirror {
                    cfg.set_active_transform(1.0, 0.0, 0.0, false);
                    w.apply_interactive_config(cfg);
                }
            });
            widget.add_controller(click);

            // Keyboard panning/zooming when canvas has focus.
            let key = gtk4::EventControllerKey::new();
            let weak_key = widget.downgrade();
            key.connect_key_pressed(move |_, keyval, _, state| {
                let Some(w) = weak_key.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if !w.imp().interactive.get() {
                    return glib::Propagation::Proceed;
                }
                let mut cfg = w.imp().cfg.get();
                let step = if state.contains(gdk::ModifierType::SHIFT_MASK) {
                    0.06
                } else {
                    0.02
                };
                let (zoom, mut px, mut py, mirror) = cfg.active_transform();
                match keyval {
                    gdk::Key::Left => px = (px - step).clamp(-1.0, 1.0),
                    gdk::Key::Right => px = (px + step).clamp(-1.0, 1.0),
                    gdk::Key::Up => {
                        if state.contains(gdk::ModifierType::CONTROL_MASK)
                            && cfg.mode == CompareMode::Overlay
                        {
                            cfg.opacity = (cfg.opacity + 0.05).clamp(0.0, 1.0);
                            w.apply_interactive_config(cfg);
                            return glib::Propagation::Stop;
                        }
                        py = (py - step).clamp(-1.0, 1.0);
                    }
                    gdk::Key::Down => {
                        if state.contains(gdk::ModifierType::CONTROL_MASK)
                            && cfg.mode == CompareMode::Overlay
                        {
                            cfg.opacity = (cfg.opacity - 0.05).clamp(0.0, 1.0);
                            w.apply_interactive_config(cfg);
                            return glib::Propagation::Stop;
                        }
                        py = (py + step).clamp(-1.0, 1.0);
                    }
                    gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => {
                        let nz = (zoom + 0.1).clamp(0.5, 3.0);
                        cfg.set_active_transform(nz, px, py, mirror);
                        w.apply_interactive_config(cfg);
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::minus | gdk::Key::KP_Subtract => {
                        let min_z = if cfg.target == CompareTarget::Both {
                            1.0
                        } else {
                            0.5
                        };
                        let nz = (zoom - 0.1).clamp(min_z, 3.0);
                        cfg.set_active_transform(nz, px, py, mirror);
                        w.apply_interactive_config(cfg);
                        return glib::Propagation::Stop;
                    }
                    _ => return glib::Propagation::Proceed,
                }
                cfg.set_active_transform(zoom, px, py, mirror);
                w.apply_interactive_config(cfg);
                glib::Propagation::Stop
            });
            widget.add_controller(key);
        }

        fn dispose(&self) {
            if let Some(bound) = self.loop_bound.borrow_mut().take() {
                bound.disconnect();
            }
            if let Some(bound) = self.live_bound.borrow_mut().take() {
                bound.disconnect();
            }
            *self.frozen_frame.borrow_mut() = None;
        }
    }

    impl WidgetImpl for CompareView {
        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let widget = self.obj();
            let w = widget.width() as f32;
            let h = widget.height() as f32;
            if w <= 1.0 || h <= 1.0 {
                return;
            }

            let bounds = graphene::Rect::new(0.0, 0.0, w, h);
            snapshot.push_clip(&bounds);
            snapshot.append_color(&gdk::RGBA::new(0.043, 0.051, 0.071, 1.0), &bounds);

            let cfg = self.cfg.get();
            let frozen = self.frozen_frame.borrow().clone();
            let loop_p = frozen.or_else(|| {
                self.loop_bound
                    .borrow()
                    .as_ref()
                    .map(|b| b.paintable.clone())
            });
            let live_p = self
                .live_bound
                .borrow()
                .as_ref()
                .map(|b| b.paintable.clone());

            match cfg.mode {
                CompareMode::Overlay => {
                    if let Some(ref lp) = loop_p {
                        draw_transformed(snapshot, lp, (0.0, 0.0, w, h), &cfg, cfg.loop_transform);
                    }
                    if let Some(ref lv) = live_p {
                        snapshot.push_opacity(cfg.opacity.clamp(0.0, 1.0));
                        draw_transformed(snapshot, lv, (0.0, 0.0, w, h), &cfg, cfg.live_transform);
                        snapshot.pop();
                    }
                }
                CompareMode::SideBySide => {
                    let half = w * 0.5;
                    if let Some(ref lp) = loop_p {
                        draw_transformed(
                            snapshot,
                            lp,
                            (0.0, 0.0, half, h),
                            &cfg,
                            cfg.loop_transform,
                        );
                    }
                    if let Some(ref lv) = live_p {
                        draw_transformed(
                            snapshot,
                            lv,
                            (half, 0.0, w - half, h),
                            &cfg,
                            cfg.live_transform,
                        );
                    }
                    let divider = graphene::Rect::new(half - 1.0, 0.0, 2.0, h);
                    snapshot.append_color(&gdk::RGBA::new(1.0, 1.0, 1.0, 0.22), &divider);
                }
                CompareMode::Split => {
                    let (first, second) =
                        split_clip_rects_oriented(w, h, cfg.split_pos as f32, cfg.split_vertical);
                    if let Some(ref lp) = loop_p {
                        let clip = graphene::Rect::new(first.0, first.1, first.2, first.3);
                        snapshot.push_clip(&clip);
                        draw_transformed(snapshot, lp, (0.0, 0.0, w, h), &cfg, cfg.loop_transform);
                        snapshot.pop();
                    }
                    if let Some(ref lv) = live_p {
                        let clip = graphene::Rect::new(second.0, second.1, second.2, second.3);
                        snapshot.push_clip(&clip);
                        draw_transformed(snapshot, lv, (0.0, 0.0, w, h), &cfg, cfg.live_transform);
                        snapshot.pop();
                    }
                    let split_pos_px = if cfg.split_vertical { first.2 } else { first.3 };
                    draw_split_handle_oriented(snapshot, w, h, split_pos_px, cfg.split_vertical);
                }
            }

            let green = if self.highlight_match.get() {
                match_green_strength(self.similarity.get())
            } else {
                0.0
            };
            if green > 0.0 {
                snapshot.append_color(&gdk::RGBA::new(0.08, 0.88, 0.35, 0.24 * green), &bounds);
            }

            draw_overlays(snapshot, w, h, &cfg, green);

            if green > 0.0 {
                let border = gdk::RGBA::new(0.18, 0.94, 0.46, 0.92 * green);
                let t = 3.5f32;
                snapshot.append_color(&border, &graphene::Rect::new(0.0, 0.0, w, t));
                snapshot.append_color(&border, &graphene::Rect::new(0.0, h - t, w, t));
                snapshot.append_color(
                    &border,
                    &graphene::Rect::new(0.0, t, t, (h - 2.0 * t).max(0.0)),
                );
                snapshot.append_color(
                    &border,
                    &graphene::Rect::new(w - t, t, t, (h - 2.0 * t).max(0.0)),
                );
            }
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct CompareView(ObjectSubclass<imp::CompareView>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Default for CompareView {
    fn default() -> Self {
        Self::new()
    }
}

impl CompareView {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn set_interactive(&self, interactive: bool) {
        self.imp().interactive.set(interactive);
    }

    pub fn set_paintables(&self, loop_p: Option<gdk::Paintable>, live_p: Option<gdk::Paintable>) {
        self.rebind_slot(&self.imp().loop_bound, loop_p);
        self.rebind_slot(&self.imp().live_bound, live_p);
        self.queue_draw();
    }

    pub fn set_config(&self, cfg: CompareConfig) {
        self.imp().cfg.set(cfg);
        self.queue_draw();
    }

    pub fn set_match_highlight(&self, enabled: bool) {
        self.imp().highlight_match.set(enabled);
        self.queue_draw();
    }

    pub fn set_similarity(&self, score: f32) {
        let clamped = if score.is_finite() {
            score.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if (self.imp().similarity.get() - clamped).abs() >= 0.005 {
            self.imp().similarity.set(clamped);
            self.queue_draw();
        }
    }

    pub fn set_frozen(&self, frozen: bool) {
        if frozen {
            let captured = self
                .imp()
                .loop_bound
                .borrow()
                .as_ref()
                .map(|b| b.paintable.current_image());
            *self.imp().frozen_frame.borrow_mut() = captured;
        } else {
            *self.imp().frozen_frame.borrow_mut() = None;
        }
        self.queue_draw();
    }

    pub fn is_frozen(&self) -> bool {
        self.imp().frozen_frame.borrow().is_some()
    }

    pub fn connect_config_changed(&self, f: impl Fn(CompareConfig) + 'static) {
        *self.imp().on_config_changed.borrow_mut() = Some(Box::new(f));
    }

    fn apply_interactive_config(&self, cfg: CompareConfig) {
        self.imp().cfg.set(cfg);
        self.queue_draw();
        if let Some(cb) = self.imp().on_config_changed.borrow().as_ref() {
            cb(cfg);
        }
    }

    fn update_split_from_pos(&self, x: f64, y: f64) {
        let mut cfg = self.imp().cfg.get();
        let ratio = if cfg.split_vertical {
            let w = self.width() as f64;
            if w <= 1.0 {
                return;
            }
            (x / w).clamp(0.05, 0.95)
        } else {
            let h = self.height() as f64;
            if h <= 1.0 {
                return;
            }
            (y / h).clamp(0.05, 0.95)
        };
        cfg.split_pos = ratio;
        self.apply_interactive_config(cfg);
    }

    fn update_pan_from_offset(&self, offset_x: f64, offset_y: f64) {
        let w = (self.width() as f64).max(1.0);
        let h = (self.height() as f64).max(1.0);
        let mut cfg = self.imp().cfg.get();
        let (zoom, _, _, mirror) = cfg.active_transform();
        let dx = (offset_x / (w * 0.5)).clamp(-2.0, 2.0);
        let dy = (offset_y / (h * 0.5)).clamp(-2.0, 2.0);
        let px = (self.imp().pan_start_x.get() + dx).clamp(-1.0, 1.0);
        let py = (self.imp().pan_start_y.get() + dy).clamp(-1.0, 1.0);
        cfg.set_active_transform(zoom, px, py, mirror);
        self.apply_interactive_config(cfg);
    }

    fn rebind_slot(
        &self,
        slot: &RefCell<Option<BoundPaintable>>,
        paintable: Option<gdk::Paintable>,
    ) {
        if let Some(prev) = slot.borrow_mut().take() {
            prev.disconnect();
        }
        let Some(p) = paintable else {
            return;
        };
        let weak_c = self.downgrade();
        let contents_sig = p.connect_invalidate_contents(move |_| {
            if let Some(w) = weak_c.upgrade() {
                w.queue_draw();
            }
        });
        let weak_s = self.downgrade();
        let size_sig = p.connect_invalidate_size(move |_| {
            if let Some(w) = weak_s.upgrade() {
                w.queue_draw();
            }
        });
        *slot.borrow_mut() = Some(BoundPaintable {
            paintable: p,
            contents_sig,
            size_sig,
        });
    }
}

fn draw_split_handle_oriented(snapshot: &gtk4::Snapshot, w: f32, h: f32, pos: f32, vertical: bool) {
    let shadow_col = gdk::RGBA::new(0.0, 0.0, 0.0, 0.45);
    let accent_col = gdk::RGBA::new(0.22, 0.74, 0.97, 0.96);
    let bg_col = gdk::RGBA::new(0.08, 0.11, 0.16, 0.94);
    let grip_col = gdk::RGBA::new(0.04, 0.10, 0.16, 0.80);

    if vertical {
        snapshot.append_color(&shadow_col, &graphene::Rect::new(pos - 2.0, 0.0, 4.0, h));
        snapshot.append_color(&accent_col, &graphene::Rect::new(pos - 1.0, 0.0, 2.0, h));

        let pill_rect = graphene::Rect::new(pos - 8.0, h * 0.5 - 24.0, 16.0, 48.0);
        snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(pill_rect, 8.0));
        snapshot.append_color(&bg_col, &pill_rect);
        let inner = graphene::Rect::new(pos - 6.0, h * 0.5 - 22.0, 12.0, 44.0);
        snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(inner, 6.0));
        snapshot.append_color(&accent_col, &inner);
        for dy in [-6.0f32, 0.0, 6.0] {
            snapshot.append_color(
                &grip_col,
                &graphene::Rect::new(pos - 3.0, h * 0.5 + dy - 0.75, 6.0, 1.5),
            );
        }
        snapshot.pop();
        snapshot.pop();
    } else {
        snapshot.append_color(&shadow_col, &graphene::Rect::new(0.0, pos - 2.0, w, 4.0));
        snapshot.append_color(&accent_col, &graphene::Rect::new(0.0, pos - 1.0, w, 2.0));

        let pill_rect = graphene::Rect::new(w * 0.5 - 24.0, pos - 8.0, 48.0, 16.0);
        snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(pill_rect, 8.0));
        snapshot.append_color(&bg_col, &pill_rect);
        let inner = graphene::Rect::new(w * 0.5 - 22.0, pos - 6.0, 44.0, 12.0);
        snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(inner, 6.0));
        snapshot.append_color(&accent_col, &inner);
        for dx in [-6.0f32, 0.0, 6.0] {
            snapshot.append_color(
                &grip_col,
                &graphene::Rect::new(w * 0.5 + dx - 0.75, pos - 3.0, 1.5, 6.0),
            );
        }
        snapshot.pop();
        snapshot.pop();
    }
}

fn draw_transformed(
    snapshot: &gtk4::Snapshot,
    paintable: &gdk::Paintable,
    region: (f32, f32, f32, f32),
    cfg: &CompareConfig,
    layer: LayerTransform,
) {
    let (rx, ry, rw, rh) = region;
    if rw <= 1.0 || rh <= 1.0 {
        return;
    }

    let aspect = paintable.intrinsic_aspect_ratio() as f32;
    let (ox, oy, fw, fh) = fit_aspect_rect((rw, rh), aspect);
    let fx = rx + ox;
    let fy = ry + oy;
    let view_zoom = (cfg.zoom as f32).clamp(1.0, 3.0);
    let view_scale_x = if cfg.mirror { -view_zoom } else { view_zoom };
    let view_pan_dx = (cfg.pan_x as f32).clamp(-1.0, 1.0) * fw * 0.5;
    let view_pan_dy = (cfg.pan_y as f32).clamp(-1.0, 1.0) * fh * 0.5;

    let layer_zoom = (layer.zoom as f32).clamp(0.5, 3.0);
    let layer_scale_x = if layer.mirror {
        -layer_zoom
    } else {
        layer_zoom
    };
    let layer_pan_dx = (layer.pan_x as f32).clamp(-1.0, 1.0) * fw * 0.5;
    let layer_pan_dy = (layer.pan_y as f32).clamp(-1.0, 1.0) * fh * 0.5;

    let clip = graphene::Rect::new(rx, ry, rw, rh);
    snapshot.save();
    snapshot.push_clip(&clip);
    snapshot.translate(&graphene::Point::new(
        fx + fw * 0.5 + view_pan_dx,
        fy + fh * 0.5 + view_pan_dy,
    ));
    snapshot.scale(view_scale_x, view_zoom);
    snapshot.translate(&graphene::Point::new(layer_pan_dx, layer_pan_dy));
    snapshot.scale(layer_scale_x, layer_zoom);
    snapshot.translate(&graphene::Point::new(-fw * 0.5, -fh * 0.5));
    paintable.snapshot(snapshot, fw as f64, fh as f64);
    snapshot.pop();
    snapshot.restore();
}

fn draw_overlays(snapshot: &gtk4::Snapshot, w: f32, h: f32, cfg: &CompareConfig, green: f32) {
    let segs = grid_segments(w, h, cfg);
    let mut idx = 0usize;
    if cfg.show_grid {
        let color = gdk::RGBA::new(
            1.0 - 0.80 * green,
            1.0,
            1.0 - 0.55 * green,
            0.35 + 0.45 * green,
        );
        for &(x, y, sw, sh) in &segs[idx..idx + 4] {
            snapshot.append_color(&color, &graphene::Rect::new(x, y, sw, sh));
        }
        idx += 4;
    }
    if cfg.show_center {
        let color = gdk::RGBA::new(0.22, 0.84, 1.0, 0.68);
        for &(x, y, sw, sh) in &segs[idx..idx + 2] {
            snapshot.append_color(&color, &graphene::Rect::new(x, y, sw, sh));
        }
        idx += 2;
    }
    if cfg.show_guides {
        let color = gdk::RGBA::new(1.0, 0.76, 0.18, 0.85);
        for &(x, y, sw, sh) in &segs[idx..idx + 2] {
            snapshot.append_color(&color, &graphene::Rect::new(x, y, sw, sh));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_clip_rects_partition_bounds_cleanly() {
        let (left, right) = split_clip_rects(1000.0, 500.0, 0.35);
        assert_eq!(left, (0.0, 0.0, 350.0, 500.0));
        assert_eq!(right, (350.0, 0.0, 650.0, 500.0));

        let (top, bottom) = split_clip_rects_oriented(1000.0, 500.0, 0.40, false);
        assert_eq!(top, (0.0, 0.0, 1000.0, 200.0));
        assert_eq!(bottom, (0.0, 200.0, 1000.0, 300.0));
    }

    #[test]
    fn compare_config_tracks_independent_layer_transforms() {
        let mut cfg = CompareConfig {
            target: CompareTarget::Live,
            ..Default::default()
        };
        cfg.set_active_transform(1.4, 0.25, -0.1, true);
        assert_eq!(cfg.active_transform(), (1.4, 0.25, -0.1, true));
        assert!(cfg.loop_transform.is_identity());
        assert_eq!(cfg.zoom, 1.0);

        cfg.target = CompareTarget::Both;
        assert_eq!(cfg.active_transform(), (1.0, 0.0, 0.0, false));

        cfg.reset_transforms();
        assert!(cfg.live_transform.is_identity());
    }

    #[test]
    fn grid_segments_reflect_toggles() {
        let cfg = CompareConfig {
            show_grid: true,
            show_center: true,
            show_guides: true,
            guide_x: 0.25,
            guide_y: 0.75,
            ..Default::default()
        };
        let segs = grid_segments(900.0, 600.0, &cfg);
        // 4 lines for 3x3 + 2 for center + 2 for custom guides = 8 segments
        assert_eq!(segs.len(), 8);
    }

    #[test]
    fn match_green_strength_ramps_above_threshold() {
        assert_eq!(match_green_strength(0.0), 0.0);
        assert_eq!(match_green_strength(0.59), 0.0);
        assert!((match_green_strength(0.725) - 0.5).abs() < 0.01);
        assert_eq!(match_green_strength(0.90), 1.0);
        assert_eq!(match_green_strength(f32::NAN), 0.0);
    }
}
