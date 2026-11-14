//! The sign that the picture is on its way: at the start of a file, and
//! whenever playback stalls waiting for the network.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAccessibility, NSControlSize, NSLayoutAttribute, NSProgressIndicator,
    NSProgressIndicatorStyle, NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSString};

use super::controls::{label, medium};
use super::layout::{add, pin};
use super::surface::{Material, Surface};
use super::theme;

pub struct Waiting {
    surface: Surface,
    spinner: Retained<NSProgressIndicator>,
    /// Says what is being waited for, when that is not plain.
    caption: Retained<NSTextField>,
}

impl Waiting {
    /// The sign, hidden until there is something to wait for.
    pub fn new(main_thread: MainThreadMarker, material: Material) -> Self {
        let this = Self {
            surface: Surface::new(main_thread, material, theme::WAITING_RADIUS),
            spinner: NSProgressIndicator::new(main_thread),
            caption: label(main_thread, theme::FONT_SIZE, medium(), &theme::text()),
        };
        this.spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        this.spinner.setControlSize(NSControlSize::Large);
        this.spinner.setIndeterminate(true);
        this.spinner.setAccessibilityLabel(Some(&NSString::from_str("Loading")));
        this.view().setHidden(true);

        let content = this.surface.content();
        let parts: [&NSView; 2] = [&this.spinner, &this.caption];
        let parts = NSStackView::stackViewWithViews(&NSArray::from_slice(&parts), main_thread);
        parts.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        parts.setAlignment(NSLayoutAttribute::CenterX);
        parts.setSpacing(theme::WAITING_SPACING);
        let padding = theme::WAITING_PADDING;
        add(content, &parts);
        pin(&[
            parts.topAnchor().constraintEqualToAnchor_constant(&content.topAnchor(), padding),
            parts
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&content.bottomAnchor(), -padding),
            parts
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&content.leadingAnchor(), padding),
            parts
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&content.trailingAnchor(), -padding),
        ]);
        this
    }

    pub fn view(&self) -> &NSView {
        self.surface.view()
    }

    /// Shows the sign. `caption` says what is being waited for.
    pub fn show(&self, caption: Option<&str>) {
        self.caption.setStringValue(&NSString::from_str(caption.unwrap_or_default()));
        self.caption.setHidden(caption.is_none());
        // SAFETY: the sender is optional and not kept.
        unsafe { self.spinner.startAnimation(None) };
        self.view().setHidden(false);
    }

    /// Hides the sign and stops its spinner, which would otherwise go on
    /// turning unseen.
    pub fn hide(&self) {
        // SAFETY: the sender is optional and not kept.
        unsafe { self.spinner.stopAnimation(None) };
        self.view().setHidden(true);
    }
}
