//! The design scale: the one set of sizes for type, spacing, corners and
//! motion that every screen and the player are drawn from. No other file
//! holds a literal measurement.
//!
//! Type is the system font at five sizes, each set in its own weight:
//!
//! | size | points | weight | for |
//! | --- | --- | --- | --- |
//! | large title | 34 | bold | the name of what a page is about |
//! | title | 20 | bold | a shelf's heading, a notice |
//! | headline | 15 | semibold | a name within a page, the player's title |
//! | body | 13 | regular | running text, and semibold for a card's title or a button |
//! | caption | 11 | regular | the second line under a card, small print |
//!
//! Space is in steps of 2, 4, 8, 12, 16, 24 and 32 points.
//!
//! Corners are 4, 8, 12, 16 and 24 points, chosen by what the shape is:
//! 4 for a badge, 8 for a card or a control, 12 for artwork too large to
//! be a card. A shape that holds another is concentric with it: its radius
//! is the inner one plus the gap between them, so 16 around a card with 8
//! points of room and 24 around a control with 16.
//!
//! Motion is 0.12 seconds for an answer to the pointer, 0.2 for something
//! arriving and 0.35 for something leaving. Nothing moves unasked, and
//! nothing grows or slides when the viewer asked for less motion.

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSWorkspace};
use objc2_foundation::NSSize;

pub const WINDOW_SIZE: NSSize = NSSize { width: 1280.0, height: 800.0 };
pub const WINDOW_MIN_SIZE: NSSize = NSSize { width: 880.0, height: 560.0 };
/// A window filling a large display, for a snapshot.
pub const WINDOW_WIDE_SIZE: NSSize = NSSize { width: 2200.0, height: 1200.0 };

pub const FONT_LARGE_TITLE: f64 = 34.0;
pub const FONT_TITLE: f64 = 20.0;
pub const FONT_HEADLINE: f64 = 15.0;
pub const FONT_BODY: f64 = 13.0;
pub const FONT_CAPTION: f64 = 11.0;

pub const SPACE_XXS: f64 = 2.0;
pub const SPACE_XS: f64 = 4.0;
pub const SPACE_S: f64 = 8.0;
pub const SPACE_M: f64 = 12.0;
pub const SPACE_L: f64 = 16.0;
pub const SPACE_XL: f64 = 24.0;
pub const SPACE_XXL: f64 = 32.0;

pub const RADIUS_S: f64 = 4.0;
pub const RADIUS_M: f64 = 8.0;
pub const RADIUS_L: f64 = 12.0;
pub const RADIUS_XL: f64 = 16.0;
pub const RADIUS_XXL: f64 = 24.0;

/// How long an answer to the pointer takes: a hover, a press, controls
/// coming up.
pub const MOTION_FAST: f64 = 0.12;
/// How long something takes to arrive: a picture, the next page.
pub const MOTION_ARRIVE: f64 = 0.2;
/// How long something takes to leave when nobody sent it away.
pub const MOTION_LEAVE: f64 = 0.35;

/// The least a control measures each way where a finger has to land on
/// it. What is drawn may be smaller: the room around it takes the press.
pub const TOUCH_TARGET: f64 = 44.0;

pub const PAGE_MARGIN: f64 = SPACE_XXL;
pub const CELL_SPACING: f64 = SPACE_L;
/// Between the last row of one shelf and the heading of the next.
pub const SHELF_SPACING: f64 = SPACE_XL;
pub const SHELF_HEADING_HEIGHT: f64 = TOUCH_TARGET;
/// The round arrows at the end of a heading that page its shelf sideways.
pub const SHELF_ARROW_SIZE: f64 = 28.0;
pub const SHELF_ARROW_WASH_OPACITY: f64 = 0.1;
pub const SHELF_ARROW_HOVER_OPACITY: f64 = 0.22;
/// How strong an arrow is that has nowhere left to go.
pub const SHELF_ARROW_SPENT_OPACITY: f64 = 0.35;

