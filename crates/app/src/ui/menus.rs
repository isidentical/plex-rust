//! The menu bar.

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::NSString;

use super::callback::Callback;

/// What the app's own menu items do. The shell keeps this, because menu
/// items do not retain their targets.
pub struct Commands {
    pub back: Retained<Callback>,
    pub forward: Retained<Callback>,
    pub reload: Retained<Callback>,
    pub find: Retained<Callback>,
    pub play: Retained<Callback>,
    pub toggle_watched: Retained<Callback>,
}

/// A place the Go menu leads to: its title and what going there does.
pub type Place = (String, Retained<Callback>);

const COMMAND: NSEventModifierFlags = NSEventModifierFlags::Command;

/// Builds the menu bar and gives it to the app. `places` are Home and the
/// libraries, which get Command-1 and up.
pub fn install(main_thread: MainThreadMarker, commands: &Commands, places: &[Place]) {
    let app = NSApplication::sharedApplication(main_thread);
    let bar = NSMenu::new(main_thread);
    let add = |title: &str, entries: Vec<Retained<NSMenuItem>>| {
        let menu = NSMenu::initWithTitle(NSMenu::alloc(main_thread), &NSString::from_str(title));
        for entry in entries {
            menu.addItem(&entry);
        }
        let holder = NSMenuItem::new(main_thread);
        holder.setSubmenu(Some(&menu));
        bar.addItem(&holder);
        menu
    };
    let system = |title: &str, action, key: &str, modifiers| {
        entry(title, action, None, key, modifiers, main_thread)
    };
    let own = |title: &str, target: &Callback, key: &str, modifiers| {
        entry(title, Callback::action(), Some(target), key, modifiers, main_thread)
    };
    let line = || NSMenuItem::separatorItem(main_thread);
    let with_option = COMMAND | NSEventModifierFlags::Option;
    let with_shift = COMMAND | NSEventModifierFlags::Shift;
    let with_control = COMMAND | NSEventModifierFlags::Control;

    add(
        "plex-rust",
        vec![
            system("About plex-rust", sel!(orderFrontStandardAboutPanel:), "", COMMAND),
            line(),
            system("Hide plex-rust", sel!(hide:), "h", COMMAND),
            system("Hide Others", sel!(hideOtherApplications:), "h", with_option),
            system("Show All", sel!(unhideAllApplications:), "", COMMAND),
            line(),
            system("Quit plex-rust", sel!(terminate:), "q", COMMAND),
        ],
    );
    add("File", vec![system("Close Window", sel!(performClose:), "w", COMMAND)]);
    add(
        "Edit",
        vec![
            system("Undo", sel!(undo:), "z", COMMAND),
            system("Redo", sel!(redo:), "z", with_shift),
            line(),
            system("Cut", sel!(cut:), "x", COMMAND),
            system("Copy", sel!(copy:), "c", COMMAND),
            system("Paste", sel!(paste:), "v", COMMAND),
            system("Select All", sel!(selectAll:), "a", COMMAND),
            line(),
            own("Find", &commands.find, "f", COMMAND),
        ],
    );
    add(
        "View",
        vec![
            system("Show Sidebar", sel!(toggleSidebar:), "s", with_control),
            system("Enter Full Screen", sel!(toggleFullScreen:), "f", with_control),
            line(),
            own("Reload", &commands.reload, "r", COMMAND),
        ],
    );

    let mut go = vec![
        own("Back", &commands.back, "[", COMMAND),
        own("Forward", &commands.forward, "]", COMMAND),
        line(),
    ];
    for (number, (title, target)) in (1..=9).zip(places) {
        let key = number.to_string();
        go.push(own(title, target, &key, COMMAND));
    }
    add("Go", go);
    add(
        "Playback",
        vec![
            own("Play", &commands.play, "p", COMMAND),
            own("Mark as Watched or Unwatched", &commands.toggle_watched, "u", with_shift),
        ],
    );
    let window = add(
        "Window",
        vec![
            system("Minimize", sel!(performMiniaturize:), "m", COMMAND),
            system("Zoom", sel!(performZoom:), "", COMMAND),
        ],
    );
    app.setWindowsMenu(Some(&window));
    app.setMainMenu(Some(&bar));
}

/// One menu item. Without a target the action goes to whatever in the
/// window or the app answers to it.
fn entry(
    title: &str,
    action: Sel,
    target: Option<&Callback>,
    key: &str,
    modifiers: NSEventModifierFlags,
    main_thread: MainThreadMarker,
) -> Retained<NSMenuItem> {
    // SAFETY: the action is a selector AppKit's responders implement, or
    // `Callback`'s own with a callback as the target below.
    let entry = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(main_thread),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    };
    entry.setKeyEquivalentModifierMask(modifiers);
    if let Some(target) = target {
        // SAFETY: the menu item does not retain its target. `Commands` and
        // the places are kept by the shell for as long as the app runs.
        unsafe { entry.setTarget(Some(target)) };
    }
    entry
}
