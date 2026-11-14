//! Sizes, spacing and colours. No other file holds a literal measurement.

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSWorkspace};
use objc2_foundation::NSSize;

pub const WINDOW_SIZE: NSSize = NSSize { width: 1280.0, height: 800.0 };
pub const WINDOW_MIN_SIZE: NSSize = NSSize { width: 880.0, height: 560.0 };
/// A window filling a large display, for a snapshot.
pub const WINDOW_WIDE_SIZE: NSSize = NSSize { width: 2200.0, height: 1200.0 };

// The type scale. Every label uses one of these sizes.
pub const FONT_LARGE_TITLE: f64 = 34.0;
pub const FONT_TITLE: f64 = 20.0;
pub const FONT_HEADLINE: f64 = 15.0;
pub const FONT_BODY: f64 = 13.0;
pub const FONT_CAPTION: f64 = 11.0;

// The spacing scale. Every gap and margin is one of these.
pub const SPACE_XXS: f64 = 2.0;
pub const SPACE_XS: f64 = 4.0;
pub const SPACE_S: f64 = 8.0;
pub const SPACE_M: f64 = 12.0;
pub const SPACE_L: f64 = 16.0;
pub const SPACE_XL: f64 = 24.0;
pub const SPACE_XXL: f64 = 32.0;

// The corner scale.
pub const RADIUS_S: f64 = 4.0;
pub const RADIUS_M: f64 = 8.0;
pub const RADIUS_L: f64 = 12.0;

pub const PAGE_MARGIN: f64 = SPACE_XXL;
pub const CELL_SPACING: f64 = SPACE_L;
/// Between the last row of one shelf and the heading of the next.
pub const SHELF_SPACING: f64 = SPACE_XL;
pub const SHELF_HEADING_HEIGHT: f64 = 44.0;

pub const SIDEBAR_WIDTH: f64 = 220.0;
pub const SIDEBAR_ROW_HEIGHT: f64 = 30.0;
pub const SIDEBAR_ICON_GAP: f64 = SPACE_S;

/// Artwork is requested at twice the point size so it is sharp on Retina
/// displays, which is every display a current Mac ships with.
pub const RETINA_SCALE: u32 = 2;

/// Upright artwork is two wide to three tall.
pub const POSTER_ASPECT: f64 = 1.5;
/// Wide artwork is sixteen to nine.
pub const LANDSCAPE_ASPECT: f64 = 9.0 / 16.0;

/// A poster in a shelf.
pub const POSTER_WIDTH: f64 = 160.0;
/// The narrowest a poster gets in a grid. A grid is for looking through a
/// whole library, so it trades size for more posters in view: four across
/// in the smallest window, six in the default one.
pub const POSTER_GRID_MIN_WIDTH: f64 = 136.0;
/// A wide card in a shelf, and the narrowest one gets in a grid.
pub const LANDSCAPE_WIDTH: f64 = 288.0;
/// The size posters are requested at, in points. One size for every grid
/// width, so that resizing the window never downloads them again.
pub const POSTER_REQUEST: (u32, u32) = (200, 300);
pub const LANDSCAPE_REQUEST: (u32, u32) = (320, 180);
/// An episode among the others of its season, in a shelf.
pub const STILL_WIDTH: f64 = 240.0;
/// A member of the cast, in a shelf.
pub const PORTRAIT_WIDTH: f64 = 112.0;
pub const PORTRAIT_REQUEST: (u32, u32) = (112, 112);
/// The glyph that stands in for a missing photo.
pub const PORTRAIT_GLYPH_SIZE: f64 = 44.0;
pub const PLACEHOLDER_GLYPH_OPACITY: f64 = 0.25;

/// The two lines of text under a card's artwork, with the gaps around them.
pub const CAPTIONS_HEIGHT: f64 = 46.0;

