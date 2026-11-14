//! Asking before a change too large for one click to make, and saying
//! why something asked for was not done.

use std::cell::Cell;

use block2::RcBlock;
use objc2::MainThreadOnly;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSModalResponse, NSWindow};
use objc2_foundation::NSString;
use plex::model::{Item, Kind};

use super::text;

/// Says, in a sheet on `window`, that what the viewer asked for was not
/// done: `message` is what, `reason` is why. The page behind it stays as
/// it is, which an error page in its place would not.
pub fn refusal(window: &NSWindow, message: &str, reason: &str) {
    let alert = NSAlert::new(window.mtm());
    alert.setMessageText(&NSString::from_str(message));
    alert.setInformativeText(&NSString::from_str(reason));
    alert.beginSheetModalForWindow_completionHandler(window, None);
}

/// Asks, in a sheet on `window`, whether every episode of `item`, a show
/// or a season, is to be marked as watched or as unwatched. Calls `then`
/// if the viewer says so.
///
/// One click on an episode is undone by another. One click on a show
/// would overwrite what was watched of a hundred episodes, and nothing
/// brings that back.
pub fn marking_every_episode(
    window: &NSWindow,
    item: &Item,
    watched: bool,
    then: impl FnOnce() + 'static,
) {
    let state = if watched { "Watched" } else { "Unwatched" };
    let episodes = match item.leaf_count {
        Some(count) => format!("all {}", text::count(count, "episode")),
        None => "every episode".to_owned(),
    };
    let whole = match (item.kind, &item.parent_title) {
        (Kind::Season, Some(show)) => format!("{} of {show}", item.title),
        _ => item.title.clone(),
    };
    let alert = NSAlert::new(window.mtm());
    alert.setMessageText(&NSString::from_str(&format!(
        "Mark {episodes} of {whole} as {}?",
        state.to_lowercase()
    )));
    alert.setInformativeText(&NSString::from_str(
        "This replaces what the server remembers of each episode, for every Plex app.",
    ));
    alert.addButtonWithTitle(&NSString::from_str(&format!("Mark as {state}")));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));

    let then = Cell::new(Some(then));
    let answered = RcBlock::new(move |answer: NSModalResponse| {
        if let (Some(then), true) = (then.take(), answer == NSAlertFirstButtonReturn) {
            then();
        }
    });
    alert.beginSheetModalForWindow_completionHandler(window, Some(&answered));
}
