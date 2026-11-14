//! A rounded piece of the controls that holds a symbol, a few words, or
//! both. Most chips are buttons.

use std::cell::{Cell, OnceCell};

use objc2::rc::Retained;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSEvent, NSFont, NSFontWeightMedium, NSImage,
    NSImageSymbolConfiguration, NSImageView, NSLayoutConstraint, NSLayoutPriorityDefaultLow,
    NSStackView, NSTextField, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSString, ns_string};

use super::theme;

/// What a chip looks like and whether it can be clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// A button that is only its symbol or text until the pointer is over
    /// it. For controls that sit on a pane's shade.
    Plain,
    /// A button on a light fill, readable over any picture.
    Solid,
    /// Text on a dark fill that cannot be clicked: clicks pass through it.
    Caption,
}

pub struct Ivars {
    style: Style,
    icon: Retained<NSImageView>,
    label: Retained<NSTextField>,
    /// The name of the symbol shown, to tell when setting it changes nothing.
    symbol: Cell<&'static str>,
    hovered: Cell<bool>,
    pressed: Cell<bool>,
    on_click: OnceCell<Box<dyn Fn()>>,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `Chip` does not
    // implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustPlayerChip"]
    #[ivars = Ivars]
    pub struct Chip;

    impl Chip {
        /// The whole chip is one target: the symbol and the text inside it
        /// are not separate things to hit.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            // SAFETY: forwards the argument to the superclass unchanged.
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            let is_target = self.ivars().style != Style::Caption;
            hit.filter(|_| is_target).map(|_| Retained::into_super(self.retain()))
        }

        #[unsafe(method(drawRect:))]
        fn draw(&self, _dirty: NSRect) {
            let Some(fill) = self.fill() else {
                return;
            };
            fill.setFill();
            let radius = theme::CHIP_RADIUS;
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), radius, radius)
                .fill();
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.ivars().hovered.set(true);
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.ivars().hovered.set(false);
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            self.set_pressed(true);
        }

        /// As with any button, dragging off it before letting go cancels.
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.set_pressed(self.holds(event));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            if !self.ivars().pressed.get() {
                return;
            }
            self.set_pressed(false);
            if let Some(on_click) = self.ivars().on_click.get() {
                on_click();
            }
        }
    }
);

