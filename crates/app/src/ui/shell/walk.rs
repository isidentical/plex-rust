//! A scripted walk through the app that prints when each thing happened,
//! for measuring what the viewer waits for. Like a snapshot it runs off
//! screen, and it ends in one.

use std::rc::Rc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2_app_kit::{NSScrollView, NSView};

use plex::model::RatingKey;

use super::{Press, Route, SHELF_LENGTH, Screen, Shell, Snapshot};
use crate::downloads::State;
use crate::timing;
use crate::ui::sidebar::Destination;
use crate::ui::text;

/// How long each step gets before the next, so that what it started has
/// answered and its timings do not run into the next step's.
const PAUSE: Duration = Duration::from_millis(1500);

/// One thing a viewer does.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Click the library at this position in the sidebar.
    Library(usize),
    Home,
    /// Open the whole list at this server path.
    List(String),
    /// Ask the server about the place on screen again.
    Refresh,
    /// Scroll this many points down.
    Scroll(f64),
    /// Press a key among the cards.
    Press(Press),
    Back,
    Forward,
    /// Have this in the search field all at once.
    Search(String),
    /// Type this in the search field letter by letter, without a pause.
    Type(String),
    /// Come back to the app from another one.
    Activate,
    /// Click Downloads in the sidebar.
    Downloads,
    /// Click the card Home has for the item with this rating key, going
    /// by what was saved of Home.
    Open(String),
    /// On the page of a film or an episode, click the download button.
    Download,
    /// On Downloads, click the first button with this title.
    Click(String),
    /// Click Try Again on the line that says the server is away.
    Retry,
    /// Do nothing for one more pause.
    Wait,
}

impl Step {
    pub fn named(name: &str) -> Option<Self> {
        if let Some(press) = Press::named(name) {
            return Some(Self::Press(press));
        }
        match name.split_once('=') {
            None if name == "home" => Some(Self::Home),
            None if name == "refresh" => Some(Self::Refresh),
            None if name == "back" => Some(Self::Back),
            None if name == "forward" => Some(Self::Forward),
            None if name == "activate" => Some(Self::Activate),
            None if name == "wait" => Some(Self::Wait),
            None if name == "downloads" => Some(Self::Downloads),
            None if name == "download" => Some(Self::Download),
            None if name == "retry" => Some(Self::Retry),
            Some(("open", key)) => Some(Self::Open(key.to_owned())),
            Some(("click", title)) => Some(Self::Click(title.to_owned())),
            Some(("library", index)) => index.parse().ok().map(Self::Library),
            Some(("list", path)) => Some(Self::List(path.to_owned())),
            Some(("scroll", points)) => points.parse().ok().map(Self::Scroll),
            Some(("search", query)) => Some(Self::Search(query.to_owned())),
            Some(("type", query)) => Some(Self::Type(query.to_owned())),
            _ => None,
        }
    }
}

impl Shell {
    /// Takes `steps` one pause apart, then the snapshot.
    pub(super) fn walk(self: &Rc<Self>, steps: Vec<Step>, snapshot: Snapshot) {
        // A timer can only be created on the runtime's own threads.
        let pause = async { tokio::time::sleep(PAUSE).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, pause, move |()| {
            shell.mark_state();
            let mut steps = steps.into_iter();
            let Some(step) = steps.next() else {
                return shell
                    .snapshot_later(Snapshot { subject: super::Subject::Home, ..snapshot });
            };
            timing::mark(&format!("step {step:?}"));
            shell.take(step);
            shell.walk(steps.collect(), snapshot);
        });
    }

