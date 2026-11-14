//! A track with a knob, for the place in the film and for the volume.
//!
//! It is far taller than what it draws, so it is easy to grab, and a click
//! anywhere on it moves the knob there at once.

use std::cell::{Cell, OnceCell, RefCell};

use objc2::rc::Retained;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSEvent, NSLayoutConstraint, NSTrackingArea, NSTrackingAreaOptions,
    NSView,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize};

use super::theme;

/// A place on the slider that the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pointed {
    /// The value the slider would take if clicked there.
    pub value: f64,
    /// How far along the slider that is, in points from its left edge.
    pub x: f64,
}

/// Whether the viewer still has hold of the knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grip {
    Held,
    /// The knob was just let go, where it now is.
    Released,
}

#[derive(Default)]
pub struct Ivars {
    /// Between 0 and `limit`.
    value: Cell<f64>,
    limit: Cell<f64>,
    /// Up to where the track is drawn as loaded. Nothing below `value`.
    loaded: Cell<f64>,
    /// Values at which the track is notched.
    marks: RefCell<Vec<f64>>,
    hovered: Cell<bool>,
    dragging: Cell<bool>,
    on_change: OnceCell<Box<dyn Fn(f64, Grip)>>,
    on_point: OnceCell<Box<dyn Fn(Option<Pointed>)>>,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `Slider` does not
    // implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustPlayerSlider"]
    #[ivars = Ivars]
    pub struct Slider;

    impl Slider {
        #[unsafe(method(drawRect:))]
        fn draw(&self, _dirty: NSRect) {
            let ivars = self.ivars();
            let (start, end) = self.span();
            let (value, loaded) = (ivars.value.get(), ivars.loaded.get());
            self.fill_track(start, end, &theme::track());
            if loaded > value {
                self.fill_track(start, self.x_of(loaded), &theme::track_loaded());
            }
            self.fill_track(start, self.x_of(value), &theme::track_filled());

            theme::mark().setFill();
            for mark in ivars.marks.borrow().iter() {
                let x = self.x_of(*mark) - theme::MARK_WIDTH / 2.0;
                NSBezierPath::fillRect(self.track_rect(x, x + theme::MARK_WIDTH));
            }

            let radius = theme::KNOB_RADIUS;
            let corner = NSPoint::new(self.x_of(value) - radius, self.middle() - radius);
            let knob = NSRect::new(corner, NSSize::new(2.0 * radius, 2.0 * radius));
            theme::track_filled().setFill();
            NSBezierPath::bezierPathWithOvalInRect(knob).fill();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.hold_at(self.x_in(event));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.hold_at(self.x_in(event));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let x = self.x_in(event);
            self.ivars().dragging.set(false);
            self.move_knob(x, Grip::Released);
            if !(0.0..self.bounds().size.width).contains(&x) {
                self.set_hovered(false);
            }
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            self.set_hovered(true);
            self.point_at(self.x_in(event));
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.point_at(self.x_in(event));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            // A drag that strays off the slider is still a drag.
            if !self.ivars().dragging.get() {
                self.set_hovered(false);
            }
        }
    }
);

