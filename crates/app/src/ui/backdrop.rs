//! What lies behind a detail page: the colours of the item's artwork.

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSColor, NSView};
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSArray, NSRect};
use objc2_quartz_core::{CAGradientLayer, CATransaction};
use plex::model::{Colors, Rgb};

use super::theme;

pub struct Layers {
    /// The two bottom corner colours, left to right.
    lower: Retained<CAGradientLayer>,
    /// The two top corner colours, left to right, fading out downwards so
    /// that the four meet as one surface.
    upper: Retained<CAGradientLayer>,
    upper_mask: Retained<CAGradientLayer>,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `Backdrop` does not
    // implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustBackdrop"]
    #[ivars = Layers]
    pub struct Backdrop;

    impl Backdrop {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(Layers::new());
            // SAFETY: forwards the designated initializer unchanged.
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        /// Layers that are not a view's own do not follow its size.
        #[unsafe(method(layout))]
        fn layout(&self) {
            // SAFETY: forwards the call unchanged.
            let () = unsafe { msg_send![super(self), layout] };
            self.ivars().fit(self.bounds());
        }
    }
);

impl Backdrop {
    pub fn new(main_thread: MainThreadMarker) -> Retained<Self> {
        let view: Retained<Self> =
            // SAFETY: `initWithFrame:` is NSView's designated initializer.
            unsafe { msg_send![Self::alloc(main_thread), initWithFrame: NSRect::ZERO] };
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setBackgroundColor(Some(&theme::page_background().CGColor()));
            let layers = view.ivars();
            layer.addSublayer(&layers.lower);
            layer.addSublayer(&layers.upper);
        }
        view
    }

    /// Paints the page with the four corner colours of an item's artwork.
    pub fn set_colors(&self, colors: Colors) {
        let layers = self.ivars();
        set_gradient(&layers.lower, &tint(colors.bottom_left), &tint(colors.bottom_right));
        set_gradient(&layers.upper, &tint(colors.top_left), &tint(colors.top_right));
    }
}

impl Layers {
    fn new() -> Self {
        let sideways = || {
            let gradient = CAGradientLayer::new();
            gradient.setStartPoint(CGPoint { x: 0.0, y: 0.5 });
            gradient.setEndPoint(CGPoint { x: 1.0, y: 0.5 });
            gradient
        };
        // Opaque at the top, clear at the bottom. A layer's own coordinates
        // have their origin at the bottom left.
        let downwards = || {
            let gradient = CAGradientLayer::new();
            gradient.setStartPoint(CGPoint { x: 0.5, y: 1.0 });
            gradient.setEndPoint(CGPoint { x: 0.5, y: 0.0 });
            set_gradient(&gradient, &NSColor::blackColor(), &NSColor::clearColor());
            gradient
        };

        let (lower, upper, upper_mask) = (sideways(), sideways(), downwards());
        // SAFETY: a mask is a layer with no parent, which this one is.
        unsafe { upper.setMask(Some(&upper_mask)) };
        Self { lower, upper, upper_mask }
    }

    /// Sizes every layer for a view with these bounds.
    fn fit(&self, bounds: NSRect) {
        // Without this each layer would glide to its new size, a quarter
        // of a second behind the window being resized.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.lower.setFrame(bounds);
        self.upper.setFrame(bounds);
        self.upper_mask.setFrame(bounds);
        CATransaction::commit();
    }
}

/// A corner colour, darkened so that white text on it stays readable.
pub fn tint(color: Rgb) -> Retained<NSColor> {
    let part = |byte: u8| f64::from(byte) / 255.0 * theme::TINT_BRIGHTNESS;
    NSColor::colorWithSRGBRed_green_blue_alpha(
        part(color.red),
        part(color.green),
        part(color.blue),
        1.0,
    )
}

pub fn set_gradient(layer: &CAGradientLayer, from: &NSColor, to: &NSColor) {
    let (from, to) = (from.CGColor(), to.CGColor());
    let colors: [&AnyObject; 2] = [(*from).as_ref(), (*to).as_ref()];
    // SAFETY: a gradient layer takes an array of CGColors.
    unsafe { layer.setColors(Some(&NSArray::from_slice(&colors))) };
}
