//! The menus that pop up from the speed, audio and subtitle buttons.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSControlStateValueOn, NSMenu, NSMenuItem};
use objc2_foundation::{NSString, ns_string};

/// One line of a menu.
pub struct Choice {
    pub title: String,
    /// A second, smaller line under the title, such as what the file
    /// itself calls a track.
    pub detail: Option<String>,
    /// What the menu item carries to the method that handles it.
    pub tag: isize,
    /// Whether it is the one in effect, which gets the check mark.
    pub chosen: bool,
    /// Whether a line is drawn under it, to set it apart from the rest.
    pub stands_apart: bool,
}

/// A menu of `choices` under a `heading` that says what is being chosen.
/// Picking one sends `action` to `target` with the menu item.
///
/// # Safety
///
/// `target` must implement `action` as a method taking the menu item, and
/// must stay alive for as long as the menu is: a menu item does not retain
/// its target.
pub unsafe fn build(
    main_thread: MainThreadMarker,
    heading: &str,
    choices: &[Choice],
    action: Sel,
    target: &AnyObject,
) -> Retained<NSMenu> {
    let menu = NSMenu::new(main_thread);
    menu.addItem(&NSMenuItem::sectionHeaderWithTitle(&NSString::from_str(heading), main_thread));
    for choice in choices {
        // SAFETY: the caller vouches for the selector.
        let entry = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(main_thread),
                &NSString::from_str(&choice.title),
                Some(action),
                ns_string!(""),
            )
        };
        // SAFETY: the caller keeps the target alive while the menu is.
        unsafe { entry.setTarget(Some(target)) };
        entry.setTag(choice.tag);
        entry.setSubtitle(choice.detail.as_deref().map(NSString::from_str).as_deref());
        if choice.chosen {
            entry.setState(NSControlStateValueOn);
        }
        menu.addItem(&entry);
        if choice.stands_apart {
            menu.addItem(&NSMenuItem::separatorItem(main_thread));
        }
    }
    menu
}
