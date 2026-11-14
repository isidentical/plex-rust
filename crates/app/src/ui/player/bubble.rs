//! The bubble over the scrubber that says what time the pointer is at, and
//! which chapter that is in.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAccessibility, NSLayoutConstraint, NSTextField, NSView};
use objc2_foundation::NSString;

use super::controls::{label, medium, regular};
use super::layout::{add, pin, row};
use super::surface::{Material, Surface};
use super::theme;

pub struct Bubble {
    /// A panel like the bar's, so that the two lines read over a white
    /// picture as the bar's own times do. On a plain dark fill they stood
    /// on a light grey there.
    surface: Surface,
    time: Retained<NSTextField>,
    chapter: Retained<NSTextField>,
    /// Holds the bubble over the pointer: the constant is the distance of
    /// its middle from the scrubber's left edge.
    over: Retained<NSLayoutConstraint>,
}

impl Bubble {
    /// The bubble, hidden until the pointer is on the scrubber. `scrubber`
    /// is what it follows the pointer along.
    pub fn new(main_thread: MainThreadMarker, material: Material, scrubber: &NSView) -> Self {
        let surface = Surface::new(main_thread, material, theme::BUBBLE_HEIGHT / 2.0);
        let over =
            surface.view().centerXAnchor().constraintEqualToAnchor(&scrubber.leadingAnchor());
        // Less than anything else in the layout: a bubble that cannot be
        // where the pointer is gives way. At the default strength it
        // moved the scrubber and widened the window instead.
        over.setPriority(theme::FOLLOWS_POINTER);
        let bubble = Self {
            surface,
            time: label(main_thread, theme::CONTROL_FONT_SIZE, medium(), &theme::text()),
            chapter: label(main_thread, theme::FONT_SIZE, regular(), &theme::secondary_text()),
            over,
        };
        bubble.view().setHidden(true);
        // The scrubber says its own value to VoiceOver.
        bubble.view().setAccessibilityElement(false);

        let content = bubble.surface.content();
        let lines: [&NSView; 2] = [&bubble.time, &bubble.chapter];
        let lines = row(&lines, theme::BUBBLE_SPACING, main_thread);
        add(content, &lines);
        let padding = theme::BUBBLE_PADDING;
        pin(&[
            content.heightAnchor().constraintEqualToConstant(theme::BUBBLE_HEIGHT),
            content.widthAnchor().constraintLessThanOrEqualToConstant(theme::NOTICE_MAX_WIDTH),
            lines.centerYAnchor().constraintEqualToAnchor(&content.centerYAnchor()),
            lines
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&content.leadingAnchor(), padding),
            lines
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&content.trailingAnchor(), -padding),
        ]);
        bubble
    }

    pub fn view(&self) -> &NSView {
        self.surface.view()
    }

    /// What holds the bubble over the pointer, to be put into effect once
    /// the bubble and the scrubber are in one view.
    pub fn follows(&self) -> Retained<NSLayoutConstraint> {
        self.over.clone()
    }

    /// Shows `time`, and the name of the `chapter` it is in if that has
    /// one, `x` points along the scrubber.
    pub fn show(&self, time: &str, chapter: Option<&str>, x: f64) {
        self.time.setStringValue(&NSString::from_str(time));
        self.chapter.setStringValue(&NSString::from_str(chapter.unwrap_or_default()));
        self.chapter.setHidden(chapter.is_none());
        self.over.setConstant(x);
        self.view().setHidden(false);
    }

    pub fn hide(&self) {
        self.view().setHidden(true);
    }
}
