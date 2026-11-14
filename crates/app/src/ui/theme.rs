//! Sizes and spacing. No other file holds a literal measurement.

use objc2_foundation::NSSize;

pub const WINDOW_SIZE: NSSize = NSSize {
    width: 1280.0,
    height: 800.0,
};
pub const WINDOW_MIN_SIZE: NSSize = NSSize {
    width: 880.0,
    height: 560.0,
};

pub const SIDEBAR_WIDTH: f64 = 220.0;
pub const SIDEBAR_ROW_HEIGHT: f64 = 30.0;
pub const SIDEBAR_ICON_GAP: f64 = 8.0;

/// Artwork is requested at twice the point size so it is sharp on Retina
/// displays, which is every display a current Mac ships with.
pub const RETINA_SCALE: u32 = 2;

pub const POSTER_WIDTH: u32 = 150;
pub const POSTER_HEIGHT: u32 = 225;
pub const STILL_WIDTH: u32 = 240;
pub const STILL_HEIGHT: u32 = 135;
pub const ARTWORK_CORNER_RADIUS: f64 = 8.0;
pub const CAPTION_GAP: f64 = 4.0;
/// The two lines of text under a cell's artwork, with the gaps around them.
pub const CAPTIONS_HEIGHT: f64 = 47.0;

pub const TITLE_FONT_SIZE: f64 = 13.0;
pub const SUBTITLE_FONT_SIZE: f64 = 11.0;
pub const HEADING_FONT_SIZE: f64 = 20.0;

pub const CELL_SPACING: f64 = 16.0;
pub const SHELF_HEADING_HEIGHT: f64 = 40.0;
pub const PAGE_MARGIN: f64 = 28.0;

pub const DETAIL_POSTER_WIDTH: u32 = 200;
pub const DETAIL_POSTER_HEIGHT: u32 = 300;
pub const DETAIL_TITLE_FONT_SIZE: f64 = 30.0;
pub const DETAIL_TEXT_WIDTH: f64 = 620.0;
pub const DETAIL_SUMMARY_LINES: isize = 8;
