//! Everything that sits on top of the video, and where each piece goes.
//!
//! This file only builds and places the pieces. What they do is decided by
//! the screen, which connects them once it exists.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAccessibility, NSColor, NSLayoutAttribute};
use objc2_app_kit::{
    NSFont, NSFontWeight, NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold, NSImage,
    NSImageSymbolConfiguration, NSLayoutConstraint, NSLayoutConstraintOrientation,
    NSLayoutPriorityDefaultLow, NSLayoutPriorityDragThatCannotResizeWindow, NSLineBreakMode,
    NSProgressIndicator, NSProgressIndicatorStyle, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSString};
use plex::model::{Item, Kind};

use super::badge::Badge;
use super::chip::{Chip, Style};
use super::layout::{add, cover, pin, row};
use super::pane::Pane;
use super::slider::{Reading, Slider};
use super::surface::{Material, Surface};
use super::theme;
use super::upnext::UpNext;

/// How much the bubble's wish to be over the pointer counts for in the
/// layout: less than anything else there. At the default strengths it
/// outweighed the window's wish to keep its size and a label's wish to
/// keep its width, so a bubble that could not be where it was asked to be
/// moved the scrubber and widened the window instead of giving way.
const FOLLOWS_POINTER: f32 = 1.0;

/// The loudest the volume slider goes, which is the file's own level.
pub const FULL_VOLUME: f64 = 100.0;

pub struct Controls {
    /// The back button and the title, in a capsule. Fades when idle.
    pub top: Retained<Pane>,
    /// The bar that holds the scrubber and the buttons. Fades when idle.
    pub bottom: Retained<Pane>,
    pub back: Retained<Chip>,
    /// The name of what is playing, and under it the line that places it.
    title: Retained<NSTextField>,
    detail: Retained<NSTextField>,
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
    pub badge: Badge,
}

impl Controls {
    /// Builds the controls for `item` on top of whatever `root` holds, on
    /// panels of `material`.
    pub fn new(
        main_thread: MainThreadMarker,
        root: &NSView,
        item: &Item,
        material: Material,
    ) -> Self {
        let button = |symbol, name, key| Chip::button(main_thread, symbol, name, key);
        let bubble = Chip::new(main_thread, Style::Caption);
        let scrubber =
            Slider::new(main_thread, "Position", 0.0, theme::SCRUBBER_STEP, Reading::Time);
        let controls = Self {
            top: Pane::new(main_thread),
            bottom: Pane::new(main_thread),
            back: button("chevron.left", "Back", "Esc"),
            title: label(main_thread, theme::TITLE_FONT_SIZE, semibold(), &theme::text()),
            detail: label(main_thread, theme::FONT_SIZE, regular(), &theme::secondary_text()),
            toggle: button("pause.fill", "Pause", "Space"),
            rewind: button("gobackward.10", "Back 10 seconds", "Shift \u{2190}"),
            forward: button("goforward.10", "Forward 10 seconds", "Shift \u{2192}"),
            previous: button("backward.end.fill", "Previous episode", "P"),
            next: button("forward.end.fill", "Next episode", "N"),
            mute: button("speaker.wave.2.fill", "Mute", "M"),
            volume: Slider::new(
                main_thread,
                "Volume",
                FULL_VOLUME,
                theme::VOLUME_STEP,
                Reading::Share,
            ),
            elapsed: label(main_thread, theme::CONTROL_FONT_SIZE, medium(), &theme::text()),
            remaining: Chip::new(main_thread, Style::Plain),
            bubble_x: bubble.centerXAnchor().constraintEqualToAnchor(&scrubber.leadingAnchor()),
            bubble,
            scrubber,
            speed: Chip::new(main_thread, Style::Plain),
            audio: button("waveform", "Audio track", "A"),
            subtitles: button("captions.bubble", "Subtitles", "C"),
            full_screen: button("arrow.up.left.and.arrow.down.right", "Full screen", "F"),
            skip: Chip::new(main_thread, Style::Solid),
            up_next: UpNext::new(main_thread, material),
            spinner: NSProgressIndicator::new(main_thread),
            badge: Badge::new(main_thread, material),
        };
        controls.back.set_symbol("chevron.left", theme::BACK_SYMBOL_SIZE);
        controls.toggle.set_symbol("pause.fill", theme::MAIN_SYMBOL_SIZE);
        let tooltip = |text| Some(NSString::from_str(text));
        controls.remaining.setToolTip(tooltip("Time left or full length").as_deref());
        controls.remaining.set_name("Time left or full length");
        controls
            .speed
            .setToolTip(tooltip("Playback speed (S slower, D faster, R normal)").as_deref());
        controls.speed.set_name("Playback speed");
        controls.skip.setToolTip(tooltip("Skip (Return)").as_deref());
        let is_episode = item.kind == Kind::Episode;
        controls.previous.setHidden(!is_episode);
        controls.next.setHidden(!is_episode);

        controls.set_heading(item, None);
        controls.place_top(main_thread, root, material);
        controls.place_bottom(main_thread, root, material);
        controls.place_loose(root);
        controls
    }

