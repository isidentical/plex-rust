//! The badge that says for a moment what a key press did: a symbol, a
//! word or a number, and for the volume and the speed how far along they
//! are.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityAnnouncementKey,
    NSAccessibilityAnnouncementRequestedNotification, NSAccessibilityPostNotificationWithUserInfo,
    NSAnimatablePropertyContainer, NSBezierPath, NSImageView, NSTextField, NSView,
};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

use super::controls::{label, semibold, symbol};
use super::layout::{add, pin, row};
use super::surface::{Material, Surface};
use super::theme;

/// What the badge says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Notice<'a> {
    /// The name of an SF Symbol for what changed.
    pub symbol: &'static str,
    pub text: &'a str,
    /// How far along its range what changed now is, from 0 to 1, when it
    /// has a range.
    pub level: Option<f64>,
}

pub struct MeterIvars {
    level: Cell<f64>,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `Meter` does not
    // implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustPlayerMeter"]
    #[ivars = MeterIvars]
    struct Meter;

    impl Meter {
        #[unsafe(method(drawRect:))]
        fn draw(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            let radius = bounds.size.height / 2.0;
            let bar = |width: f64| {
                let rect = NSRect::new(NSPoint::ZERO, NSSize::new(width, bounds.size.height));
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius)
            };
            theme::track().setFill();
            bar(bounds.size.width).fill();
            let filled = bounds.size.width * self.ivars().level.get();
            // Narrower than it is high, the filled end would not be round.
            if filled >= bounds.size.height {
                theme::track_filled().setFill();
                bar(filled).fill();
            }
        }
    }
);

pub struct Badge {
    surface: Surface,
    icon: Retained<NSImageView>,
    text: Retained<NSTextField>,
    meter: Retained<Meter>,
    shown: Cell<bool>,
}

impl Badge {
    /// The badge, not yet showing.
    pub fn new(main_thread: MainThreadMarker, material: Material) -> Self {
        let this = Meter::alloc(main_thread).set_ivars(MeterIvars { level: Cell::new(0.0) });
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let meter: Retained<Meter> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        let badge = Self {
            surface: Surface::new(main_thread, material, theme::BADGE_HEIGHT / 2.0),
            icon: NSImageView::new(main_thread),
            text: label(main_thread, theme::BADGE_FONT_SIZE, semibold(), &theme::text()),
            meter,
            shown: Cell::new(false),
        };
        badge.icon.setContentTintColor(Some(&theme::text()));
        badge.view().setAlphaValue(0.0);
        // VoiceOver is told what the badge says when it says it, and has
        // no use for finding it afterwards.
        badge.view().setAccessibilityElement(false);

        let content = badge.surface.content();
        let parts: [&NSView; 3] = [&badge.icon, &badge.text, &badge.meter];
        let parts = row(&parts, theme::BADGE_SPACING, main_thread);
        add(content, &parts);
        let padding = theme::BADGE_PADDING;
        pin(&[
            content.heightAnchor().constraintEqualToConstant(theme::BADGE_HEIGHT),
            content.widthAnchor().constraintLessThanOrEqualToConstant(theme::NOTICE_MAX_WIDTH),
            parts.centerYAnchor().constraintEqualToAnchor(&content.centerYAnchor()),
            parts
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&content.leadingAnchor(), padding),
            parts
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&content.trailingAnchor(), -padding),
            badge.meter.widthAnchor().constraintEqualToConstant(theme::METER_WIDTH),
            badge.meter.heightAnchor().constraintEqualToConstant(theme::METER_HEIGHT),
        ]);
        badge
    }

    pub fn view(&self) -> &NSView {
        self.surface.view()
    }

    /// Shows `notice`, and has VoiceOver say it.
    pub fn show(&self, notice: &Notice) {
        self.icon.setImage(symbol(notice.symbol, theme::BADGE_SYMBOL_SIZE).as_deref());
        self.text.setStringValue(&NSString::from_str(notice.text));
        self.meter.setHidden(notice.level.is_none());
        if let Some(level) = notice.level {
            self.meter.ivars().level.set(level.clamp(0.0, 1.0));
            self.meter.setNeedsDisplay(true);
        }
        self.announce(notice.text);
        self.set_shown(true);
    }

    pub fn hide(&self) {
        self.set_shown(false);
    }

    fn set_shown(&self, shown: bool) {
        // A notice that is renewed while it is up is not faded in again.
        if self.shown.replace(shown) != shown {
            self.view().animator().setAlphaValue(if shown { 1.0 } else { 0.0 });
        }
    }

    /// Has VoiceOver say `text`, as a notice is of no use to a viewer who
    /// has to go looking for it before it is gone.
    fn announce(&self, text: &str) {
        // An announcement is only heard if it comes from a window.
        let Some(window) = self.view().window() else {
            return;
        };
        let text = NSString::from_str(text);
        let said: &AnyObject = &text;
        // SAFETY: the key is a constant that AppKit provides.
        let key = unsafe { NSAccessibilityAnnouncementKey };
        let info = NSDictionary::from_slices(&[key], &[said]);
        // SAFETY: a window is an accessibility element, the notification
        // is a constant that AppKit provides, and the announcement key
        // takes a string.
        unsafe {
            NSAccessibilityPostNotificationWithUserInfo(
                &window,
                NSAccessibilityAnnouncementRequestedNotification,
                Some(&info),
            );
        }
    }
}
