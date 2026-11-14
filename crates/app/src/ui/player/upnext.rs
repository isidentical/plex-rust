//! The card that offers the next episode as this one ends.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityGroupRole, NSBezierPath, NSEvent, NSLayoutAttribute,
    NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSRect, NSString};
use plex::model::Item;

use super::chip::{Chip, Style};
use super::controls::{label, medium, semibold};
use super::layout::{add, pin, row};
use super::theme;

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `Card` does not
    // implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustPlayerCard"]
    struct Card;

    impl Card {
        #[unsafe(method(drawRect:))]
        fn draw(&self, _dirty: NSRect) {
            theme::card().setFill();
            let radius = theme::CARD_RADIUS;
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), radius, radius)
                .fill();
        }

        /// A click between the card's buttons ends here, instead of
        /// reaching the video and pausing it.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {}
    }
);

pub struct UpNext {
    card: Retained<Card>,
    countdown: Retained<NSTextField>,
    /// The number of the episode on offer, once one is.
    number: Cell<Option<u32>>,
    pub play: Retained<Chip>,
    pub cancel: Retained<Chip>,
}

impl UpNext {
    /// The card, hidden until there is an episode to offer.
    pub fn new(main_thread: MainThreadMarker) -> Self {
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let card: Retained<Card> =
            unsafe { msg_send![Card::alloc(main_thread), initWithFrame: NSRect::ZERO] };
        let countdown = label(main_thread, theme::FONT_SIZE, medium(), &theme::secondary_text());
        let this = Self {
            card,
            countdown,
            number: Cell::new(None),
            play: Chip::new(main_thread, Style::Solid),
            cancel: Chip::new(main_thread, Style::Quiet),
        };
        this.play.set_symbol("play.fill", theme::FONT_SIZE);
        this.play.set_title("Play Now");
        this.play.setToolTip(Some(&NSString::from_str("Play the next episode now (Return)")));
        this.cancel.set_title("Cancel");
        this.cancel.setToolTip(Some(&NSString::from_str("Stay on this episode")));
        // VoiceOver meets the card as one thing called Up Next, with the
        // two lines and the two buttons inside it.
        this.card.setAccessibilityElement(true);
        // SAFETY: the role is a constant that AppKit provides.
        this.card.setAccessibilityRole(Some(unsafe { NSAccessibilityGroupRole }));
        this.card.setAccessibilityLabel(Some(&NSString::from_str("Up Next")));
        this.card.setHidden(true);
        this
    }

    pub fn view(&self) -> &NSView {
        &self.card
    }

    /// Fills the card in for `episode` and lays it out. Called once, when
    /// the episode that follows is known.
    pub fn offer(&self, main_thread: MainThreadMarker, episode: &Item) {
        let title = label(main_thread, theme::TITLE_FONT_SIZE, semibold(), &theme::text());
        title.setStringValue(&NSString::from_str(&episode.title));
        self.number.set(episode.index);
        // The one to press comes first, where reading starts.
        let buttons: [&NSView; 2] = [&self.play, &self.cancel];
        let buttons = row(&buttons, theme::CARD_BUTTON_SPACING, main_thread);
        let lines: [&NSView; 3] = [&self.countdown, &title, &buttons];
        let lines = NSStackView::stackViewWithViews(&NSArray::from_slice(&lines), main_thread);
        lines.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        lines.setAlignment(NSLayoutAttribute::Leading);
        lines.setSpacing(theme::CARD_LINE_SPACING);
        lines.setCustomSpacing_afterView(theme::CARD_PADDING, &title);

        let padding = theme::CARD_PADDING;
        add(&self.card, &lines);
        pin(&[
            lines.topAnchor().constraintEqualToAnchor_constant(&self.card.topAnchor(), padding),
            lines
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&self.card.bottomAnchor(), -padding),
            lines
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&self.card.leadingAnchor(), padding),
            lines
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&self.card.trailingAnchor(), -padding),
            self.card.widthAnchor().constraintLessThanOrEqualToConstant(theme::CARD_MAX_WIDTH),
        ]);
    }

    /// Shows the card, saying the episode starts in `seconds`.
    pub fn show(&self, seconds: u64) {
        // The number goes here and not before the title, which then has
        // the card's whole width and is seldom cut short.
        let line = match self.number.get() {
            Some(number) => format!("Episode {number} starts in {seconds} s"),
            None => format!("Up Next in {seconds} s"),
        };
        self.countdown.setStringValue(&NSString::from_str(&line));
        self.card.setHidden(false);
    }

    pub fn hide(&self) {
        self.card.setHidden(true);
    }
}
