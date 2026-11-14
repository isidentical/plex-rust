//! The heading of the list of what a person is in or has directed: their
//! photo, their name and how much the library has of theirs.

use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAccessibility, NSColor, NSLayoutConstraint, NSTextField, NSView};
use objc2_foundation::NSArray;
use plex::model::Role;

use super::artwork::ArtView;
use super::label::{self, Weight};
use super::text;
use super::theme;
use crate::images::Images;

pub struct PersonHeading {
    view: Retained<NSView>,
    count: Retained<NSTextField>,
    /// Kept because the view only holds the photo's views, not what
    /// loads the picture into them.
    _photo: ArtView,
}

impl PersonHeading {
    pub fn new(main_thread: MainThreadMarker, person: &Role, images: &Rc<Images>) -> Self {
        let width = theme::PORTRAIT_WIDTH;
        let photo = ArtView::new(main_thread, width / 2.0);
        photo.set_flat();
        photo.show(person.thumb.as_deref(), theme::PORTRAIT_REQUEST, images);
        // The name beside it says who it is.
        photo.view().setAccessibilityElement(false);
        let name =
            label::line(theme::FONT_LARGE_TITLE, Weight::Bold, &NSColor::labelColor(), main_thread);
        label::set(&name, &person.tag);
        let secondary = NSColor::secondaryLabelColor();
        let count = label::line(theme::FONT_HEADLINE, Weight::Medium, &secondary, main_thread);

        let view = NSView::new(main_thread);
        let parts: [&NSView; 3] = [photo.view(), &name, &count];
        for part in parts {
            view.addSubview(part);
        }
        let (portrait, margin) = (photo.view(), theme::PAGE_MARGIN);
        // A person the server has no photo of is a name alone.
        portrait.setHidden(person.thumb.is_none());
        let beside = match person.thumb {
            Some(_) => name
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&portrait.trailingAnchor(), theme::SPACE_XL),
            None => {
                name.leadingAnchor().constraintEqualToAnchor_constant(&view.leadingAnchor(), margin)
            }
        };
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            portrait
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&view.leadingAnchor(), margin),
            portrait.centerYAnchor().constraintEqualToAnchor(&view.centerYAnchor()),
            portrait.widthAnchor().constraintEqualToConstant(width),
            portrait.heightAnchor().constraintEqualToConstant(width),
            beside,
            name.trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&view.trailingAnchor(), -margin),
            name.bottomAnchor()
                .constraintEqualToAnchor_constant(&view.centerYAnchor(), theme::SPACE_XS),
            count.leadingAnchor().constraintEqualToAnchor(&name.leadingAnchor()),
            count
                .topAnchor()
                .constraintEqualToAnchor_constant(&name.bottomAnchor(), theme::SPACE_XS),
        ]));
        Self { view, count, _photo: photo }
    }

    pub fn view(&self) -> &NSView {
        &self.view
    }

    /// Says how many titles the list has in all.
    pub fn set_total(&self, total: u32) {
        label::set(&self.count, &text::count(total, "title"));
    }
}