pub const SIDEBAR_WIDTH: f64 = 220.0;
/// The column the rows' symbols are centred in, so that titles line up
/// whatever the width of the symbol before them.
pub const SIDEBAR_ICON_WIDTH: f64 = 22.0;
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
/// The disc and the initials that stand in for a person's missing photo.
pub const MONOGRAM_DISC_OPACITY: f64 = 0.12;
pub const MONOGRAM_INK_OPACITY: f64 = 0.7;

/// The two lines of text under a card's artwork, with the gaps around them.
pub const CAPTIONS_HEIGHT: f64 = 46.0;
/// How many of them a title may take, where the second only holds a year.
pub const CARD_TITLE_LINES: isize = 2;

/// An episode in a list: a still on the left, text beside it.
pub const EPISODE_ROW_HEIGHT: f64 = 124.0;
pub const EPISODE_STILL_WIDTH: f64 = 192.0;
pub const EPISODE_TEXT_WIDTH: f64 = 640.0;
pub const EPISODE_SUMMARY_LINES: isize = 2;
/// How strongly white is laid over a row under the pointer.
pub const ROW_HOVER_OPACITY: f64 = 0.06;

// What a card does under the pointer and under a press.
pub const HOVER_SCALE: f64 = 1.04;
pub const PRESS_SCALE: f64 = 0.98;
pub const HOVER_SHADOW_OPACITY: f32 = 0.6;
pub const HOVER_SHADOW_RADIUS: f64 = 14.0;
pub const HOVER_SHADOW_DROP: f64 = 8.0;
pub const HOVER_SCRIM_OPACITY: f64 = 0.28;
pub const PLAY_GLYPH_SIZE: f64 = 44.0;
pub const ARTWORK_OUTLINE_WIDTH: f64 = 1.0;
pub const ARTWORK_OUTLINE_OPACITY: f64 = 0.10;
pub const ARTWORK_HOVER_OUTLINE_OPACITY: f64 = 0.35;
pub const ARTWORK_PLACEHOLDER_OPACITY: f64 = 0.06;
/// The bars that stand for the captions of a card not loaded yet, and how
/// much of the card's width each takes.
pub const OUTLINE_BAR_HEIGHT: f64 = 8.0;
pub const OUTLINE_TITLE_WIDTH: f64 = 0.7;
pub const OUTLINE_SUBTITLE_WIDTH: f64 = 0.35;
pub const FOCUS_RING_WIDTH: f64 = 3.0;
/// How far outside the artwork the outer edge of the focus ring is, which
/// leaves a gap between the two.
pub const FOCUS_RING_OUTSET: f64 = FOCUS_RING_WIDTH + SPACE_XXS;
/// Around the card for the episode a page is about.
pub const CURRENT_OUTLINE_WIDTH: f64 = 2.0;
pub const CURRENT_OUTLINE_OPACITY: f64 = 0.9;

pub const PROGRESS_BAR_HEIGHT: f64 = 4.0;
pub const PROGRESS_BAR_INSET: f64 = SPACE_S;
pub const PROGRESS_TRACK_OPACITY: f64 = 0.55;
pub const BADGE_INSET: f64 = SPACE_S;
pub const BADGE_HEIGHT: f64 = 20.0;
pub const BADGE_PADDING: f64 = SPACE_S;
pub const BADGE_BACKGROUND_OPACITY: f64 = 0.7;

