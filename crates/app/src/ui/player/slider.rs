//! A track with a knob, for the place in the film and for the volume.
//!
//! It is far taller than what it draws, so it is easy to grab, and a click
//! anywhere on it moves the knob there at once.

use std::cell::{Cell, OnceCell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilitySliderRole, NSBezierPath, NSColor, NSEvent, NSGraphicsContext,
    NSLayoutConstraint, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindingRule,
};
use objc2_foundation::{NSArray, NSNumber, NSObject, NSPoint, NSRect, NSSize, NSString};

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

/// What a slider's value is, which decides how VoiceOver says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reading {
    /// A share of the whole: "65 percent".
    #[default]
    Share,
    /// Seconds into a stretch of time: "12 minutes 25 seconds of 18
    /// minutes 29 seconds".
    Time,
}

#[derive(Default)]
pub struct Ivars {
    /// Between 0 and `limit`.
    value: Cell<f64>,
    limit: Cell<f64>,
    /// How far VoiceOver moves the knob at a time.
    step: f64,
    reading: Reading,
    /// Up to where the track is drawn as loaded. Nothing below `value`.
    loaded: Cell<f64>,
    /// Values at which the track is notched.
    marks: RefCell<Vec<f64>>,
    hovered: Cell<bool>,
    /// Where along the slider the pointer is, in points from its left edge.
    pointed: Cell<Option<f64>>,
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
            let pointed = ivars.pointed.get().filter(|_| !ivars.dragging.get());
            let pointed = pointed.map(|x| x.clamp(start, end));

            // A chapter starts at a gap in the track. A gap shows on the
            // filled part and on the rest alike, which a line of any one
            // colour would not. The gaps are left out of what may be drawn
            // on: with the even-odd rule, a rectangle inside the bounds
            // is a hole in them.
            NSGraphicsContext::saveGraphicsState_class();
            let drawable = NSBezierPath::bezierPathWithRect(self.bounds());
            for mark in ivars.marks.borrow().iter() {
                let x = self.x_of(*mark) - theme::MARK_WIDTH / 2.0;
                drawable.appendBezierPathWithRect(self.track_rect(x, x + theme::MARK_WIDTH));
            }
            drawable.setWindingRule(NSWindingRule::EvenOdd);
            drawable.addClip();
            self.fill_track(start, end, &theme::track_bed());
            self.fill_track(start, end, &theme::track());
            if loaded > value {
                self.fill_track(start, self.x_of(loaded), &theme::track_loaded());
            }
            if let Some(x) = pointed {
                self.fill_track(start, x, &theme::track_pointed());
            }
            self.fill_track(start, self.x_of(value), &theme::track_filled());
            NSGraphicsContext::restoreGraphicsState_class();

            // Where a click would land, seen on the track itself and not
            // only read off the bubble. Dark around white, to show on the
            // filled part and on the rest.
            if let Some(x) = pointed {
                let height = theme::POINTER_HEIGHT;
                let line = |width: f64| {
                    let corner = NSPoint::new(x - width / 2.0, self.middle() - height / 2.0);
                    NSRect::new(corner, NSSize::new(width, height))
                };
                theme::knob_rim().setFill();
                NSBezierPath::fillRect(line(theme::POINTER_WIDTH + 2.0));
                theme::track_filled().setFill();
                NSBezierPath::fillRect(line(theme::POINTER_WIDTH));
            }

            let center = NSPoint::new(self.x_of(value), self.middle());
            // A dark rim keeps the knob apart from the filled track it
            // sits on and from a white picture behind it.
            theme::knob_rim().setFill();
            NSBezierPath::bezierPathWithOvalInRect(disc(center, self.knob_radius() + 1.0)).fill();
            theme::track_filled().setFill();
            NSBezierPath::bezierPathWithOvalInRect(disc(center, self.knob_radius())).fill();
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

        /// How far along the knob is, in percent, for VoiceOver.
        #[unsafe(method_id(accessibilityValue))]
        fn accessibility_value(&self) -> Option<Retained<AnyObject>> {
            let limit = self.ivars().limit.get();
            let percent = if limit > 0.0 { 100.0 * self.ivars().value.get() / limit } else { 0.0 };
            let number: Retained<NSObject> =
                Retained::into_super(Retained::into_super(NSNumber::new_f64(percent.round())));
            Some(Retained::into_super(number))
        }