    /// Names `item` as what is playing. `note` is added to the line that
    /// places it.
    pub fn set_heading(&self, item: &Item, note: Option<&str>) {
        let (title, detail) = heading(item);
        let detail = match (detail, note) {
            (Some(detail), Some(note)) => Some(format!("{detail} \u{b7} {note}")),
            (detail, note) => detail.or(note.map(str::to_owned)),
        };
        self.title.setStringValue(&NSString::from_str(&title));
        self.detail.setStringValue(&NSString::from_str(detail.as_deref().unwrap_or_default()));
        self.detail.setHidden(detail.is_none());
    }

    /// The way out and the name of what is playing, in one capsule in the
    /// top corner, under the window's own buttons.
    fn place_top(&self, main_thread: MainThreadMarker, root: &NSView, material: Material) {
        let pane = &self.top;
        let surface = Surface::new(main_thread, material, theme::CAPSULE_RADIUS);
        // The title is cut short by the window's edge and by nothing less,
        // but does not get to widen the window either.
        self.title.setContentCompressionResistancePriority_forOrientation(
            NSLayoutPriorityDragThatCannotResizeWindow,
            NSLayoutConstraintOrientation::Horizontal,
        );
        let holder = surface.content();
        let titles: [&NSView; 2] = [&self.title, &self.detail];
        let titles = NSStackView::stackViewWithViews(&NSArray::from_slice(&titles), main_thread);
        titles.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        titles.setAlignment(NSLayoutAttribute::Leading);
        titles.setSpacing(theme::TITLE_SPACING);

        // Below the title bar, which holds the window's own buttons. In
        // full screen there is none and the safe area starts at the top.
        let below_title_bar = root.safeAreaLayoutGuide().topAnchor();
        add(root, pane);
        add(pane, surface.view());
        cover(pane, surface.view());
        add(holder, &self.back);
        add(holder, &titles);
        pin(&[
            pane.topAnchor().constraintEqualToAnchor_constant(&below_title_bar, theme::TOP_GAP),
            pane.leadingAnchor()
                .constraintEqualToAnchor_constant(&root.leadingAnchor(), theme::MARGIN),
            pane.trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&root.trailingAnchor(), -theme::MARGIN),
            pane.heightAnchor().constraintEqualToConstant(2.0 * theme::CAPSULE_RADIUS),
            self.back
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&holder.leadingAnchor(), theme::CAPSULE_INSET),
            self.back.centerYAnchor().constraintEqualToAnchor(&holder.centerYAnchor()),
            titles.leadingAnchor().constraintEqualToAnchor_constant(
                &self.back.trailingAnchor(),
                theme::CONTROL_SPACING,
            ),
            titles.centerYAnchor().constraintEqualToAnchor(&holder.centerYAnchor()),
            titles
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&holder.trailingAnchor(), -theme::CAPSULE_RADIUS),
        ]);
    }

    /// The scrubber, and under it the buttons in three groups, on one bar
    /// that floats above the bottom edge.
    fn place_bottom(&self, main_thread: MainThreadMarker, root: &NSView, material: Material) {
        let pane = &self.bottom;
        let surface = Surface::new(main_thread, material, theme::BAR_RADIUS);
        let bar = surface.content();
        let spacing = theme::CONTROL_SPACING;
        let sound: [&NSView; 2] = [&self.mute, &self.volume];
        let sound = row(&sound, spacing, main_thread);
        let transport: [&NSView; 5] =
            [&self.previous, &self.rewind, &self.toggle, &self.forward, &self.next];
        let transport = row(&transport, theme::TRANSPORT_SPACING, main_thread);
        let choices: [&NSView; 4] = [&self.speed, &self.audio, &self.subtitles, &self.full_screen];
        let choices = row(&choices, spacing, main_thread);

        let (leading, trailing) = (bar.leadingAnchor(), bar.trailingAnchor());
        let inset = theme::BAR_INSET;
        // The times stand in from the bar's edge as far as the symbols of
        // the buttons under them do.
        let text_inset = inset + theme::CHIP_INSET;
        let line = self.scrubber.centerYAnchor();
        let group = theme::GROUP_SPACING;
        add(root, pane);
        add(pane, surface.view());
        cover(pane, surface.view());
        add(bar, &transport);
        add(bar, &sound);
        add(bar, &choices);
        add(bar, &self.elapsed);
        add(bar, &self.remaining);
        add(bar, &self.scrubber);
        // As wide as the window lets it be, up to a width past which the
        // groups of buttons would drift too far apart to be one bar. The
        // wish to fill the window is weaker than the window's wish to keep
        // its size, or a window wider than the bar would shrink to fit it.
        let fills = pane
            .widthAnchor()
            .constraintEqualToAnchor_constant(&root.widthAnchor(), -2.0 * theme::MARGIN);
        fills.setPriority(NSLayoutPriorityDragThatCannotResizeWindow);
        pin(&[
            pane.bottomAnchor()
                .constraintEqualToAnchor_constant(&root.bottomAnchor(), -theme::BOTTOM_MARGIN),
            pane.centerXAnchor().constraintEqualToAnchor(&root.centerXAnchor()),
            pane.widthAnchor().constraintLessThanOrEqualToConstant(theme::BAR_MAX_WIDTH),
            fills,
            self.scrubber.topAnchor().constraintEqualToAnchor_constant(&bar.topAnchor(), inset),
            transport.centerXAnchor().constraintEqualToAnchor(&bar.centerXAnchor()),
            transport.topAnchor().constraintEqualToAnchor_constant(
                &self.scrubber.bottomAnchor(),
                theme::ROW_SPACING,
            ),
            bar.bottomAnchor().constraintEqualToAnchor_constant(&transport.bottomAnchor(), inset),
            sound.leadingAnchor().constraintEqualToAnchor_constant(&leading, inset),
            sound.centerYAnchor().constraintEqualToAnchor(&transport.centerYAnchor()),
            self.volume.widthAnchor().constraintEqualToConstant(theme::VOLUME_WIDTH),
            // The play symbol is wider than the pause symbol. Left to fit
            // its content, the chip would push its neighbours aside on
            // every pause.
            self.toggle.widthAnchor().constraintEqualToConstant(theme::TOGGLE_WIDTH),
            choices.trailingAnchor().constraintEqualToAnchor_constant(&trailing, -inset),
            choices.centerYAnchor().constraintEqualToAnchor(&transport.centerYAnchor()),
            self.elapsed.leadingAnchor().constraintEqualToAnchor_constant(&leading, text_inset),
            self.elapsed.centerYAnchor().constraintEqualToAnchor(&line),
            self.remaining
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&trailing, -(text_inset - theme::CHIP_PADDING)),
            self.remaining.centerYAnchor().constraintEqualToAnchor(&line),
            self.scrubber
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&self.elapsed.trailingAnchor(), group),
            self.scrubber.trailingAnchor().constraintEqualToAnchor_constant(
                &self.remaining.leadingAnchor(),
                -(group - theme::CHIP_PADDING),
            ),
        ]);
    }

    /// Places the pieces that come and go by themselves, whether or not
    /// the panes are showing.
    fn place_loose(&self, root: &NSView) {
        let below_title_bar = root.safeAreaLayoutGuide().topAnchor();
        let (leading, trailing) = (root.leadingAnchor(), root.trailingAnchor());
        self.skip.setHidden(true);
        self.bubble.setHidden(true);
        self.bubble.limit_width(theme::NOTICE_MAX_WIDTH);
        self.bubble_x.setPriority(FOLLOWS_POINTER);
        self.spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        self.spinner.setIndeterminate(true);
        self.spinner.setDisplayedWhenStopped(false);
        self.spinner.setAccessibilityLabel(Some(&NSString::from_str("Loading")));
        add(root, &self.skip);
        add(root, self.up_next.view());
        add(root, self.badge.view());
        add(root, &self.spinner);
        add(root, &self.bubble);
        pin(&[
            self.spinner.centerXAnchor().constraintEqualToAnchor(&root.centerXAnchor()),
            self.spinner.centerYAnchor().constraintEqualToAnchor(&root.centerYAnchor()),
            self.skip.trailingAnchor().constraintEqualToAnchor(&self.bottom.trailingAnchor()),
            self.skip
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&self.bottom.topAnchor(), -theme::SKIP_GAP),
            self.up_next
                .view()
                .trailingAnchor()
                .constraintEqualToAnchor(&self.skip.trailingAnchor()),
            self.up_next.view().bottomAnchor().constraintEqualToAnchor(&self.skip.bottomAnchor()),
            // In the middle, where the eyes are, and at the top, where it
            // covers the least of the picture.
            self.badge.view().centerXAnchor().constraintEqualToAnchor(&root.centerXAnchor()),
            self.badge
                .view()
                .topAnchor()
                .constraintEqualToAnchor_constant(&below_title_bar, theme::TOP_GAP),
            self.bubble_x.clone(),
            self.bubble
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&self.bottom.topAnchor(), -theme::BUBBLE_GAP),
            self.bubble
                .leadingAnchor()
                .constraintGreaterThanOrEqualToAnchor_constant(&leading, theme::MARGIN),
            self.bubble
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&trailing, -theme::MARGIN),
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

/// The SF Symbol called `name`, `size` points high.
pub fn symbol(name: &str, size: f64) -> Option<Retained<NSImage>> {
    let size = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, medium());
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
        .and_then(|image| image.imageWithSymbolConfiguration(&size))
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

pub fn regular() -> NSFontWeight {
    // SAFETY: the weight is a constant that AppKit provides.
    unsafe { NSFontWeightRegular }
}

pub fn semibold() -> NSFontWeight {
    // SAFETY: the weight is a constant that AppKit provides.
    unsafe { NSFontWeightSemibold }
}