/// The top of a detail page: artwork on the left, text beside it.
pub const HERO_HEIGHT: f64 = 380.0;
/// The strip under a show's header that holds its season picker.
pub const SEASON_BAR_HEIGHT: f64 = 52.0;
/// The buttons of a detail page, drawn as capsules.
pub const PILL_HEIGHT: f64 = 36.0;
pub const PILL_PADDING: f64 = SPACE_L;
/// Between the glyph of a button and its title, in the pages and in the
/// player. The one gap off the scale: it is inside a single control.
pub const PILL_GLYPH_GAP: f64 = 6.0;
/// How strongly white is laid under a button that is not the main one.
pub const PILL_WASH_OPACITY: f64 = 0.16;
pub const HERO_POSTER_WIDTH: f64 = 232.0;
pub const HERO_POSTER_REQUEST: (u32, u32) = (232, 348);
/// An episode's page leads with a frame of the episode: this much of the
/// page's width, within these bounds.
pub const HERO_STILL_SHARE: f64 = 0.38;
pub const HERO_STILL_MIN_WIDTH: f64 = 240.0;
pub const HERO_STILL_MAX_WIDTH: f64 = 420.0;
pub const HERO_STILL_REQUEST: (u32, u32) = (420, 236);
/// The poster of the show beside the title of one of its episodes.
pub const HERO_SHOW_POSTER_WIDTH: f64 = 44.0;
pub const HERO_SHOW_POSTER_REQUEST: (u32, u32) = (44, 66);
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
/// The menus of a detail page that choose what plays: each under a
/// caption this wide, and itself no wider than this.
pub const CHOICE_CAPTION_WIDTH: f64 = 72.0;
pub const CHOICE_MENU_MAX_WIDTH: f64 = 320.0;
/// A genre on a detail page: a capsule that leads to the others like it.
pub const CHIP_HEIGHT: f64 = 24.0;
pub const CHIP_PADDING: f64 = 10.0;
pub const CHIP_WASH_OPACITY: f64 = 0.10;
pub const CHIP_HOVER_OPACITY: f64 = 0.22;

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
/// The least of the card's width the backdrop takes, at its right. It is
/// shown whole where that is wider.
pub const HOME_HERO_ART_LEAST: f64 = 0.6;
/// How much of the backdrop's width, from its near edge, the card's colour
/// runs over, and how strong the colour is at each point of the way.
pub const HOME_HERO_FADE: f64 = 0.6;
pub const HOME_HERO_FADE_STOPS: [(f64, f64); 5] =
    [(0.0, 1.0), (0.25, 0.86), (0.5, 0.58), (0.75, 0.24), (1.0, 0.0)];
/// The white of a card whose item has no colours of its own.
pub const HOME_HERO_PLAIN_WHITE: f64 = 0.14;
/// The grey shapes that stand for the title and the facts of a hero not
/// loaded yet: how wide and how tall each is.
pub const HOME_HERO_OUTLINE_TITLE: (f64, f64) = (280.0, 24.0);
pub const HOME_HERO_OUTLINE_FACTS: (f64, f64) = (180.0, 12.0);
/// How far up the banner, from its bottom, the page's colour starts to
/// run over it.
pub const HOME_HERO_SINK: f64 = 0.45;
pub const HOME_HERO_TITLE_LINES: isize = 2;
/// The bar beside the hero's button that shows how much was watched.
pub const HOME_HERO_PROGRESS_WIDTH: f64 = 120.0;
pub const HOME_HERO_TRACK_OPACITY: f64 = 0.3;
/// The capsule in the hero's corner that leads from one item to the next.
pub const HOME_HERO_PAGER_HEIGHT: f64 = SPACE_XL;
pub const HOME_HERO_PAGER_OPACITY: f64 = 0.45;
pub const HOME_HERO_DOT_SIZE: f64 = 6.0;
/// How strong the dot of an item that is not the one shown is.
pub const HOME_HERO_DOT_OPACITY: f64 = 0.4;

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

/// Whether the viewer asked the system for less transparency. Then the
/// sidebar and the toolbar are solid.
pub fn reduces_transparency() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency()
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

/// How strong white text of the second rank is on artwork and on video.
pub const SECONDARY_ON_ART_OPACITY: f64 = 0.8;

/// White at `opacity`, for outlines and overlays on artwork.
pub fn white(opacity: f64) -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(1.0, opacity)
}

/// Black at `opacity`, for scrims and tracks on artwork.
pub fn black(opacity: f64) -> Retained<NSColor> {
    NSColor::colorWithWhite_alpha(0.0, opacity)
}
