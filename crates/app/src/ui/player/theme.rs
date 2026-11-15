//! Sizes, spacing, timing and colours of the player screen. No other file
//! of the player holds a literal measurement.
//!
//! Type, space, corners and timing come from the scale the pages use, in
//! [`scale`]. What is measured here is what only a player has: its
//! controls, its sliders and the shade under them.

use objc2::rc::Retained;
use objc2_app_kit::NSColor;

use crate::ui::theme as scale;

/// Space between the window's edge and the nearest control.
pub const MARGIN: f64 = scale::SPACE_XL;
/// The bar of controls: how far its buttons stand in from its edge, how
/// round its corners are, which is concentric with those buttons, and
/// how wide it gets in a very wide window.
pub const BAR_INSET: f64 = scale::SPACE_S;
pub const BAR_RADIUS: f64 = CHIP_RADIUS + BAR_INSET;
pub const BAR_MAX_WIDTH: f64 = 1400.0;
/// The mini player: how wide it opens, the narrowest it may be made, how
/// round its corners are, how far its buttons float above its bottom edge
/// and how far it first stands in from the corner of the display.
pub const MINI_WIDTH: f64 = 400.0;
pub const MINI_MIN_WIDTH: f64 = 240.0;
pub const MINI_RADIUS: f64 = scale::RADIUS_L;
pub const MINI_BAR_GAP: f64 = scale::SPACE_S;
pub const MINI_SCREEN_GAP: f64 = scale::SPACE_XL;
/// The capsule that holds the back button and the title: half its height,
/// and how far the back button stands in from its end.
pub const CAPSULE_RADIUS: f64 = 27.0;
pub const CAPSULE_INSET: f64 = 6.0;

/// How far a symbol sits inside the edge of its chip.
pub const CHIP_INSET: f64 = 10.0;
/// Space between the controls and the bottom edge of the window.
pub const BOTTOM_MARGIN: f64 = scale::SPACE_L;
/// Space between the title bar, or the top of the screen, and the controls.
pub const TOP_GAP: f64 = scale::SPACE_S;

pub const CONTROL_SPACING: f64 = scale::SPACE_XS;
/// Between the buttons that move through the film, which are the ones
/// reached for most and so get the most room.
pub const TRANSPORT_SPACING: f64 = scale::SPACE_M;
pub const GROUP_SPACING: f64 = scale::SPACE_L;

/// Width and height of a control that holds one symbol, and the height of
/// one that holds text: the 44 points Apple asks for where a finger
/// lands, which a Mac with a touch screen makes this.
pub const CHIP_SIZE: f64 = 44.0;
/// Width of the play and pause button, which fits the wider of its two
/// symbols with the same padding as any chip.
pub const TOGGLE_WIDTH: f64 = 56.0;
pub const CHIP_RADIUS: f64 = scale::RADIUS_M;
pub const CHIP_PADDING: f64 = scale::SPACE_M;
pub const RIM_WIDTH: f64 = 1.0;
/// The ring around the control the keyboard is on.
pub const FOCUS_WIDTH: f64 = 2.5;
pub const CHIP_CONTENT_SPACING: f64 = scale::PILL_GLYPH_GAP;
/// The badge that says what a key press did is read at a glance, often
/// from the sofa and with nothing else on screen, so it is larger than a
/// label.
pub const BADGE_HEIGHT: f64 = 48.0;
pub const BADGE_FONT_SIZE: f64 = scale::FONT_TITLE;
pub const BADGE_SYMBOL_SIZE: f64 = scale::FONT_TITLE;
pub const BADGE_PADDING: f64 = scale::SPACE_L;
pub const BADGE_SPACING: f64 = scale::SPACE_S;
/// The bar in the badge that shows how loud or how fast.
pub const METER_WIDTH: f64 = 96.0;
pub const METER_HEIGHT: f64 = 5.0;
/// A longer notice than this, such as a track with a long name, ends in
/// an ellipsis.
pub const NOTICE_MAX_WIDTH: f64 = 480.0;
pub const SYMBOL_SIZE: f64 = 18.0;
/// The play and pause symbol, which is the control used most.
pub const MAIN_SYMBOL_SIZE: f64 = 28.0;