impl Slider {
    /// A slider from 0 to `limit`, with the knob at 0.
    pub fn new(main_thread: MainThreadMarker, limit: f64) -> Retained<Self> {
        let ivars = Ivars { limit: Cell::new(limit), ..Ivars::default() };
        let this = Self::alloc(main_thread).set_ivars(ivars);
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let slider: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        slider.setTranslatesAutoresizingMaskIntoConstraints(false);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[slider
            .heightAnchor()
            .constraintEqualToConstant(theme::SLIDER_HEIGHT)]));

        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::ActiveInActiveApp
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: the view owns the area, so the owner outlives it.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                options,
                Some(&slider),
                None,
            )
        };
        slider.addTrackingArea(&area);
        slider
    }

    /// Sets what happens when the viewer moves the knob or lets it go, and
    /// when the pointer comes over the slider, moves along it and leaves.
    /// Can be called once.
    pub fn listen(
        &self,
        on_change: impl Fn(f64, Grip) + 'static,
        on_point: impl Fn(Option<Pointed>) + 'static,
    ) {
        // Cannot fail the first time, which is the only time by contract.
        let _ = self.ivars().on_change.set(Box::new(on_change));
        let _ = self.ivars().on_point.set(Box::new(on_point));
    }

    pub fn is_dragging(&self) -> bool {
        self.ivars().dragging.get()
    }

    /// Moves the knob, unless the viewer is holding it.
    pub fn set_value(&self, value: f64) {
        if self.is_dragging() {
            return;
        }
        self.show(&self.ivars().value, value);
    }

    pub fn limit(&self) -> f64 {
        self.ivars().limit.get()
    }

    pub fn set_limit(&self, limit: f64) {
        self.show(&self.ivars().limit, limit);
    }

    pub fn set_loaded(&self, loaded: f64) {
        self.show(&self.ivars().loaded, loaded);
    }

    pub fn set_marks(&self, marks: Vec<f64>) {
        *self.ivars().marks.borrow_mut() = marks;
        self.setNeedsDisplay(true);
    }

    /// Reports the place `x` points from the left edge as pointed at.
    pub fn point_at(&self, x: f64) {
        if let Some(on_point) = self.ivars().on_point.get() {
            on_point(Some(Pointed { value: self.value_at(x), x }));
        }
    }

    /// Stores `value` in `cell` and redraws if that changed anything.
    fn show(&self, cell: &Cell<f64>, value: f64) {
        if cell.replace(value) != value {
            self.setNeedsDisplay(true);
        }
    }

    fn set_hovered(&self, hovered: bool) {
        if self.ivars().hovered.replace(hovered) == hovered {
            return;
        }
        self.setNeedsDisplay(true);
        if let (false, Some(on_point)) = (hovered, self.ivars().on_point.get()) {
            on_point(None);
        }
    }

    /// Takes hold of the knob, if it is not held already, and puts it `x`
    /// points from the left edge.
    pub fn hold_at(&self, x: f64) {
        self.ivars().dragging.set(true);
        self.move_knob(x, Grip::Held);
    }

    /// Puts the knob `x` points from the left edge and says so. A held
    /// knob that has not moved says nothing: the pointer moves far more
    /// often than the knob has a new place to be.
    fn move_knob(&self, x: f64, grip: Grip) {
        let value = self.value_at(x);
        let moved = self.ivars().value.replace(value) != value;
        self.setNeedsDisplay(true);
        self.point_at(x);
        if !moved && grip == Grip::Held {
            return;
        }
        if let Some(on_change) = self.ivars().on_change.get() {
            on_change(value, grip);
        }
    }

    /// Where the pointer of `event` is, in points from the left edge.
    fn x_in(&self, event: &NSEvent) -> f64 {
        self.convertPoint_fromView(event.locationInWindow(), None).x - self.bounds().origin.x
    }

    /// The two ends of the track. The knob's centre stays between them, so
    /// that the knob never hangs over the slider's edge.
    fn span(&self) -> (f64, f64) {
        (theme::KNOB_RADIUS, self.bounds().size.width - theme::KNOB_RADIUS)
    }

    fn x_of(&self, value: f64) -> f64 {
        let (start, end) = self.span();
        let limit = self.ivars().limit.get();
        let fraction = if limit > 0.0 { (value / limit).clamp(0.0, 1.0) } else { 0.0 };
        start + fraction * (end - start)
    }

    fn value_at(&self, x: f64) -> f64 {
        let (start, end) = self.span();
        if end <= start {
            return 0.0;
        }
        ((x - start) / (end - start)).clamp(0.0, 1.0) * self.ivars().limit.get()
    }

    fn middle(&self) -> f64 {
        self.bounds().size.height / 2.0
    }

    /// The part of the track between two x positions.
    fn track_rect(&self, from: f64, to: f64) -> NSRect {
        let ivars = self.ivars();
        let thickness = if ivars.hovered.get() || ivars.dragging.get() {
            theme::TRACK_THICKNESS_HOVERED
        } else {
            theme::TRACK_THICKNESS
        };
        let corner = NSPoint::new(from, self.middle() - thickness / 2.0);
        NSRect::new(corner, NSSize::new(to - from, thickness))
    }

    fn fill_track(&self, from: f64, to: f64, color: &NSColor) {
        let rect = self.track_rect(from, to);
        let radius = rect.size.height / 2.0;
        color.setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius).fill();
    }
}
