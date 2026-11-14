//! What the player shows when a file stops playing because something
//! went wrong: what happened, and the two ways on from there.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityGroupRole, NSFont, NSImageView, NSLayoutAttribute, NSStackView,
    NSTextAlignment, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSString};

use super::chip::{Chip, Style};
use super::controls::{label, semibold, symbol};
use super::layout::{add, pin, row};
use super::surface::{Material, Surface};
use super::theme;

pub struct Failure {
    surface: Surface,
    title: Retained<NSTextField>,
    /// The player's own words for what went wrong.
    reason: Retained<NSTextField>,
    pub retry: Retained<Chip>,
    pub leave: Retained<Chip>,
}

impl Failure {
    /// The card, hidden until something fails.
    pub fn new(main_thread: MainThreadMarker, material: Material) -> Self {
        let reason = NSTextField::wrappingLabelWithString(&NSString::new(), main_thread);
        reason.setFont(Some(&NSFont::systemFontOfSize(theme::FONT_SIZE)));
        reason.setTextColor(Some(&theme::secondary_text()));
        reason.setAlignment(NSTextAlignment::Center);
        reason.setMaximumNumberOfLines(theme::FAILURE_REASON_LINES);
        reason.setPreferredMaxLayoutWidth(theme::FAILURE_TEXT_WIDTH);
        reason.setSelectable(false);
        let this = Self {
            surface: Surface::new(main_thread, material, theme::CARD_RADIUS),
            title: label(main_thread, theme::FAILURE_TITLE_FONT_SIZE, semibold(), &theme::text()),
            reason,
            retry: Chip::new(main_thread, Style::Solid),
            leave: Chip::new(main_thread, Style::Quiet),
        };
        this.retry.set_symbol("arrow.clockwise", theme::FONT_SIZE);
        this.retry.set_title("Try Again");
        this.retry.setToolTip(Some(&NSString::from_str("Play on from where it stopped (Return)")));
        this.leave.set_title("Go Back");
        this.leave.setToolTip(Some(&NSString::from_str("Leave the player (Esc)")));
        let card = this.view();
        card.setAccessibilityElement(true);
        // SAFETY: the role is a constant that AppKit provides.
        card.setAccessibilityRole(Some(unsafe { NSAccessibilityGroupRole }));
        card.setHidden(true);
        this.lay_out(main_thread);
        this
    }

    pub fn view(&self) -> &NSView {
        self.surface.view()
    }

    fn lay_out(&self, main_thread: MainThreadMarker) {
        let card = self.surface.content();
        let icon = NSImageView::new(main_thread);
        icon.setImage(
            symbol("exclamationmark.triangle.fill", theme::FAILURE_SYMBOL_SIZE).as_deref(),
        );
        icon.setContentTintColor(Some(&theme::text()));
        let buttons: [&NSView; 2] = [&self.retry, &self.leave];
        let buttons = row(&buttons, theme::CARD_BUTTON_SPACING, main_thread);
        let lines: [&NSView; 4] = [&icon, &self.title, &self.reason, &buttons];
        let lines = NSStackView::stackViewWithViews(&NSArray::from_slice(&lines), main_thread);
        lines.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        lines.setAlignment(NSLayoutAttribute::CenterX);
        lines.setSpacing(theme::FAILURE_LINE_SPACING);
        lines.setCustomSpacing_afterView(theme::CARD_PADDING, &self.reason);

        let padding = theme::FAILURE_PADDING;
        add(card, &lines);
        pin(&[
            lines.topAnchor().constraintEqualToAnchor_constant(&card.topAnchor(), padding),
            lines.bottomAnchor().constraintEqualToAnchor_constant(&card.bottomAnchor(), -padding),
            lines.leadingAnchor().constraintEqualToAnchor_constant(&card.leadingAnchor(), padding),
            lines
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&card.trailingAnchor(), -padding),
            self.title.widthAnchor().constraintLessThanOrEqualToConstant(theme::FAILURE_TEXT_WIDTH),
        ]);
    }

    /// Shows the card, saying that what is called `title` stopped for
    /// `reason`.
    pub fn show(&self, title: &str, reason: &str) {
        let heading = "Playback stopped";
        self.title.setStringValue(&NSString::from_str(heading));
        self.reason.setStringValue(&NSString::from_str(reason));
        self.view()
            .setAccessibilityLabel(Some(&NSString::from_str(&format!("{heading}: {title}"))));
        self.view().setHidden(false);
    }

    pub fn hide(&self) {
        self.view().setHidden(true);
    }
}
