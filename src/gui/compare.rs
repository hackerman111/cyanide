use std::cell::{Cell, RefCell};

use gtk4::gdk;
use gtk4::glib;
use gtk4::graphene;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::config::{CompareConfig, CompareMode};

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
    let clamped = ratio.clamp(0.05, 0.95);
    let split_x = w * clamped;
    ((0.0, 0.0, split_x, h), (split_x, 0.0, w - split_x, h))
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

type SplitCallback = Box<dyn Fn(f64)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CompareView {
        pub(super) loop_bound: RefCell<Option<BoundPaintable>>,
        pub(super) live_bound: RefCell<Option<BoundPaintable>>,
        pub(super) frozen_frame: RefCell<Option<gdk::Paintable>>,
        pub(super) cfg: Cell<CompareConfig>,
        pub(super) drag_start_x: Cell<f64>,
        pub(super) on_split_changed: RefCell<Option<SplitCallback>>,
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

            let drag = gtk4::GestureDrag::new();
            let weak_begin = widget.downgrade();
            drag.connect_drag_begin(move |_, start_x, _| {
                let Some(w) = weak_begin.upgrade() else {
                    return;
                };
                if w.imp().cfg.get().mode != CompareMode::Split {
                    return;
                }
                w.imp().drag_start_x.set(start_x);
                w.update_split_from_x(start_x);
            });
            let weak_update = widget.downgrade();
            drag.connect_drag_update(move |_, offset_x, _| {
                let Some(w) = weak_update.upgrade() else {
                    return;
                };
                if w.imp().cfg.get().mode != CompareMode::Split {
                    return;
                }
                let x = w.imp().drag_start_x.get() + offset_x;
                w.update_split_from_x(x);
            });
            widget.add_controller(drag);
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
            snapshot.append_color(&gdk::RGBA::new(0.09, 0.09, 0.11, 1.0), &bounds);

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
                        draw_transformed(snapshot, lp, (0.0, 0.0, w, h), &cfg);
                    }
                    if let Some(ref lv) = live_p {
                        snapshot.push_opacity(cfg.opacity.clamp(0.0, 1.0));
                        draw_transformed(snapshot, lv, (0.0, 0.0, w, h), &cfg);
                        snapshot.pop();
                    }
                }
                CompareMode::SideBySide => {
                    let half = w * 0.5;
                    if let Some(ref lp) = loop_p {
                        draw_transformed(snapshot, lp, (0.0, 0.0, half, h), &cfg);
                    }
                    if let Some(ref lv) = live_p {
                        draw_transformed(snapshot, lv, (half, 0.0, w - half, h), &cfg);
                    }
                    let divider = graphene::Rect::new(half - 1.0, 0.0, 2.0, h);
                    snapshot.append_color(&gdk::RGBA::new(1.0, 1.0, 1.0, 0.35), &divider);
                }
                CompareMode::Split => {
                    let (left, right) = split_clip_rects(w, h, cfg.split_pos as f32);
                    if let Some(ref lp) = loop_p {
                        let clip = graphene::Rect::new(left.0, left.1, left.2, left.3);
                        snapshot.push_clip(&clip);
                        draw_transformed(snapshot, lp, (0.0, 0.0, w, h), &cfg);
                        snapshot.pop();
                    }
                    if let Some(ref lv) = live_p {
                        let clip = graphene::Rect::new(right.0, right.1, right.2, right.3);
                        snapshot.push_clip(&clip);
                        draw_transformed(snapshot, lv, (0.0, 0.0, w, h), &cfg);
                        snapshot.pop();
                    }
                    let line = graphene::Rect::new(left.2 - 1.0, 0.0, 2.0, h);
                    snapshot.append_color(&gdk::RGBA::new(0.25, 0.78, 1.0, 0.95), &line);
                    let handle = graphene::Rect::new(left.2 - 5.0, h * 0.5 - 20.0, 10.0, 40.0);
                    snapshot.append_color(&gdk::RGBA::new(0.25, 0.78, 1.0, 0.95), &handle);
                }
            }

            draw_overlays(snapshot, w, h, &cfg);
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

    pub fn set_paintables(&self, loop_p: Option<gdk::Paintable>, live_p: Option<gdk::Paintable>) {
        self.rebind_slot(&self.imp().loop_bound, loop_p);
        self.rebind_slot(&self.imp().live_bound, live_p);
        self.queue_draw();
    }

    pub fn set_config(&self, cfg: CompareConfig) {
        self.imp().cfg.set(cfg);
        self.queue_draw();
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

    pub fn connect_split_changed(&self, f: impl Fn(f64) + 'static) {
        *self.imp().on_split_changed.borrow_mut() = Some(Box::new(f));
    }

    fn update_split_from_x(&self, x: f64) {
        let w = self.width() as f64;
        if w <= 1.0 {
            return;
        }
        let ratio = (x / w).clamp(0.05, 0.95);
        let mut cfg = self.imp().cfg.get();
        cfg.split_pos = ratio;
        self.imp().cfg.set(cfg);
        self.queue_draw();
        if let Some(cb) = self.imp().on_split_changed.borrow().as_ref() {
            cb(ratio);
        }
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

fn draw_transformed(
    snapshot: &gtk4::Snapshot,
    paintable: &gdk::Paintable,
    region: (f32, f32, f32, f32),
    cfg: &CompareConfig,
) {
    let (rx, ry, rw, rh) = region;
    if rw <= 1.0 || rh <= 1.0 {
        return;
    }

    let aspect = paintable.intrinsic_aspect_ratio() as f32;
    let (ox, oy, fw, fh) = fit_aspect_rect((rw, rh), aspect);
    let fx = rx + ox;
    let fy = ry + oy;
    let zoom = (cfg.zoom as f32).clamp(1.0, 3.0);
    let scale_x = if cfg.mirror { -zoom } else { zoom };
    let pan_dx = (cfg.pan_x as f32).clamp(-1.0, 1.0) * fw * 0.5;
    let pan_dy = (cfg.pan_y as f32).clamp(-1.0, 1.0) * fh * 0.5;

    let clip = graphene::Rect::new(rx, ry, rw, rh);
    snapshot.save();
    snapshot.push_clip(&clip);
    snapshot.translate(&graphene::Point::new(
        fx + fw * 0.5 + pan_dx,
        fy + fh * 0.5 + pan_dy,
    ));
    snapshot.scale(scale_x, zoom);
    snapshot.translate(&graphene::Point::new(-fw * 0.5, -fh * 0.5));
    paintable.snapshot(snapshot, fw as f64, fh as f64);
    snapshot.pop();
    snapshot.restore();
}

fn draw_overlays(snapshot: &gtk4::Snapshot, w: f32, h: f32, cfg: &CompareConfig) {
    let segs = grid_segments(w, h, cfg);
    let mut idx = 0usize;
    if cfg.show_grid {
        let color = gdk::RGBA::new(1.0, 1.0, 1.0, 0.35);
        for &(x, y, sw, sh) in &segs[idx..idx + 4] {
            snapshot.append_color(&color, &graphene::Rect::new(x, y, sw, sh));
        }
        idx += 4;
    }
    if cfg.show_center {
        let color = gdk::RGBA::new(0.2, 0.85, 1.0, 0.7);
        for &(x, y, sw, sh) in &segs[idx..idx + 2] {
            snapshot.append_color(&color, &graphene::Rect::new(x, y, sw, sh));
        }
        idx += 2;
    }
    if cfg.show_guides {
        let color = gdk::RGBA::new(1.0, 0.75, 0.15, 0.85);
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
}
