//! Text on a screen: one line that cuts off, or a paragraph that wraps.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSColor, NSFont, NSFontWeight, NSFontWeightBold, NSFontWeightMedium, NSFontWeightRegular,
    NSFontWeightSemibold, NSLineBreakMode, NSTextField,
};
use objc2_foundation::NSString;

/// How heavy the letters are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

impl Weight {
    /// The weight as AppKit counts it.
    pub fn value(self) -> NSFontWeight {
        // SAFETY: the weights are constants that AppKit provides.
        unsafe {
            match self {
                Self::Regular => NSFontWeightRegular,
                Self::Medium => NSFontWeightMedium,
                Self::Semibold => NSFontWeightSemibold,
                Self::Bold => NSFontWeightBold,
            }
        }
    }
}

/// The system font at `size` points.
pub fn font(size: f64, weight: Weight) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, weight.value())
}

/// One line of text that cuts off with an ellipsis when it does not fit.
pub fn line(
    size: f64,
    weight: Weight,
    color: &NSColor,
    main_thread: MainThreadMarker,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::new(), main_thread);
    label.setFont(Some(&font(size, weight)));
    label.setTextColor(Some(color));
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    label.setTranslatesAutoresizingMaskIntoConstraints(false);
    label
}

/// Text that wraps onto at most `lines` lines.
pub fn paragraph(
    size: f64,
    weight: Weight,
    color: &NSColor,
    lines: isize,
    main_thread: MainThreadMarker,
) -> Retained<NSTextField> {
    let label = NSTextField::wrappingLabelWithString(&NSString::new(), main_thread);
    label.setFont(Some(&font(size, weight)));
    label.setTextColor(Some(color));
    label.setMaximumNumberOfLines(lines);
    // Wraps by word, and ends the last line that fits with an ellipsis.
    if let Some(cell) = label.cell() {
        cell.setTruncatesLastVisibleLine(true);
    }
    label.setSelectable(false);
    label.setTranslatesAutoresizingMaskIntoConstraints(false);
    label
}

pub fn set(label: &NSTextField, text: &str) {
    label.setStringValue(&NSString::from_str(text));
}