/// An episode in a list: a still on the left, text beside it.
pub const EPISODE_ROW_HEIGHT: f64 = 124.0;
pub const EPISODE_STILL_WIDTH: f64 = 192.0;
pub const EPISODE_TEXT_WIDTH: f64 = 640.0;
pub const EPISODE_SUMMARY_LINES: isize = 3;
/// How strongly white is laid over a row under the pointer.
pub const ROW_HOVER_OPACITY: f64 = 0.06;

// What a card does under the pointer and under a press.
pub const HOVER_SCALE: f64 = 1.04;
pub const PRESS_SCALE: f64 = 0.98;
pub const HOVER_DURATION: f64 = 0.12;
pub const HOVER_SHADOW_OPACITY: f32 = 0.6;
pub const HOVER_SHADOW_RADIUS: f64 = 14.0;
pub const HOVER_SHADOW_DROP: f64 = 8.0;
pub const HOVER_SCRIM_OPACITY: f64 = 0.28;
pub const PLAY_GLYPH_SIZE: f64 = 44.0;
/// How long freshly loaded artwork takes to fade in.
pub const ARTWORK_FADE: f64 = 0.18;
pub const ARTWORK_OUTLINE_WIDTH: f64 = 1.0;
pub const ARTWORK_OUTLINE_OPACITY: f64 = 0.10;
pub const ARTWORK_HOVER_OUTLINE_OPACITY: f64 = 0.35;
pub const ARTWORK_PLACEHOLDER_OPACITY: f64 = 0.06;
pub const FOCUS_RING_WIDTH: f64 = 3.0;
/// How far outside the artwork the outer edge of the focus ring is, which
/// leaves a gap between the two.
pub const FOCUS_RING_OUTSET: f64 = 5.0;
/// Around the card for the episode a page is about.
pub const CURRENT_OUTLINE_WIDTH: f64 = 2.0;
pub const CURRENT_OUTLINE_OPACITY: f64 = 0.9;

pub const PROGRESS_BAR_HEIGHT: f64 = 4.0;
pub const PROGRESS_BAR_INSET: f64 = SPACE_S;
pub const PROGRESS_TRACK_OPACITY: f64 = 0.55;
pub const BADGE_INSET: f64 = SPACE_S;
pub const BADGE_DOT_SIZE: f64 = 12.0;
pub const BADGE_DOT_RIM_WIDTH: f64 = 1.5;
pub const BADGE_DOT_RIM_OPACITY: f64 = 0.95;
pub const BADGE_SHADOW_OPACITY: f32 = 0.5;
pub const BADGE_SHADOW_RADIUS: f64 = 2.0;
pub const BADGE_HEIGHT: f64 = 20.0;
pub const BADGE_PADDING: f64 = 7.0;
pub const BADGE_BACKGROUND_OPACITY: f64 = 0.7;

/// How long one screen takes to fade into the next.
pub const SCREEN_FADE: f64 = 0.15;

/// The top of a detail page: artwork on the left, text beside it.
pub const HERO_HEIGHT: f64 = 380.0;
/// The strip under a show's header that holds its season picker.
pub const SEASON_BAR_HEIGHT: f64 = 52.0;
/// The buttons of a detail page, drawn as capsules.
pub const PILL_HEIGHT: f64 = 36.0;
pub const PILL_PADDING: f64 = SPACE_L;
/// Between the glyph of a button and its title.
pub const PILL_GLYPH_GAP: f64 = 6.0;
/// How strongly white is laid under a button that is not the main one.
pub const PILL_WASH_OPACITY: f64 = 0.16;
pub const HERO_POSTER_WIDTH: f64 = 232.0;
pub const HERO_POSTER_REQUEST: (u32, u32) = (232, 348);
pub const HERO_TEXT_WIDTH: f64 = 640.0;
pub const HERO_TITLE_LINES: isize = 3;
pub const HERO_FACTS_LINES: isize = 2;
pub const HERO_SUMMARY_LINES: isize = 4;
/// The popover that holds a summary too long for the page.
pub const SUMMARY_POPOVER_WIDTH: f64 = 420.0;
pub const HERO_SHADOW_OPACITY: f32 = 0.5;
pub const HERO_SHADOW_RADIUS: f64 = 24.0;
pub const HERO_SHADOW_DROP: f64 = 12.0;
pub const FACT_BADGE_BORDER_OPACITY: f64 = 0.4;

