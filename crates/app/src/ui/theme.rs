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
