//! Everything that sits on top of the video, and where each piece goes.
//!
//! This file only builds and places the pieces. What they do is decided by
//! the screen, which connects them once it exists.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSLayoutAttribute};
use objc2_app_kit::{
    NSFont, NSFontWeight, NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold,
    NSLayoutConstraint, NSLayoutConstraintOrientation, NSLayoutPriorityDefaultHigh,
    NSLayoutPriorityDefaultLow, NSLineBreakMode, NSProgressIndicator, NSProgressIndicatorStyle,
    NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSString};
use plex::model::{Item, Kind};

use super::chip::{Chip, Style};
use super::layout::{add, pin, row};
use super::pane::{Edge, Pane};
use super::slider::Slider;
use super::theme;
use super::upnext::UpNext;

/// The loudest the volume slider goes, which is the file's own level.
pub const FULL_VOLUME: f64 = 100.0;

pub struct Controls {
    /// The back button and the title. Fades when idle.
    pub top: Retained<Pane>,
    /// The scrubber and the buttons. Fades when idle.
    pub bottom: Retained<Pane>,
    pub back: Retained<Chip>,
    pub toggle: Retained<Chip>,
    pub rewind: Retained<Chip>,
    pub forward: Retained<Chip>,
    /// Shown for episodes only, as is `next`.
    pub previous: Retained<Chip>,
    pub next: Retained<Chip>,
    pub mute: Retained<Chip>,
    pub volume: Retained<Slider>,
    pub elapsed: Retained<NSTextField>,
    pub scrubber: Retained<Slider>,
    /// The time left or the full length. A click switches between them.
    pub remaining: Retained<Chip>,
    /// Says what time the pointer is over on the scrubber.
    pub bubble: Retained<Chip>,
    /// Holds the bubble over the pointer: its constant is the distance from
    /// the scrubber's left edge.
    pub bubble_x: Retained<NSLayoutConstraint>,
    pub speed: Retained<Chip>,
    pub audio: Retained<Chip>,
    pub subtitles: Retained<Chip>,
    pub full_screen: Retained<Chip>,
    /// Offers to jump past the intro or the credits while they play.
    pub skip: Retained<Chip>,
    /// Offers the next episode as this one ends, in the skip button's place.
    pub up_next: UpNext,
    /// Turns while the picture is on its way: at the start, and whenever
    /// playback stalls waiting for the network.
    pub spinner: Retained<NSProgressIndicator>,
    /// Briefly says what a key press did.
    pub notice: Retained<Chip>,
}

impl Controls {
    /// Builds the controls for `item` on top of whatever `root` holds.
    pub fn new(main_thread: MainThreadMarker, root: &NSView, item: &Item) -> Self {
        let button = |symbol, tooltip| Chip::button(main_thread, symbol, tooltip);
        let bubble = Chip::new(main_thread, Style::Caption);
        let scrubber = Slider::new(main_thread, 0.0);
        let controls = Self {
            top: Pane::new(main_thread, Edge::Top),
            bottom: Pane::new(main_thread, Edge::Bottom),
            back: button("chevron.left", "Back (Esc)"),
            toggle: button("pause.fill", "Pause or play (Space)"),
            rewind: button("gobackward.10", "Back 10 seconds (Shift \u{2190})"),
            forward: button("goforward.10", "Forward 10 seconds (Shift \u{2192})"),
            previous: button("backward.end.fill", "Previous episode (P)"),
            next: button("forward.end.fill", "Next episode (N)"),
            mute: button("speaker.wave.2.fill", "Mute (M)"),
            volume: Slider::new(main_thread, FULL_VOLUME),
            elapsed: label(main_thread, theme::FONT_SIZE, regular(), &theme::text()),
            remaining: Chip::new(main_thread, Style::Plain),
            bubble_x: bubble.centerXAnchor().constraintEqualToAnchor(&scrubber.leadingAnchor()),
            bubble,
            scrubber,
            speed: Chip::new(main_thread, Style::Plain),
            audio: button("waveform", "Audio track (A)"),
            subtitles: button("captions.bubble", "Subtitles (C)"),
            full_screen: button("arrow.up.left.and.arrow.down.right", "Full screen (F)"),
            skip: Chip::new(main_thread, Style::Solid),
            up_next: UpNext::new(main_thread),
            spinner: NSProgressIndicator::new(main_thread),
            notice: Chip::new(main_thread, Style::Caption),
        };
        controls.back.set_symbol("chevron.left", theme::MAIN_SYMBOL_SIZE);
        controls.toggle.set_symbol("pause.fill", theme::MAIN_SYMBOL_SIZE);
        let tooltip = |text| Some(NSString::from_str(text));
        controls.remaining.setToolTip(tooltip("Time left or full length").as_deref());
        controls.speed.setToolTip(tooltip("Playback speed (S, D, R)").as_deref());
        let is_episode = item.kind == Kind::Episode;
        controls.previous.setHidden(!is_episode);
        controls.next.setHidden(!is_episode);

        controls.place_top(main_thread, root, item);
        controls.place_bottom(main_thread, root);
        controls.place_loose(root);
        controls
    }

