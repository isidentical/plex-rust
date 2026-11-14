//! The strip down the far edge of a library listed by title: the first
//! characters of its titles, each a way to where those titles start. A
//! press on one goes there, and so does a press that slides along the
//! strip and lets go on one, which is how a finger finds a small letter.

use std::cell::{Cell, OnceCell, RefCell};

use objc2::rc::{Allocated, Retained};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSColor, NSEvent, NSLayoutAttribute, NSLayoutConstraint,
    NSLayoutPriorityRequired, NSStackView, NSStackViewDistribution, NSStackViewGravity,
    NSTextAlignment, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSPoint, NSRect};

use super::label::{self, Weight};
use super::theme;

#[derive(Default)]
pub struct Ivars {
    /// One label for each character, top to bottom.
    marks: RefCell<Vec<Retained<NSTextField>>>,
    /// The column the labels are stacked in, once there is one.
    column: OnceCell<Retained<NSStackView>>,
    /// As tall as the labels ask for, where the window leaves the room.
    height: OnceCell<Retained<NSLayoutConstraint>>,
    on_pick: OnceCell<Box<dyn Fn(usize)>>,
    /// The character whose titles are at the top of the page.
    current: Cell<Option<usize>>,
    /// The character under a press that has not been let go.
    pressed: Cell<Option<usize>>,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `JumpBar` does not
    // implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustJumpBar"]
    #[ivars = Ivars]
    pub struct JumpBar;

    impl JumpBar {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(Ivars::default());
            // SAFETY: forwards the designated initializer unchanged.
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        /// The strip takes every press on it itself, wherever among its
        /// labels the press lands.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            // SAFETY: forwards the call unchanged.
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            hit.map(|_| self.retain().into_super())
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.press(self.mark_at(event));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.press(self.mark_at(event));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            let picked = self.ivars().pressed.take();
            self.paint();
            if let (Some(picked), Some(on_pick)) = (picked, self.ivars().on_pick.get()) {
                on_pick(picked);
            }
        }
    }
);

impl JumpBar {
    /// An empty strip, which takes no room until [`show`](Self::show)
    /// gives it characters. `on_pick` hears which of them was pressed, by
    /// its place among them.
    pub fn new(main_thread: MainThreadMarker, on_pick: impl Fn(usize) + 'static) -> Retained<Self> {
        let bar: Retained<Self> =
            // SAFETY: `initWithFrame:` is NSView's designated initializer.
            unsafe { msg_send![Self::alloc(main_thread), initWithFrame: NSRect::ZERO] };
        bar.setTranslatesAutoresizingMaskIntoConstraints(false);
        bar.setHidden(true);

        let column = NSStackView::new(main_thread);
        column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        column.setAlignment(NSLayoutAttribute::CenterX);
        column.setDistribution(NSStackViewDistribution::FillEqually);
        column.setSpacing(0.0);
        column.setTranslatesAutoresizingMaskIntoConstraints(false);
        bar.addSubview(&column);
        let height = bar.heightAnchor().constraintEqualToConstant(0.0);
        // The window may be too short for every character at its full
        // height. Then they are set closer. Short of that, the height
        // outranks the labels' wish to be no taller than their text.
        height.setPriority(NSLayoutPriorityRequired - 1.0);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            bar.widthAnchor().constraintEqualToConstant(theme::JUMP_BAR_WIDTH),
            height.clone(),
            column.leadingAnchor().constraintEqualToAnchor(&bar.leadingAnchor()),
            column.trailingAnchor().constraintEqualToAnchor(&bar.trailingAnchor()),
            column.topAnchor().constraintEqualToAnchor(&bar.topAnchor()),
            column.bottomAnchor().constraintEqualToAnchor(&bar.bottomAnchor()),
        ]));
        // Cannot fail: the cells were created empty by the initializer.
        let _ = bar.ivars().column.set(column);
        let _ = bar.ivars().height.set(height);
        let _ = bar.ivars().on_pick.set(Box::new(on_pick));
        bar
    }

    /// Lists `characters` from the top. Fewer than two are no way to
    /// anywhere, and hide the strip.
    pub fn show(&self, characters: &[String]) {
        let ivars = self.ivars();
        let (Some(column), Some(height)) = (ivars.column.get(), ivars.height.get()) else {
            return;
        };
        let marks: Vec<Retained<NSTextField>> = characters
            .iter()
            .map(|character| {
                let colour = NSColor::secondaryLabelColor();
                let mark = label::line(theme::FONT_CAPTION, Weight::Semibold, &colour, self.mtm());
                mark.setAlignment(NSTextAlignment::Center);
                // Loose letters say nothing to VoiceOver, which has the
                // grid itself to go through.
                mark.setAccessibilityElement(false);
                label::set(&mark, character);
                mark
            })
            .collect();
        let views: Vec<&NSView> = marks.iter().map(|mark| -> &NSView { mark }).collect();
        column.setViews_inGravity(&NSArray::from_slice(&views), NSStackViewGravity::Top);
        height.setConstant(theme::JUMP_MARK_HEIGHT * marks.len() as f64);
        self.setHidden(marks.len() < 2);
        ivars.marks.replace(marks);
        ivars.pressed.set(None);
        self.paint();
    }

    /// Marks the character whose titles are at the top of the page.
    pub fn set_current(&self, current: Option<usize>) {
        if self.ivars().current.replace(current) != current {
            self.paint();
        }
    }

    /// Which character `event` happened on. One past either end of the
    /// strip is the character at that end, so that a press that slides
    /// off the strip keeps the last one it was on.
    fn mark_at(&self, event: &NSEvent) -> Option<usize> {
        let count = self.ivars().marks.borrow().len();
        let (point, bounds) =
            (self.convertPoint_fromView(event.locationInWindow(), None), self.bounds());
        if count == 0 || bounds.size.height <= 0.0 {
            return None;
        }
        // A view's own coordinates have their origin at the bottom left.
        let down = 1.0 - (point.y - bounds.origin.y) / bounds.size.height;
        Some(((down * count as f64).floor().max(0.0) as usize).min(count - 1))
    }

    fn press(&self, pressed: Option<usize>) {
        if self.ivars().pressed.replace(pressed) != pressed {
            self.paint();
        }
    }

    /// Colours each character by what it is: the one under a press, the
    /// one the page is at, or one of the others.
    fn paint(&self) {
        let (current, pressed) = (self.ivars().current.get(), self.ivars().pressed.get());
        for (place, mark) in self.ivars().marks.borrow().iter().enumerate() {
            let colour = match (pressed, current) {
                (Some(pressed), _) if pressed == place => NSColor::controlAccentColor(),
                (None, Some(current)) if current == place => NSColor::labelColor(),
                _ => NSColor::secondaryLabelColor(),
            };
            mark.setTextColor(Some(&colour));
        }
    }
}
