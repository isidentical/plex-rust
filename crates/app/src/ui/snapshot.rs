//! Saving what a view shows as a PNG.
//!
//! The app draws itself into the image, so this needs no screen recording
//! permission and works while the window is not on screen. It exists so
//! that an unattended run can check what the interface looks like.

use std::path::Path;

use objc2_app_kit::{NSBitmapImageFileType, NSView};
use objc2_foundation::NSDictionary;

use crate::error::{Error, Result};

pub fn write_png(view: &NSView, path: &Path) -> Result<()> {
    let bounds = view.bounds();
    let bitmap = view.bitmapImageRepForCachingDisplayInRect(bounds).ok_or(Error::Snapshot)?;
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &bitmap);

    // SAFETY: PNG takes no properties, so the empty dictionary is valid.
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    };
    let png = png.ok_or(Error::Snapshot)?;
    std::fs::write(path, png.to_vec())
        .map_err(|source| Error::File { path: path.to_owned(), source })
}