    fn take(self: &Rc<Self>, step: Step) {
        let screen = self.current(|entry| entry.screen.clone());
        match (step, screen) {
            (Step::Library(index), _) => {
                let section =
                    self.session().and_then(|session| session.sections.get(index).cloned());
                if let Some(section) = section {
                    self.go_to(&Destination::Section(section));
                }
            }
            (Step::Home, _) => self.go_to(&Destination::Home),
            (Step::List(path), _) => self.go(Route::List { title: "List".to_owned(), path }),
            (Step::Refresh, _) => self.refresh(),
            (Step::Scroll(y), Some(screen)) => screen.scroll_to(y),
            (Step::Press(press), Some(Screen::Posters(posters))) => posters.press(press),
            (Step::Back, _) => self.back(),
            (Step::Forward, _) => self.forward(),
            (Step::Search(query), _) => {
                self.toolbar.set_search_text(&query);
                self.search(&query);
            }
            (Step::Type(query), _) => {
                for (end, _) in query.char_indices().skip(1).chain([(query.len(), ' ')]) {
                    self.toolbar.set_search_text(&query[..end]);
                    self.search(&query[..end]);
                }
            }
            (Step::Activate, _) => self.activated(),
            (Step::Downloads, _) => self.go_to(&Destination::Downloads),
            (Step::Open(key), _) => self.open_from_home(RatingKey::new(key)),
            (Step::Download, Some(Screen::Detail(detail))) => detail.press_download(),
            (Step::Click(title), Some(Screen::Downloads(list))) => {
                if !list.press(&title) {
                    timing::mark(&format!("no button {title:?}"));
                }
            }
            (Step::Retry, _) if self.stale.is_shown() => self.retry(),
            (
                Step::Wait
                | Step::Scroll(_)
                | Step::Press(_)
                | Step::Download
                | Step::Click(_)
                | Step::Retry,
                _,
            ) => {}
        }
    }

    /// Opens the page of the item with `key` with what Home's card for it
    /// holds, which is what a click on that card hands over.
    fn open_from_home(self: &Rc<Self>, key: RatingKey) {
        let Some(session) = self.session() else {
            return;
        };
        let remembered = session.server.remembered();
        let read = async move { remembered.home(SHELF_LENGTH).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, read, move |hubs| {
            let hubs = hubs.unwrap_or_default();
            let card = hubs.iter().flat_map(|hub| &hub.items).find(|item| item.rating_key == key);
            match card {
                Some(item) => shell.open(item),
                None => timing::mark(&format!("no card for {key}")),
            }
        });
    }

    /// Prints where the walk stands: the place, how many cards it holds,
    /// how far down it is scrolled, whether the server is there and how
    /// far each download has got.
    fn mark_state(&self) {
        let Some((title, screen)) =
            self.current(|entry| (super::title(&entry.route).to_owned(), entry.screen.clone()))
        else {
            return timing::mark("state: no screen");
        };
        // A window that is never on screen lays out only when asked.
        self.window.layoutIfNeeded();
        let cards = match &screen {
            Screen::Posters(posters) => posters.len(),
            Screen::Detail(_) | Screen::Downloads(_) | Screen::Activity(_) => 0,
        };
        let scrolled = first_scroller_in(screen.view())
            .map_or(0.0, |scroller| scroller.contentView().bounds().origin.y);
        let searched = self.toolbar.search_text();
        let sidebar = self.split.splitViewItems().containsObject(&self.sidebar_item);
        let stale = self.stale.is_shown();
        let notice = self.notice.borrow().is_some();
        let reach = match self.reach.get() {
            reach if reach.is_answering() => "answering",
            reach if reach.is_away() => "away",
            _ => "unknown",
        };
        let download = |entry: &crate::downloads::Entry| {
            let (key, size) = (entry.download.key(), entry.download.size);
            match &entry.state {
                State::Downloading(transfer) => {
                    format!("{key}:downloading {}%", text::percent(transfer.received(), size))
                }
                State::Incomplete { received, error } => {
                    let failed = if error.is_some() { " failed" } else { "" };
                    format!("{key}:stopped {}%{failed}", text::percent(*received, size))
                }
                State::Downloaded => format!("{key}:downloaded"),
            }
        };
        let downloads: Vec<String> = self.downloads.entries().iter().map(download).collect();
        timing::mark(&format!(
            "state: {title:?} cards={cards} scrolled={scrolled:.0} search={searched:?} \
             sidebar={sidebar} stale={stale} notice={notice} reach={reach} \
             downloads=[{}]",
            downloads.join(", ")
        ));
    }

    /// Prints that `what` was handed to the views, and then when they had
    /// made of it what the viewer sees. Nothing when no one is timing.
    pub(super) fn mark_shown(&self, what: &str) {
        if !timing::is_on() {
            return;
        }
        timing::mark(what);
        self.window.layoutIfNeeded();
        timing::mark("  laid out");
    }
}

fn first_scroller_in(view: &NSView) -> Option<Retained<NSScrollView>> {
    view.subviews().iter().find_map(|child| match child.downcast::<NSScrollView>() {
        Ok(scroller) => Some(scroller),
        Err(child) => first_scroller_in(&child),
    })
}
