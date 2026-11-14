//! The card that offers the next episode as this one ends.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSEvent, NSLayoutAttribute, NSStackView, NSTextAlignment, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSRect, NSString};
use plex::model::Item;

use super::chip::{Chip, Style};
use super::controls::{label, medium};
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
    pub play: Retained<Chip>,
    pub cancel: Retained<Chip>,
}

impl UpNext {
    /// The card, hidden until there is an episode to offer.
    pub fn new(main_thread: MainThreadMarker) -> Self {
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let card: Retained<Card> =
            unsafe { msg_send![Card::alloc(main_thread), initWithFrame: NSRect::ZERO] };
        let this = Self {
            card,
            countdown: line(main_thread, &theme::secondary_text()),
            play: Chip::new(main_thread, Style::Solid),
            cancel: Chip::new(main_thread, Style::Plain),
        };
        this.play.set_title("Play Now");
        this.cancel.set_title("Cancel");
        this.card.setHidden(true);
        this
    }

    pub fn view(&self) -> &NSView {
        &self.card
    }

    /// Fills the card in for `episode` and lays it out. Called once, when
    /// the episode that follows is known.
    pub fn offer(&self, main_thread: MainThreadMarker, episode: &Item) {
        let title = line(main_thread, &theme::text());
        title.setStringValue(&NSString::from_str(&name(episode)));
        let buttons: [&NSView; 2] = [&self.cancel, &self.play];
        let buttons = row(&buttons, theme::CONTROL_SPACING, main_thread);
        let lines: [&NSView; 3] = [&self.countdown, &title, &buttons];
        let lines = NSStackView::stackViewWithViews(&NSArray::from_slice(&lines), main_thread);
        lines.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        lines.setAlignment(NSLayoutAttribute::Trailing);
        lines.setSpacing(theme::TITLE_SPACING);
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
        self.countdown.setStringValue(&NSString::from_str(&format!("Up Next in {seconds}")));
        self.card.setHidden(false);
    }

    pub fn hide(&self) {
        self.card.setHidden(true);
    }
}

/// `S1 E2 · Earth Skills`, or only the title of an episode with no number.
fn name(episode: &Item) -> String {
    match (episode.parent_index, episode.index) {
        (Some(season), Some(number)) => format!("S{season} E{number} \u{b7} {}", episode.title),
        _ => episode.title.clone(),
    }
}

/// One line of the card's text, set against its right edge like the
/// buttons under it.
fn line(main_thread: MainThreadMarker, color: &NSColor) -> Retained<NSTextField> {
    let line = label(main_thread, theme::FONT_SIZE, medium(), color);
    line.setAlignment(NSTextAlignment::Right);
    line
}