    fn place_top(&self, main_thread: MainThreadMarker, root: &NSView, item: &Item) {
        let pane = &self.top;
        let (title, detail) = heading(item);
        let title_label = label(main_thread, theme::TITLE_FONT_SIZE, semibold(), &theme::text());
        title_label.setStringValue(&NSString::from_str(&title));
        let detail_label =
            label(main_thread, theme::FONT_SIZE, regular(), &theme::secondary_text());
        detail_label.setStringValue(&NSString::from_str(&detail.unwrap_or_default()));
        detail_label.setHidden(detail_label.stringValue().is_empty());
        let titles: [&NSView; 2] = [&title_label, &detail_label];
        let titles = NSStackView::stackViewWithViews(&NSArray::from_slice(&titles), main_thread);
        titles.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        titles.setAlignment(NSLayoutAttribute::Leading);
        titles.setSpacing(theme::TITLE_SPACING);

        let hang = theme::MARGIN - theme::CHIP_INSET;
        // Below the title bar, which holds the window's own buttons. In
        // full screen there is none and the safe area starts at the top.
        let below_title_bar = root.safeAreaLayoutGuide().topAnchor();
        add(root, pane);
        add(pane, &self.back);
        add(pane, &titles);
        pin(&[
            pane.topAnchor().constraintEqualToAnchor(&root.topAnchor()),
            pane.leadingAnchor().constraintEqualToAnchor(&root.leadingAnchor()),
            pane.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()),
            self.back.leadingAnchor().constraintEqualToAnchor_constant(&pane.leadingAnchor(), hang),
            self.back
                .topAnchor()
                .constraintEqualToAnchor_constant(&below_title_bar, theme::TOP_GAP),
            pane.bottomAnchor()
                .constraintEqualToAnchor_constant(&self.back.bottomAnchor(), theme::SHADE_RUNOUT),
            titles.leadingAnchor().constraintEqualToAnchor_constant(
                &self.back.trailingAnchor(),
                theme::CONTROL_SPACING,
            ),
            titles.centerYAnchor().constraintEqualToAnchor(&self.back.centerYAnchor()),
            titles
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&pane.trailingAnchor(), -theme::MARGIN),
        ]);
    }

    fn place_bottom(&self, main_thread: MainThreadMarker, root: &NSView) {
        let pane = &self.bottom;
        let spacing = theme::CONTROL_SPACING;
        let sound: [&NSView; 2] = [&self.mute, &self.volume];
        let sound = row(&sound, spacing, main_thread);
        let transport: [&NSView; 5] =
            [&self.previous, &self.rewind, &self.toggle, &self.forward, &self.next];
        let transport = row(&transport, spacing, main_thread);
        let choices: [&NSView; 4] = [&self.speed, &self.audio, &self.subtitles, &self.full_screen];
        let choices = row(&choices, spacing, main_thread);

        let (leading, trailing) = (pane.leadingAnchor(), pane.trailingAnchor());
        let hang = theme::MARGIN - theme::CHIP_INSET;
        let line = self.scrubber.centerYAnchor();
        let group = theme::GROUP_SPACING;
        add(root, pane);
        add(pane, &transport);
        add(pane, &sound);
        add(pane, &choices);
        add(pane, &self.elapsed);
        add(pane, &self.remaining);
        add(pane, &self.scrubber);
        add(pane, &self.bubble);
        // The bubble follows the pointer, but stops short of the window's
        // edge instead of being cut off by it.
        self.bubble_x.setPriority(NSLayoutPriorityDefaultHigh);
        self.bubble.setHidden(true);
        pin(&[
            pane.bottomAnchor().constraintEqualToAnchor(&root.bottomAnchor()),
            pane.leadingAnchor().constraintEqualToAnchor(&root.leadingAnchor()),
            pane.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()),
            transport.centerXAnchor().constraintEqualToAnchor(&pane.centerXAnchor()),
            transport
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&pane.bottomAnchor(), -theme::BOTTOM_MARGIN),
            sound.leadingAnchor().constraintEqualToAnchor_constant(&leading, hang),
            sound.centerYAnchor().constraintEqualToAnchor(&transport.centerYAnchor()),
            self.volume.widthAnchor().constraintEqualToConstant(theme::VOLUME_WIDTH),
            choices.trailingAnchor().constraintEqualToAnchor_constant(&trailing, -hang),
            choices.centerYAnchor().constraintEqualToAnchor(&transport.centerYAnchor()),
            self.elapsed.leadingAnchor().constraintEqualToAnchor_constant(&leading, theme::MARGIN),
            self.elapsed.centerYAnchor().constraintEqualToAnchor(&line),
            self.remaining.trailingAnchor().constraintEqualToAnchor_constant(&trailing, -hang),
            self.remaining.centerYAnchor().constraintEqualToAnchor(&line),
            self.scrubber
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&self.elapsed.trailingAnchor(), group),
            self.scrubber.trailingAnchor().constraintEqualToAnchor_constant(
                &self.remaining.leadingAnchor(),
                -(group - theme::CHIP_PADDING),
            ),
            self.scrubber
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&transport.topAnchor(), -theme::ROW_SPACING),
            pane.topAnchor()
                .constraintEqualToAnchor_constant(&self.scrubber.topAnchor(), -theme::SHADE_RUNOUT),
            self.bubble_x.clone(),
            self.bubble
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&self.scrubber.topAnchor(), -theme::BUBBLE_GAP),
            self.bubble
                .leadingAnchor()
                .constraintGreaterThanOrEqualToAnchor_constant(&leading, theme::MARGIN),
            self.bubble
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&trailing, -theme::MARGIN),
        ]);
    }

    /// Places the pieces that come and go by themselves, whether or not
    /// the panes are showing.
    fn place_loose(&self, root: &NSView) {
        let trailing = root.trailingAnchor();
        let below_title_bar = root.safeAreaLayoutGuide().topAnchor();
        self.skip.setHidden(true);
        self.notice.setAlphaValue(0.0);
        self.spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        self.spinner.setIndeterminate(true);
        self.spinner.setDisplayedWhenStopped(false);
        add(root, &self.skip);
        add(root, self.up_next.view());
        add(root, &self.notice);
        add(root, &self.spinner);
        pin(&[
            self.spinner.centerXAnchor().constraintEqualToAnchor(&root.centerXAnchor()),
            self.spinner.centerYAnchor().constraintEqualToAnchor(&root.centerYAnchor()),
            self.skip.trailingAnchor().constraintEqualToAnchor_constant(&trailing, -theme::MARGIN),
            self.skip.bottomAnchor().constraintEqualToAnchor_constant(
                &self.bottom.topAnchor(),
                theme::SHADE_RUNOUT - theme::SKIP_GAP,
            ),
            self.up_next
                .view()
                .trailingAnchor()
                .constraintEqualToAnchor(&self.skip.trailingAnchor()),
            self.up_next.view().bottomAnchor().constraintEqualToAnchor(&self.skip.bottomAnchor()),
            self.notice
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&trailing, -theme::MARGIN),
            self.notice
                .topAnchor()
                .constraintEqualToAnchor_constant(&below_title_bar, theme::TOP_GAP),
        ]);
    }
}

