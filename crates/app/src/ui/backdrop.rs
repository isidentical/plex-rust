//! What lies behind a detail page: the colours of the item's artwork, and
//! its backdrop fading into them.

use std::rc::Rc;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSColor, NSView};
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSArray, NSRect, ns_string};
use objc2_quartz_core::{
    CAGradientLayer, CALayer, CAMediaTiming, CATransaction, CATransition,
    kCAGravityResizeAspectFill, kCATransitionFade,
};
use plex::model::{Colors, Rgb};

use super::theme;
use crate::images::Images;

pub struct Layers {
    /// The two bottom corner colours, left to right.
    lower: Retained<CAGradientLayer>,
    /// The two top corner colours, left to right, fading out downwards so
    /// that the four meet as one surface.
    upper: Retained<CAGradientLayer>,
    upper_mask: Retained<CAGradientLayer>,
    /// The backdrop picture, fading out downwards.
    art: Retained<CALayer>,
    art_mask: Retained<CAGradientLayer>,
    /// Darkens the left, where the text is.
    scrim: Retained<CAGradientLayer>,
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
            layer.addSublayer(&layers.art);
            layer.addSublayer(&layers.scrim);
        }
        view
    }

    /// Paints the page with the four corner colours of an item's artwork.
    pub fn set_colors(&self, colors: Colors) {
        let layers = self.ivars();
        set_gradient(&layers.lower, &tint(colors.bottom_left), &tint(colors.bottom_right));
        set_gradient(&layers.upper, &tint(colors.top_left), &tint(colors.top_right));
    }

    /// Shows the backdrop at server path `path`, fading it in when it had
    /// to be fetched.
    pub fn show_art(&self, path: &str, images: &Rc<Images>) {
        let (width, height) = theme::BACKDROP_REQUEST;
        let art = self.ivars().art.clone();
        if let Some(artwork) = images.cached(path, width, height) {
            // SAFETY: the contents are a decoded CGImage.
            unsafe { art.setContents(Some(artwork.contents())) };
            return;
        }
        images.load(path, width, height, move |artwork| {
            let fade = CATransition::new();
            // SAFETY: the type is a constant that Core Animation provides.
            fade.setType(unsafe { kCATransitionFade });
            fade.setDuration(theme::ARTWORK_FADE);
            art.addAnimation_forKey(&fade, Some(ns_string!("contents")));
            // SAFETY: the contents are a decoded CGImage.
            unsafe { art.setContents(Some(artwork.contents())) };
        });
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

        let (lower, upper, scrim) = (sideways(), sideways(), sideways());
        let (upper_mask, art_mask) = (downwards(), downwards());
        set_gradient(&scrim, &theme::black(theme::BACKDROP_SCRIM_OPACITY), &NSColor::clearColor());
        let art = CALayer::new();
        // SAFETY: the gravity is a constant that Core Animation provides.
        art.setContentsGravity(unsafe { kCAGravityResizeAspectFill });
        art.setMasksToBounds(true);
        art.setOpacity(theme::BACKDROP_OPACITY);
        // SAFETY: a mask is a layer with no parent, which these are.
        unsafe {
            upper.setMask(Some(&upper_mask));
            art.setMask(Some(&art_mask));
        }
        Self { lower, upper, upper_mask, art, art_mask, scrim }
    }

    /// Sizes every layer for a view with these bounds.
    fn fit(&self, bounds: NSRect) {
        let mut top = bounds;
        top.size.height = (bounds.size.height * theme::BACKDROP_HEIGHT_FRACTION).round();
        top.origin.y = bounds.size.height - top.size.height;
        let at_origin = |rect: NSRect| NSRect { origin: CGPoint::ZERO, size: rect.size };

        // Without this each layer would glide to its new size, a quarter
        // of a second behind the window being resized.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.lower.setFrame(bounds);
        self.upper.setFrame(bounds);
        self.upper_mask.setFrame(bounds);
        self.scrim.setFrame(bounds);
        self.art.setFrame(top);
        self.art_mask.setFrame(at_origin(top));
        CATransaction::commit();
    }
}

/// A corner colour, darkened so that white text on it stays readable.
fn tint(color: Rgb) -> Retained<NSColor> {
    let part = |byte: u8| f64::from(byte) / 255.0 * theme::TINT_BRIGHTNESS;
    NSColor::colorWithSRGBRed_green_blue_alpha(
        part(color.red),
        part(color.green),
        part(color.blue),
        1.0,
    )
}

fn set_gradient(layer: &CAGradientLayer, from: &NSColor, to: &NSColor) {
    let (from, to) = (from.CGColor(), to.CGColor());
    let colors: [&AnyObject; 2] = [(*from).as_ref(), (*to).as_ref()];
    // SAFETY: a gradient layer takes an array of CGColors.
    unsafe { layer.setColors(Some(&NSArray::from_slice(&colors))) };
}