impl Chip {
    pub fn new(main_thread: MainThreadMarker, style: Style) -> Retained<Self> {
        let color = match style {
            Style::Plain | Style::Caption => theme::text(),
            Style::Solid => theme::text_on_solid(),
        };
        let icon = NSImageView::new(main_thread);
        icon.setContentTintColor(Some(&color));
        icon.setHidden(true);
        let label = NSTextField::labelWithString(ns_string!(""), main_thread);
        label.setTextColor(Some(&color));
        // SAFETY: the weight is a constant that AppKit provides.
        let weight = unsafe { NSFontWeightMedium };
        // Digits of equal width keep a time or a speed from changing the
        // chip's width as it counts.
        label.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            theme::FONT_SIZE,
            weight,
        )));
        label.setHidden(true);

        let this = Self::alloc(main_thread).set_ivars(Ivars {
            style,
            icon,
            label,
            symbol: Cell::new(""),
            hovered: Cell::new(false),
            pressed: Cell::new(false),
            on_click: OnceCell::new(),
        });
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let chip: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        chip.lay_out(main_thread);
        if style != Style::Caption {
            chip.track_pointer();
        }
        chip
    }

    /// A button that shows the symbol named `symbol` and explains itself
    /// with `tooltip` when the pointer rests on it.
    pub fn button(
        main_thread: MainThreadMarker,
        symbol: &'static str,
        tooltip: &str,
    ) -> Retained<Self> {
        let chip = Self::new(main_thread, Style::Plain);
        chip.set_symbol(symbol, theme::SYMBOL_SIZE);
        chip.setToolTip(Some(&NSString::from_str(tooltip)));
        chip
    }

    /// Sets what a click does. Can be called once.
    pub fn on_click(&self, on_click: impl Fn() + 'static) {
        // Cannot fail the first time, which is the only time by contract.
        let _ = self.ivars().on_click.set(Box::new(on_click));
    }

    /// Shows the SF Symbol called `name`, `size` points high.
    pub fn set_symbol(&self, name: &'static str, size: f64) {
        let ivars = self.ivars();
        if ivars.symbol.replace(name) == name {
            return;
        }
        // SAFETY: the weight is a constant that AppKit provides.
        let weight = unsafe { NSFontWeightMedium };
        let size = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, weight);
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(name),
            None,
        );
        let image = image.and_then(|image| image.imageWithSymbolConfiguration(&size));
        ivars.icon.setImage(image.as_deref());
        ivars.icon.setHidden(false);
    }

    pub fn set_title(&self, title: &str) {
        let label = &self.ivars().label;
        label.setStringValue(&NSString::from_str(title));
        label.setHidden(title.is_empty());
    }

    /// Puts the symbol and the text side by side in the middle, and makes
    /// the chip a square that widens to fit them.
    fn lay_out(&self, main_thread: MainThreadMarker) {
        let ivars = self.ivars();
        let content: [&NSView; 2] = [&ivars.icon, &ivars.label];
        let content = NSStackView::stackViewWithViews(&NSArray::from_slice(&content), main_thread);
        content.setSpacing(theme::CHIP_CONTENT_SPACING);
        content.setTranslatesAutoresizingMaskIntoConstraints(false);
        self.addSubview(&content);
        self.setTranslatesAutoresizingMaskIntoConstraints(false);

        let height = match ivars.style {
            Style::Plain | Style::Solid => theme::CHIP_SIZE,
            Style::Caption => theme::CAPTION_HEIGHT,
        };
        let padded = content.widthAnchor();
        let square = self.widthAnchor().constraintEqualToConstant(height);
        square.setPriority(NSLayoutPriorityDefaultLow);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            content.centerXAnchor().constraintEqualToAnchor(&self.centerXAnchor()),
            content.centerYAnchor().constraintEqualToAnchor(&self.centerYAnchor()),
            self.heightAnchor().constraintEqualToConstant(height),
            self.widthAnchor().constraintGreaterThanOrEqualToConstant(height),
            self.widthAnchor()
                .constraintGreaterThanOrEqualToAnchor_constant(&padded, 2.0 * theme::CHIP_PADDING),
            square,
        ]));
    }

    /// Asks to hear when the pointer comes and goes, to light up under it.
    fn track_pointer(&self) {
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveInActiveApp
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: the view owns the area, so the owner outlives it.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                options,
                Some(self),
                None,
            )
        };
        self.addTrackingArea(&area);
    }

    fn set_pressed(&self, pressed: bool) {
        if self.ivars().pressed.replace(pressed) != pressed {
            self.setNeedsDisplay(true);
        }
    }

    /// Whether the pointer of `event` is on the chip.
    fn holds(&self, event: &NSEvent) -> bool {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let bounds = self.bounds();
        let (x, y) = (point.x - bounds.origin.x, point.y - bounds.origin.y);
        (0.0..bounds.size.width).contains(&x) && (0.0..bounds.size.height).contains(&y)
    }

    /// The colour behind the content right now, if any.
    fn fill(&self) -> Option<Retained<NSColor>> {
        let ivars = self.ivars();
        match (ivars.style, ivars.pressed.get(), ivars.hovered.get()) {
            (Style::Caption, ..) => Some(theme::backdrop()),
            (Style::Solid, true, _) => Some(theme::solid_pressed()),
            (Style::Solid, false, true) => Some(theme::solid_hovered()),
            (Style::Solid, false, false) => Some(theme::solid()),
            (Style::Plain, true, _) => Some(theme::pressed()),
            (Style::Plain, false, true) => Some(theme::hovered()),
            (Style::Plain, false, false) => None,
        }
    }
}