/// The way out, which has a corner to itself and is drawn to be found.
pub const BACK_SYMBOL_SIZE: f64 = 22.0;

pub const FONT_SIZE: f64 = scale::FONT_BODY;
/// Text that is a control or sits among them: the times, the speed, the
/// words on a button. At 13 points it looked like small print next to
/// symbols of 18.
pub const CONTROL_FONT_SIZE: f64 = scale::FONT_HEADLINE;
pub const TITLE_FONT_SIZE: f64 = scale::FONT_HEADLINE;
pub const TITLE_SPACING: f64 = scale::SPACE_XXS;

/// The height a slider can be grabbed in, far more than what it draws,
/// and enough for a finger.
pub const SLIDER_HEIGHT: f64 = 44.0;
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
/// With less room than this for each chapter on average, the gaps show
/// only under the pointer.
pub const MARK_SPACING_AT_REST: f64 = 28.0;
pub const VOLUME_WIDTH: f64 = 84.0;
/// How far VoiceOver moves the scrubber at a time, in seconds, and the
/// volume, out of 100.
pub const SCRUBBER_STEP: f64 = 10.0;
pub const VOLUME_STEP: f64 = 5.0;
/// The bubble that says what time the pointer is at on the scrubber: a
/// line of text with as much room above and below it as a notice has for
/// its size, and how far it floats above the bar.
pub const BUBBLE_HEIGHT: f64 = 32.0;
pub const BUBBLE_PADDING: f64 = scale::SPACE_M;
pub const BUBBLE_SPACING: f64 = scale::SPACE_S;
pub const BUBBLE_GAP: f64 = scale::SPACE_S;
/// How much the bubble's wish to be over the pointer counts for in the
/// layout, out of the 1000 of a constraint that must hold.
pub const FOLLOWS_POINTER: f32 = 1.0;
/// Space between the bar and the Skip Intro button above it.
pub const SKIP_GAP: f64 = scale::SPACE_M;

/// The panel that opens above a button of the bar: how wide it is, how
/// far its rows stand in from its edge, its corners, which are concentric
/// with those rows, and how far it floats above the bar.
pub const PANEL_WIDTH: f64 = 380.0;
pub const PANEL_INSET: f64 = scale::SPACE_S;
pub const PANEL_RADIUS: f64 = CHIP_RADIUS + PANEL_INSET;
pub const PANEL_GAP: f64 = scale::SPACE_S;
pub const PANEL_HEADING_GAP: f64 = scale::SPACE_XS;
/// The tick before the row in effect.
pub const PANEL_TICK_SIZE: f64 = scale::FONT_BODY;
/// Room for the widest value between two buttons, such as "−12.5 s".
pub const PANEL_VALUE_WIDTH: f64 = 64.0;

pub const CARD_PADDING: f64 = scale::SPACE_L;
/// Concentric with the buttons the card holds: their radius plus the
/// padding around them.
pub const CARD_RADIUS: f64 = scale::RADIUS_XXL;
pub const CARD_LINE_SPACING: f64 = scale::SPACE_XXS;
pub const CARD_BUTTON_SPACING: f64 = scale::SPACE_S;
/// A longer episode title than fits in this ends in an ellipsis.
pub const CARD_MAX_WIDTH: f64 = 560.0;
/// The still of the next episode on its card.
pub const STILL_WIDTH: f64 = 160.0;
pub const STILL_HEIGHT: f64 = 90.0;
pub const STILL_RADIUS: f64 = 8.0;

/// Between the names and the values in the panel of playback information.
pub const INFO_COLUMN_SPACING: f64 = 12.0;

/// The sign that the picture is on its way.
pub const WAITING_RADIUS: f64 = 22.0;
pub const WAITING_PADDING: f64 = 18.0;
pub const WAITING_SPACING: f64 = 10.0;