/// The backdrop behind a detail page, in pixels. One size for every window.
pub const BACKDROP_REQUEST: (u32, u32) = (1920, 1080);
/// How much of the page's height the backdrop covers, from the top.
pub const BACKDROP_HEIGHT_FRACTION: f64 = 0.8;
pub const BACKDROP_OPACITY: f32 = 0.5;
/// How dark the left of the backdrop is made, where the text is.
pub const BACKDROP_SCRIM_OPACITY: f64 = 0.5;
/// How far the page colours from the artwork are darkened, so that white
/// text on them stays readable.
pub const TINT_BRIGHTNESS: f64 = 0.45;

/// The top of the home screen: one item on its backdrop. How tall it is
/// until the page has a size to go by.
pub const HOME_HERO_HEIGHT: f64 = 360.0;
/// How tall it then is for each point of the page's width, so that the
/// backdrop is cut much the same way as the window grows.
pub const HOME_HERO_HEIGHT_PER_WIDTH: f64 = 0.37;
/// The most of the window's height it takes, so that the first shelf
/// stays in view under it on a large display.
pub const HOME_HERO_MOST: f64 = 0.45;
/// The size the home screen's backdrop is requested at, in points.
pub const HOME_HERO_REQUEST: (u32, u32) = (960, 540);
/// How dark the left edge of that backdrop is made, where the text is,
/// and how far across the darkening reaches before it is gone.
pub const HOME_HERO_SCRIM_OPACITY: f64 = 0.85;
pub const HOME_HERO_SCRIM_REACH: f64 = 0.75;
pub const HOME_HERO_FACTS_OPACITY: f64 = 0.8;
/// The bar beside the hero's button that shows how much was watched.
pub const HOME_HERO_PROGRESS_WIDTH: f64 = 160.0;
pub const HOME_HERO_TRACK_OPACITY: f64 = 0.3;

/// A download in the list: its poster on the left, text beside it, and
/// its buttons on the right.
pub const DOWNLOAD_ROW_HEIGHT: f64 = 112.0;
pub const DOWNLOAD_POSTER_WIDTH: f64 = 56.0;
pub const DOWNLOAD_BAR_WIDTH: f64 = 240.0;
/// The strip above the list that says the server is away.
pub const DOWNLOAD_STRIP_HEIGHT: f64 = 44.0;

/// The bar above a library's grid: its size on the left, order on the right.
pub const LIBRARY_BAR_HEIGHT: f64 = 44.0;

/// A message in the middle of an empty screen.
pub const NOTICE_SYMBOL_SIZE: f64 = 44.0;
pub const NOTICE_WIDTH: f64 = 420.0;

/// Whether the viewer asked the system for less movement. Then nothing
/// grows or slides, and one screen replaces the other without a fade.
pub fn reduces_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// Whether the viewer asked the system for more contrast. Then outlines
/// are stronger and artwork behind text is fainter.
pub fn increases_contrast() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast()
}

/// How strong the hairline around artwork at rest is.
pub fn artwork_outline_opacity() -> f64 {
    if increases_contrast() { ARTWORK_HOVER_OUTLINE_OPACITY } else { ARTWORK_OUTLINE_OPACITY }
}

/// How strongly a detail page's backdrop shows behind its text.
pub fn backdrop_opacity() -> f32 {
    if increases_contrast() { BACKDROP_OPACITY / 2.0 } else { BACKDROP_OPACITY }
}

/// Behind every screen that has no colours of its own. The window's own
/// colour, which the shelves' sideways scrollers also draw.
pub fn page_background() -> Retained<NSColor> {
    NSColor::windowBackgroundColor()
}

/// White at `opacity`, for outlines and overlays on artwork.
pub fn white(opacity: f64) -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(1.0, opacity)
}

/// Black at `opacity`, for scrims and tracks on artwork.
pub fn black(opacity: f64) -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(0.0, opacity)
}
