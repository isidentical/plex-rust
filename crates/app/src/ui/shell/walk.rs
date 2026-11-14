//! A scripted walk through the app that prints when each thing happened,
//! for measuring what the viewer waits for. Like a snapshot it runs off
//! screen, and it ends in one.

use std::rc::Rc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2_app_kit::{NSScrollView, NSView};

use super::{Press, Route, Screen, Shell, Snapshot};
use crate::timing;
use crate::ui::sidebar::Destination;

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
    /// Type this in the search field.
    Search(String),
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
            None if name == "wait" => Some(Self::Wait),
            Some(("library", index)) => index.parse().ok().map(Self::Library),
            Some(("list", path)) => Some(Self::List(path.to_owned())),
            Some(("scroll", points)) => points.parse().ok().map(Self::Scroll),
            Some(("search", query)) => Some(Self::Search(query.to_owned())),
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
            (Step::Wait | Step::Scroll(_) | Step::Press(_), _) => {}
        }
    }

    /// Prints where the walk stands: the place, how many cards it holds
    /// and how far down it is scrolled.
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
            Screen::Detail(_) | Screen::Downloads(_) => 0,
        };
        let scrolled = first_scroller_in(screen.view())
            .map_or(0.0, |scroller| scroller.contentView().bounds().origin.y);
        let searched = self.toolbar.search_text();
        let sidebar = self.split.splitViewItems().containsObject(&self.sidebar_item);
        let stale = self.stale.is_shown();
        timing::mark(&format!(
            "state: {title:?} cards={cards} scrolled={scrolled:.0} search={searched:?} \
             sidebar={sidebar} stale={stale}"
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