        /// The value in words. Without it VoiceOver reads the number
        /// above bare, and "67" says nothing about a film.
        #[unsafe(method_id(accessibilityValueDescription))]
        fn accessibility_value_description(&self) -> Option<Retained<NSString>> {
            let (value, limit) = (self.ivars().value.get(), self.ivars().limit.get());
            let words = match self.ivars().reading {
                Reading::Share if limit > 0.0 => format!("{:.0} percent", 100.0 * value / limit),
                Reading::Share => "0 percent".to_owned(),
                Reading::Time => format!("{} of {}", spoken(value), spoken(limit)),
            };
            Some(NSString::from_str(&words))
        }

        #[unsafe(method(accessibilityPerformIncrement))]
        fn perform_increment(&self) -> bool {
            self.step_by(self.ivars().step);
            true
        }

        #[unsafe(method(accessibilityPerformDecrement))]
        fn perform_decrement(&self) -> bool {
            self.step_by(-self.ivars().step);
            true
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
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
    /// A slider called `name` from 0 to `limit`, with the knob at 0.
    /// VoiceOver moves the knob `step` at a time and says the value as
    /// `reading` has it.
    pub fn new(
        main_thread: MainThreadMarker,
        name: &str,
        limit: f64,
        step: f64,
        reading: Reading,
    ) -> Retained<Self> {
        let ivars = Ivars { limit: Cell::new(limit), step, reading, ..Ivars::default() };
        let this = Self::alloc(main_thread).set_ivars(ivars);
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let slider: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        slider.setTranslatesAutoresizingMaskIntoConstraints(false);
        slider.setAccessibilityElement(true);
        // SAFETY: the role is a constant that AppKit provides.
        slider.setAccessibilityRole(Some(unsafe { NSAccessibilitySliderRole }));
        slider.setAccessibilityLabel(Some(&NSString::from_str(name)));
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
        self.set_hovered(true);
        if self.ivars().pointed.replace(Some(x)) != Some(x) {
            self.setNeedsDisplay(true);
        }
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
        if !hovered {
            self.ivars().pointed.set(None);
        }
        if let (false, Some(on_point)) = (hovered, self.ivars().on_point.get()) {
            on_point(None);
        }
    }

    /// Moves the knob by `amount` and lets it go there.
    fn step_by(&self, amount: f64) {
        let ivars = self.ivars();
        let value = (ivars.value.get() + amount).clamp(0.0, ivars.limit.get());
        self.show(&ivars.value, value);
        if let Some(on_change) = ivars.on_change.get() {
            on_change(value, Grip::Released);
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
        let inset = theme::KNOB_RADIUS_HELD + 1.0;
        (inset, self.bounds().size.width - inset)
    }

    fn is_in_use(&self) -> bool {
        self.ivars().hovered.get() || self.ivars().dragging.get()
    }

    /// The knob swells along with the track, to say it can be taken.
    fn knob_radius(&self) -> f64 {
        if self.is_in_use() { theme::KNOB_RADIUS_HELD } else { theme::KNOB_RADIUS }
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
        let thickness =
            if self.is_in_use() { theme::TRACK_THICKNESS_HOVERED } else { theme::TRACK_THICKNESS };
        let corner = NSPoint::new(from, self.middle() - thickness / 2.0);
        NSRect::new(corner, NSSize::new(to - from, thickness))
    }

    fn fill_track(&self, from: f64, to: f64, color: &NSColor) {
        if to <= from {
            return;
        }
        let rect = self.track_rect(from, to);
        let radius = rect.size.height / 2.0;
        color.setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius).fill();
    }
}

/// A length of time as it is said aloud: "1 hour 2 minutes 3 seconds",
/// leaving out the hours of a time that has none.
fn spoken(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    let count = |number: u64, unit: &str| match number {
        1 => format!("1 {unit}"),
        _ => format!("{number} {unit}s"),
    };
    let rest = format!("{} {}", count(minutes, "minute"), count(seconds, "second"));
    if hours > 0 { format!("{} {rest}", count(hours, "hour")) } else { rest }
}

/// The square around a circle of `radius` about `center`.
fn disc(center: NSPoint, radius: f64) -> NSRect {
    let corner = NSPoint::new(center.x - radius, center.y - radius);
    NSRect::new(corner, NSSize::new(2.0 * radius, 2.0 * radius))
}

#[cfg(test)]
mod tests {
    use super::spoken;

    #[test]
    fn a_time_is_said_in_words_with_hours_only_when_it_has_them() {
        assert_eq!(spoken(745.9), "12 minutes 25 seconds");
        assert_eq!(spoken(3661.0), "1 hour 1 minute 1 second");
        assert_eq!(spoken(0.0), "0 minutes 0 seconds");
    }
}