/// What the player calls the item, and a second line that places it: an
/// episode by its show, season and number, anything else by its year.
fn heading(item: &Item) -> (String, Option<String>) {
    let detail = match (item.kind, &item.grandparent_title, item.parent_index, item.index) {
        (Kind::Episode, Some(show), Some(season), Some(episode)) => {
            Some(format!("{show} \u{b7} S{season} E{episode}"))
        }
        _ => item.year.map(|year| year.to_string()),
    };
    (item.title.clone(), detail)
}

/// A line of text that gives way, by ending in an ellipsis, when there is
/// no room for all of it.
pub fn label(
    main_thread: MainThreadMarker,
    size: f64,
    weight: NSFontWeight,
    color: &NSColor,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::new(), main_thread);
    // Digits of equal width keep a running time from shifting what is
    // next to it every second.
    label.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(size, weight)));
    label.setTextColor(Some(color));
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    label.setContentCompressionResistancePriority_forOrientation(
        NSLayoutPriorityDefaultLow,
        NSLayoutConstraintOrientation::Horizontal,
    );
    label
}

pub fn medium() -> NSFontWeight {
    // SAFETY: the weight is a constant that AppKit provides.
    unsafe { NSFontWeightMedium }
}

fn regular() -> NSFontWeight {
    // SAFETY: the weight is a constant that AppKit provides.
    unsafe { NSFontWeightRegular }
}

fn semibold() -> NSFontWeight {
    // SAFETY: the weight is a constant that AppKit provides.
    unsafe { NSFontWeightSemibold }
}