/// The card that says a file stopped playing.
pub const FAILURE_PADDING: f64 = 28.0;
pub const FAILURE_LINE_SPACING: f64 = 8.0;
pub const FAILURE_SYMBOL_SIZE: f64 = 30.0;
pub const FAILURE_TITLE_FONT_SIZE: f64 = 17.0;
/// The reason wraps at this width and is cut off after this many lines.
pub const FAILURE_TEXT_WIDTH: f64 = 380.0;
pub const FAILURE_REASON_LINES: isize = 3;

/// Seconds a click on the picture waits for a second one before it
/// pauses. The second click of a double click comes sooner than this from
/// most hands, and a pause this long after its click still reads as the
/// click's doing. The system's own double click interval is twice as long
/// as it is set by default, which reads as a player that is slow.
pub const CLICK_WAIT: f64 = 0.25;

/// How much larger one press of Command-plus makes the picture.
pub const ZOOM_STEP: f64 = 1.25;

/// Seconds the controls take to appear. Short, so that they feel instant.
pub const FADE_IN: f64 = scale::MOTION_FAST;
/// Seconds they take to go away.
pub const FADE_OUT: f64 = scale::MOTION_LEAVE;

/// How much is left of a control that can do nothing right now.
pub const DISABLED_ALPHA: f64 = 0.35;

pub fn text() -> Retained<NSColor> {
    NSColor::whiteColor()
}

pub fn secondary_text() -> Retained<NSColor> {
    scale::white(scale::SECONDARY_ON_ART_OPACITY)
}

/// The ring around the control the keyboard is on. The system's own
/// accent, so that it reads as the keyboard's place as it does anywhere.
pub fn focus() -> Retained<NSColor> {
    NSColor::keyboardFocusIndicatorColor()
}

/// Behind a control the pointer is over.
pub fn hovered() -> Retained<NSColor> {
    scale::white(scale::PILL_WASH_OPACITY)
}

/// Behind a quiet button the pointer is over.
pub fn quiet_hovered() -> Retained<NSColor> {
    scale::white(0.22)
}

/// Behind a control that is being pressed.
pub fn pressed() -> Retained<NSColor> {
    scale::white(0.28)
}

/// A control that must be seen over any picture, such as Skip Intro.
pub fn solid() -> Retained<NSColor> {
    scale::white(0.88)
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

/// Laid into the glass of a panel, so that white controls keep their
/// contrast when the scene behind the glass is bright.
pub fn glass_tint() -> Retained<NSColor> {
    scale::black(0.7)
}

/// Drawn over the glass and under the controls, for the scenes that the
/// tint alone leaves too light.
pub fn glass_shade() -> Retained<NSColor> {
    scale::black(0.4)
}

/// A panel for a viewer who has asked for less transparency.
pub fn plate() -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(0.08, 0.92)
}

/// The edge of such a panel, which a dark scene would otherwise swallow.
pub fn plate_rim() -> Retained<NSColor> {
    scale::white(0.14)
}

/// The part of a slider's track that is not filled.
pub fn track() -> Retained<NSColor> {
    scale::white(0.25)
}

/// The part of the scrubber's track that is ready to play.
pub fn track_loaded() -> Retained<NSColor> {
    scale::white(0.45)
}

pub fn track_filled() -> Retained<NSColor> {
    NSColor::whiteColor()
}

/// Under the whole track. Over a white picture the shade alone leaves the
/// unfilled part of the track too close to what is behind it.
pub fn track_bed() -> Retained<NSColor> {
    scale::black(0.3)
}

/// From the start of the track to where the pointer is over it.
pub fn track_pointed() -> Retained<NSColor> {
    scale::white(0.3)
}

/// Around what is white and may have white behind it: the knob and the
/// pointer's line where they cross the filled track, and a light button
/// over a white picture.
pub fn rim() -> Retained<NSColor> {
    scale::black(0.35)
}
