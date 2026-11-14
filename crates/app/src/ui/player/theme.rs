//! Sizes, spacing, timing and colours of the player screen. No other file
//! of the player holds a literal measurement.

use objc2::rc::Retained;
use objc2_app_kit::NSColor;

/// Space between the window's edge and the nearest control.
pub const MARGIN: f64 = 20.0;
/// Space between a pane's content and the edge where its shade runs out.
pub const SHADE_RUNOUT: f64 = 56.0;
/// How dark a pane's shade is at the window's edge.
pub const SHADE_ALPHA: f64 = 0.85;
/// How dark it is halfway to where it runs out, which is about where the
/// scrubber and the title are. White on a white picture under this much
/// black has a contrast of over 5 to 1, where a shade that thinned evenly
/// from the edge left the times at about 3.5 to 1.
pub const SHADE_ALPHA_MIDWAY: f64 = 0.6;
/// How many steps the shade is drawn in, which is enough for its curve to
/// show no corners.
pub const SHADE_STOPS: u32 = 12;

/// How far a symbol sits inside the edge of its chip. Rows of chips hang
/// over the margin by this much, so that what is seen lines up with it.
pub const CHIP_INSET: f64 = 10.0;
/// Space between the controls and the bottom edge of the window.
pub const BOTTOM_MARGIN: f64 = 12.0;
/// Space between the title bar, or the top of the screen, and the controls.
pub const TOP_GAP: f64 = 8.0;

pub const ROW_SPACING: f64 = 4.0;
pub const CONTROL_SPACING: f64 = 4.0;
/// Between the buttons that move through the film, which are the ones
/// reached for most and so get the most room.
pub const TRANSPORT_SPACING: f64 = 10.0;
pub const GROUP_SPACING: f64 = 16.0;

/// Width and height of a control that holds one symbol, and the height of
/// one that holds text. Apple asks for at least 28 points to click on; the
/// best players give more.
pub const CHIP_SIZE: f64 = 42.0;
/// Width of the play and pause button, which fits the wider of its two
/// symbols with the same padding as any chip.
pub const TOGGLE_WIDTH: f64 = 56.0;
pub const CHIP_RADIUS: f64 = 10.0;
pub const CHIP_PADDING: f64 = 12.0;
pub const CHIP_CONTENT_SPACING: f64 = 6.0;
/// The height of a chip that only says something.
pub const CAPTION_HEIGHT: f64 = 28.0;
pub const SYMBOL_SIZE: f64 = 18.0;
/// The play and pause symbol, which is the control used most.
pub const MAIN_SYMBOL_SIZE: f64 = 28.0;

/// The way out, which has a corner to itself and is drawn to be found.
pub const BACK_SYMBOL_SIZE: f64 = 22.0;

pub const FONT_SIZE: f64 = 13.0;
pub const TITLE_FONT_SIZE: f64 = 16.0;
pub const TITLE_SPACING: f64 = 1.0;

/// The height a slider can be grabbed in, far more than what it draws.
pub const SLIDER_HEIGHT: f64 = 24.0;
pub const TRACK_THICKNESS: f64 = 5.0;
/// The track swells under the pointer to show it can be grabbed.
pub const TRACK_THICKNESS_HOVERED: f64 = 8.0;
pub const KNOB_RADIUS: f64 = 7.0;
/// The knob under the pointer, or in its grip.
pub const KNOB_RADIUS_HELD: f64 = 9.0;
/// The line across the track where the pointer is over it.
pub const POINTER_WIDTH: f64 = 2.0;
pub const POINTER_HEIGHT: f64 = 14.0;
/// The gap in the track where a chapter starts.
pub const MARK_WIDTH: f64 = 2.0;
pub const VOLUME_WIDTH: f64 = 84.0;
/// How far VoiceOver moves the scrubber at a time, in seconds, and the
/// volume, out of 100.
pub const SCRUBBER_STEP: f64 = 10.0;
pub const VOLUME_STEP: f64 = 5.0;
pub const BUBBLE_GAP: f64 = 2.0;
/// Space between the scrubber and the Skip Intro button above it.
pub const SKIP_GAP: f64 = 16.0;

pub const CARD_RADIUS: f64 = 16.0;
pub const CARD_PADDING: f64 = 16.0;
pub const CARD_LINE_SPACING: f64 = 2.0;
pub const CARD_BUTTON_SPACING: f64 = 8.0;
/// A longer episode title than fits in this ends in an ellipsis.
pub const CARD_MAX_WIDTH: f64 = 360.0;

/// Seconds the controls take to appear. Short, so that they feel instant.
pub const FADE_IN: f64 = 0.12;
/// Seconds they take to go away.
pub const FADE_OUT: f64 = 0.35;

/// How much is left of a control that can do nothing right now.
pub const DISABLED_ALPHA: f64 = 0.35;

pub fn text() -> Retained<NSColor> {
    NSColor::whiteColor()
}

pub fn secondary_text() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(1.0, 0.7)
}

/// Behind a control the pointer is over.
pub fn hovered() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(1.0, 0.16)
}

/// Behind a quiet button the pointer is over.
pub fn quiet_hovered() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(1.0, 0.22)
}

/// Behind a control that is being pressed.
pub fn pressed() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(1.0, 0.28)
}

/// A control that must be seen over any picture, such as Skip Intro.
pub fn solid() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(1.0, 0.88)
}

pub fn solid_hovered() -> Retained<NSColor> {
    NSColor::whiteColor()
}

pub fn solid_pressed() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(0.75, 0.92)
}

pub fn text_on_solid() -> Retained<NSColor> {
    NSColor::blackColor()
}

/// Behind text that floats over the picture with no shade under it.
pub fn backdrop() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(0.0, 0.6)
}

/// Behind a card that floats over the picture and holds buttons.
pub fn card() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(0.0, 0.72)
}

pub fn shade(alpha: f64) -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(0.0, alpha)
}

/// The part of a slider's track that is not filled.
pub fn track() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(1.0, 0.25)
}

/// The part of the scrubber's track that is ready to play.
pub fn track_loaded() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(1.0, 0.45)
}

pub fn track_filled() -> Retained<NSColor> {
    NSColor::whiteColor()
}

/// Under the whole track. Over a white picture the shade alone leaves the
/// unfilled part of the track too close to what is behind it.
pub fn track_bed() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(0.0, 0.3)
}

/// From the start of the track to where the pointer is over it.
pub fn track_pointed() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(1.0, 0.3)
}

/// Around the knob and the pointer's line, which are white on white where
/// they cross the filled track.
pub fn knob_rim() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(0.0, 0.35)
}
