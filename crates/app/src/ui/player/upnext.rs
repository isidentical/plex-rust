//! The card that offers the next episode as this one ends.

use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityGroupRole, NSLayoutAttribute, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSString};
use plex::model::Item;

use super::chip::{Chip, Style};
use super::controls::{label, medium, regular, semibold};
use super::layout::{add, pin, row};
use super::surface::{Material, Surface};
use super::theme;
use crate::images::Images;
use crate::ui::artwork::ArtView;

pub struct UpNext {
    surface: Surface,
    /// A still from the episode on offer.
    still: ArtView,
    countdown: Retained<NSTextField>,
    /// Names the episode on offer.
    title: Retained<NSTextField>,
    /// Its number, and its season when that is not the one playing.
    place: Retained<NSTextField>,
    pub play: Retained<Chip>,
    pub cancel: Retained<Chip>,
}

impl UpNext {
    /// The card, hidden until there is an episode to offer.
    pub fn new(main_thread: MainThreadMarker, material: Material) -> Self {
        let secondary = theme::secondary_text();
        let this = Self {
            surface: Surface::new(main_thread, material, theme::CARD_RADIUS),
            still: ArtView::new(main_thread, theme::STILL_RADIUS),
            countdown: label(main_thread, theme::FONT_SIZE, medium(), &secondary),
            title: label(main_thread, theme::TITLE_FONT_SIZE, semibold(), &theme::text()),
            place: label(main_thread, theme::FONT_SIZE, regular(), &secondary),
            play: Chip::new(main_thread, Style::Solid),
            cancel: Chip::new(main_thread, Style::Quiet),
        };
        this.still.set_flat();
        this.play.set_symbol("play.fill", theme::FONT_SIZE);
        this.play.set_title("Play Now");
        this.play.setToolTip(Some(&NSString::from_str("Play the next episode now (Return)")));
        this.cancel.set_title("Cancel");
        this.cancel.setToolTip(Some(&NSString::from_str("Stay on this episode (Esc)")));
        // VoiceOver meets the card as one thing called Up Next, with the
        // lines and the two buttons inside it.
        let card = this.view();
        card.setAccessibilityElement(true);
        // SAFETY: the role is a constant that AppKit provides.
        card.setAccessibilityRole(Some(unsafe { NSAccessibilityGroupRole }));
        card.setAccessibilityLabel(Some(&NSString::from_str("Up Next")));
        card.setHidden(true);
        this.lay_out(main_thread);
        this
    }

    pub fn view(&self) -> &NSView {
        self.surface.view()
    }

    /// Makes `episode` the one the card offers to someone watching
    /// `playing`.
    pub fn offer(&self, episode: &Item, playing: &Item, images: &Rc<Images>) {
        self.title.setStringValue(&NSString::from_str(&episode.title));
        let place = place(episode, playing);
        self.place.setStringValue(&NSString::from_str(place.as_deref().unwrap_or_default()));
        self.place.setHidden(place.is_none());
        let size = crate::ui::theme::LANDSCAPE_REQUEST;
        self.still.show(episode.thumb.as_deref(), size, Some("tv"), images);
    }

    fn lay_out(&self, main_thread: MainThreadMarker) {
        let (card, still) = (self.surface.content(), self.still.view());
        // The one to press comes first, where reading starts.
        let buttons: [&NSView; 2] = [&self.play, &self.cancel];
        let buttons = row(&buttons, theme::CARD_BUTTON_SPACING, main_thread);
        let lines: [&NSView; 4] = [&self.countdown, &self.title, &self.place, &buttons];
        let lines = NSStackView::stackViewWithViews(&NSArray::from_slice(&lines), main_thread);
        lines.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        lines.setAlignment(NSLayoutAttribute::Leading);
        lines.setSpacing(theme::CARD_LINE_SPACING);
        lines.setCustomSpacing_afterView(theme::CARD_PADDING, &self.place);

        let padding = theme::CARD_PADDING;
        add(card, still);
        add(card, &lines);
        pin(&[
            still.leadingAnchor().constraintEqualToAnchor_constant(&card.leadingAnchor(), padding),
            still.centerYAnchor().constraintEqualToAnchor(&card.centerYAnchor()),
            still.widthAnchor().constraintEqualToConstant(theme::STILL_WIDTH),
            still.heightAnchor().constraintEqualToConstant(theme::STILL_HEIGHT),
            lines.topAnchor().constraintEqualToAnchor_constant(&card.topAnchor(), padding),
            lines.bottomAnchor().constraintEqualToAnchor_constant(&card.bottomAnchor(), -padding),
            lines
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&still.trailingAnchor(), padding),
            lines
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&card.trailingAnchor(), -padding),
            card.widthAnchor().constraintLessThanOrEqualToConstant(theme::CARD_MAX_WIDTH),
        ]);
    }

    /// Shows the card, saying the episode starts in `seconds`.
    pub fn show(&self, seconds: u64) {
        self.countdown.setStringValue(&NSString::from_str(&format!("Up Next in {seconds} s")));
        self.view().setHidden(false);
    }

    pub fn hide(&self) {
        self.view().setHidden(true);
    }
}

/// Where `episode` is in its show, for someone watching `playing`: its
/// number, and its season too when it is not the one being watched. "Episode
/// 1" alone, after the last episode of a season, reads as a mistake.
fn place(episode: &Item, playing: &Item) -> Option<String> {
    let number = episode.index?;
    match episode.parent_index {
        Some(season) if episode.parent_index != playing.parent_index => {
            Some(format!("Season {season} \u{b7} Episode {number}"))
        }
        _ => Some(format!("Episode {number}")),
    }
}

#[cfg(test)]
mod tests {
    use plex::model::Item;

    use super::place;

    fn episode(season: u32, number: u32) -> Item {
        serde_json::from_value(serde_json::json!({
            "ratingKey": "1", "type": "episode", "title": "An episode",
            "parentIndex": season, "index": number,
        }))
        .unwrap()
    }

    #[test]
    fn the_season_is_named_only_when_the_next_episode_starts_one() {
        assert_eq!(place(&episode(1, 7), &episode(1, 6)).as_deref(), Some("Episode 7"));
        assert_eq!(
            place(&episode(2, 1), &episode(1, 22)).as_deref(),
            Some("Season 2 \u{b7} Episode 1")
        );
    }
}
