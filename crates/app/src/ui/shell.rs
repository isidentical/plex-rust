//! The main window: sidebar on the left, the chosen place on the right.

use std::cell::{Cell, OnceCell, RefCell};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameDarkAqua, NSApplication, NSApplicationActivationPolicy,
    NSApplicationDelegate, NSBackingStoreType, NSMenu, NSMenuItem, NSSplitViewController,
    NSSplitViewItem, NSTextField, NSView, NSViewController, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSString, ns_string,
};
use plex::MediaServer;
use plex::library::Page;
use plex::model::{Hub, Item, Kind, RatingKey, Section};

use super::detail::Detail;
use super::layout;
use super::player::input::Key;
use super::player::{Outcome, PlayerScreen, Purpose};
use super::poster::Shape;
use super::posters::{Arrangement, Posters, Shelf};
use super::sidebar::{Destination, Sidebar};
use super::snapshot;
use super::theme;
use crate::account;
use crate::background::Background;
use crate::error::{Error, Result, with_causes};
use crate::images::Images;
use crate::settings::Settings;

/// How many items each shelf of the home screen starts with.
const SHELF_LENGTH: u32 = 20;

/// How many items of a library are fetched at a time.
const PAGE_SIZE: u32 = 100;

/// How long a snapshot waits after the window has its content, so that
/// work started by showing it, such as loading artwork, can finish.
const SNAPSHOT_SETTLE: Duration = Duration::from_secs(3);

/// How long a snapshot of the player waits before pressing its keys, so
/// that the file is open and the keys have something to act on.
const SNAPSHOT_KEYS_AFTER: Duration = Duration::from_millis(1500);

struct Shell {
    main_thread: MainThreadMarker,
    background: Rc<Background>,
    window: Retained<NSWindow>,
    /// The area right of the sidebar. Its one subview is the current place.
    content: Retained<NSView>,
    // Set once, right after the shell exists, because the sidebar's
    // selection handler needs the shell.
    sidebar: OnceCell<Retained<Sidebar>>,
    /// Set once a server is known, from the last launch or from asking.
    session: RefCell<Option<Session>>,
    /// What the content area shows. Held here because views do not keep
    /// their data sources and button targets alive.
    place: RefCell<Place>,
    /// The sidebar's place, where the trail of detail pages starts.
    destination: RefCell<Destination>,
    /// The detail pages opened since leaving that place, last one on screen.
    trail: RefCell<Vec<Item>>,
    /// The sidebar and content area, which the player replaces while open.
    split: Retained<NSSplitViewController>,
    playing: RefCell<Option<Retained<PlayerScreen>>>,
    /// Set while an item is being fetched in order to play it.
    opening_player: Cell<bool>,
    /// Speed and volume, kept from one item to the next and across launches.
    settings: Cell<Settings>,
}

/// What exists only while a server is known.
#[derive(Clone)]
struct Session {
    server: MediaServer,
    images: Rc<Images>,
}

enum Place {
    Message,
    Posters(Retained<Posters>),
    Detail(
        #[expect(dead_code, reason = "held only to keep the page and its button target alive")]
        Retained<Detail>,
    ),
}

/// What a snapshot shows.
#[derive(Clone)]
pub enum Subject {
    Home,
    /// The library at this position in the sidebar, counting from zero.
    Library(usize),
    /// The detail page of the item with this rating key.
    Item(String),
    /// The player, playing the item with this rating key as a check, after
    /// these keys were pressed in it.
    Play(String, Vec<Key>),
}

/// Opens the main window and runs the app until it quits.
///
/// With `snapshot`, the window stays off screen: once the subject has
/// loaded, the window is saved to the path as a PNG and the app exits.
pub fn run(snapshot: Option<(Subject, PathBuf)>) -> Result<()> {
    let main_thread =
        MainThreadMarker::new().unwrap_or_else(|| panic!("main() runs on the main thread"));
    let (client, token) = account::signed_in()?;
    let background = Rc::new(Background::new()?);

    let app = NSApplication::sharedApplication(main_thread);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.setMainMenu(Some(&main_menu(main_thread)));
    // A film library looks right on dark, whatever the system is set to.
    // SAFETY: the name is a constant that AppKit provides.
    let dark = unsafe { NSAppearanceNameDarkAqua };
    app.setAppearance(NSAppearance::appearanceNamed(dark).as_deref());

    let shell = Shell::new(main_thread, Rc::clone(&background), Settings::load()?);
    if snapshot.is_none() {
        shell.window.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }

    let responses = responses_directory()?;
    let saved = account::saved_server(&client)?.map(|server| server.with_cache(responses.clone()));
    let subject = snapshot.as_ref().map(|(subject, _)| subject.clone());

    // First, what the last launch left: the same server and the libraries
    // it had then. This puts the sidebar and Home on screen at once.
    if let Some(server) = saved.clone() {
        let (shell, subject) = (Rc::clone(&shell), subject.clone());
        let remembered = server.remembered();
        let ask = async move { remembered.sections().await };
        background.run(main_thread, ask, move |sections| {
            if let (Ok(sections), None) = (sections, shell.session()) {
                shell.or_show(shell.connected(server, sections, subject.as_ref()));
            }
        });
    }

    // Then the truth. The saved server is asked directly, which is one
    // request. Only if it does not answer, say on another network, is
    // plex.tv asked where the server is now.
    let connect = async move {
        if let Some(server) = saved
            && let Ok(sections) = server.sections().await
        {
            return Ok((server, sections));
        }
        let servers = plex::discovery::servers(&client, &token).await?;
        let server = servers.first().ok_or(Error::NoServer)?;
        let server = MediaServer::connect(&client, server, None).await?.with_cache(responses);
        let sections = server.sections().await?;
        Ok::<_, Error>((server, sections))
    };
    let loaded = Rc::clone(&shell);
    background.run(main_thread, connect, move |result| {
        match result {
            Ok((server, sections)) => {
                if let Err(error) = account::save_server(&server) {
                    eprintln!("plex-rust: {}", with_causes(&error));
                }
                // Showing the same server again would reload the screen the
                // viewer is already looking at.
                let known = loaded
                    .session()
                    .is_some_and(|session| session.server.address() == server.address());
                if !known {
                    loaded.or_show(loaded.connected(server, sections, subject.as_ref()));
                }
            }
            Err(error) if loaded.session().is_some() => {
                eprintln!("plex-rust: {}", with_causes(&error));
            }
            Err(error) => loaded.show_message(&with_causes(&error)),
        }
        if let Some((_, path)) = snapshot {
            // A timer can only be created on the runtime's own threads.
            let wait = async { tokio::time::sleep(SNAPSHOT_SETTLE).await };
            let shell = Rc::clone(&loaded);
            loaded.background.run(main_thread, wait, move |()| shell.snapshot_and_exit(&path));
        }
    });

    let delegate = AppDelegate::new(main_thread, Rc::clone(&shell));
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
    Ok(())
}

impl Shell {
    fn new(
        main_thread: MainThreadMarker,
        background: Rc<Background>,
        settings: Settings,
    ) -> Rc<Self> {
        let content = NSView::new(main_thread);
        let content_controller = view_controller(main_thread);
        content_controller.setView(&content);
        let shell = Rc::new(Self {
            main_thread,
            background,
            window: window(main_thread),
            content,
            sidebar: OnceCell::new(),
            session: RefCell::new(None),
            place: RefCell::new(Place::Message),
            destination: RefCell::new(Destination::Home),
            trail: RefCell::new(Vec::new()),
            split: NSSplitViewController::new(main_thread),
            playing: RefCell::new(None),
            opening_player: Cell::new(false),
            settings: Cell::new(settings),
        });

        let selected = Rc::downgrade(&shell);
        let sidebar = Sidebar::new(main_thread, move |destination| {
            if let Some(shell) = selected.upgrade() {
                shell.go_to(destination);
            }
        });
        let sidebar_controller = view_controller(main_thread);
        sidebar_controller.setView(sidebar.view());
        let sidebar_item = NSSplitViewItem::sidebarWithViewController(&sidebar_controller);
        sidebar_item.setMinimumThickness(theme::SIDEBAR_WIDTH);
        sidebar_item.setMaximumThickness(theme::SIDEBAR_WIDTH);

        shell.split.addSplitViewItem(&sidebar_item);
        shell.split.addSplitViewItem(&NSSplitViewItem::splitViewItemWithViewController(
            &content_controller,
        ));
        shell.window.setContentViewController(Some(&shell.split));
        shell.window.setContentSize(theme::WINDOW_SIZE);
        shell.window.center();

        // Cannot fail: the cell was created empty a few lines up.
        let _ = shell.sidebar.set(sidebar);
        shell.show_message("Connecting to your server");
        shell
    }

    fn connected(
        self: &Rc<Self>,
        server: MediaServer,
        sections: Vec<Section>,
        subject: Option<&Subject>,
    ) -> Result<()> {
        let images = Images::new(self.main_thread, Rc::clone(&self.background), server.clone())?;
        *self.session.borrow_mut() = Some(Session { server, images: Rc::new(images) });

        let mut destinations = vec![Destination::Home];
        destinations.extend(sections.into_iter().map(Destination::Section));
        destinations.push(Destination::Downloads);
        let first = match subject {
            // Libraries follow Home in the sidebar.
            Some(Subject::Library(index)) => index + 1,
            _ => 0,
        };
        if let Some(sidebar) = self.sidebar.get() {
            // Selecting a row is what shows its place.
            sidebar.show(destinations, first);
        }
        match subject {
            Some(Subject::Item(key)) => self.fetch(RatingKey::new(key.as_str()), Self::open),
            Some(Subject::Play(key, keys)) => {
                let keys = keys.clone();
                self.fetch(RatingKey::new(key.as_str()), move |shell, item| {
                    shell.open_player(item, Purpose::Check);
                    shell.press_later(keys);
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn session(&self) -> Option<Session> {
        self.session.borrow().clone()
    }

    /// Asks the server something and shows the answer, twice over: first
    /// what it answered the last time the app ran, so the screen is full at
    /// once, then what it answers now.
    fn load<T, F>(
        self: &Rc<Self>,
        session: &Session,
        ask: impl Fn(MediaServer) -> F,
        show: impl Fn(T) + 'static,
    ) where
        T: Send + 'static,
        F: Future<Output = plex::Result<T>> + Send + 'static,
    {
        let show = Rc::new(show);
        // Set once the server's own answer is on screen, so that a late
        // answer from last time cannot replace it.
        let is_current = Rc::new(Cell::new(false));
        let was_shown = Rc::new(Cell::new(false));

        let (show_old, current, shown) =
            (Rc::clone(&show), Rc::clone(&is_current), Rc::clone(&was_shown));
        self.background.run(self.main_thread, ask(session.server.remembered()), move |answer| {
            if let (Ok(answer), false) = (answer, current.get()) {
                shown.set(true);
                show_old(answer);
            }
        });

        let shell = Rc::clone(self);
        self.background.run(self.main_thread, ask(session.server.clone()), move |answer| {
            match answer {
                Ok(answer) => {
                    is_current.set(true);
                    show(answer);
                }
                // What is on screen from last time is better than an error.
                Err(error) if was_shown.get() => eprintln!("plex-rust: {}", with_causes(&error)),
                Err(error) => shell.show_message(&with_causes(&error)),
            }
        });
    }

    /// Shows a place from the sidebar.
    fn go_to(self: &Rc<Self>, destination: &Destination) {
        *self.destination.borrow_mut() = destination.clone();
        self.trail.borrow_mut().clear();
        let Some(session) = self.session() else {
            return;
        };
        match destination {
            Destination::Home => self.show_home(&session),
            Destination::Section(section) => self.show_library(&session, section),
            Destination::Downloads => self.show_message("Downloads are not built yet"),
        }
    }

    fn show_home(self: &Rc<Self>, session: &Session) {
        let posters = self.posters(session, Arrangement::Shelves, || {});
        self.load(
            session,
            |server| async move { server.home(SHELF_LENGTH).await },
            move |hubs: Vec<Hub>| {
                let shelf = |hub: Hub| Shelf { title: hub.title, items: hub.items };
                posters.show(hubs.into_iter().map(shelf).collect());
            },
        );
    }

    fn show_library(self: &Rc<Self>, session: &Session, section: &Section) {
        let more = Rc::downgrade(self);
        let more_of = section.clone();
        let posters = self.posters(session, Arrangement::Grid(Shape::Poster), move || {
            if let Some(shell) = more.upgrade() {
                shell.load_page(&more_of);
            }
        });
        let section = section.clone();
        self.load(
            session,
            move |server| {
                let section = section.clone();
                async move { server.section_items(&section, 0, PAGE_SIZE).await }
            },
            move |page: Page| posters.show(vec![Shelf { title: String::new(), items: page.items }]),
        );
    }

    /// Fetches the next page of `section` and adds it to the grid on screen.
    fn load_page(self: &Rc<Self>, section: &Section) {
        let (Some(session), Place::Posters(posters)) = (self.session(), &*self.place.borrow())
        else {
            return;
        };
        let posters = posters.clone();
        let start = u32::try_from(posters.len()).unwrap_or(u32::MAX);
        let (server, section) = (session.server.clone(), section.clone());
        let load = async move { server.section_items(&section, start, PAGE_SIZE).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, load, move |page| match page {
            Ok(page) => posters.append(page.items),
            Err(error) => shell.show_message(&with_causes(&error)),
        });
    }

    /// Puts a new, empty collection in the content area.
    fn posters(
        self: &Rc<Self>,
        session: &Session,
        arrangement: Arrangement,
        on_near_end: impl Fn() + 'static,
    ) -> Retained<Posters> {
        let opened = Rc::downgrade(self);
        let on_open = move |item: &Item| {
            if let Some(shell) = opened.upgrade() {
                shell.open(item);
            }
        };
        let images = Rc::clone(&session.images);
        let posters = Posters::new(self.main_thread, images, arrangement, on_open, on_near_end);
        layout::fill(&self.content, posters.view());
        *self.place.borrow_mut() = Place::Posters(posters.clone());
        posters
    }

    /// Goes to the detail page of `item`, remembering the way back.
    fn open(self: &Rc<Self>, item: &Item) {
        self.trail.borrow_mut().push(item.clone());
        self.show_detail(item);
    }

    /// Leaves the detail page for the one it was opened from, or for the
    /// sidebar's place if it was opened from there.
    fn back(self: &Rc<Self>) {
        let previous = {
            let mut trail = self.trail.borrow_mut();
            trail.pop();
            trail.last().cloned()
        };
        match previous {
            Some(item) => self.show_detail(&item),
            None => {
                let destination = self.destination.borrow().clone();
                self.go_to(&destination);
            }
        }
    }

    /// Fetches the page on screen again, after something about its item has
    /// changed on the server, such as how far it was watched.
    fn refresh_page(self: &Rc<Self>) {
        let Some(key) = self.trail.borrow().last().map(|item| item.rating_key.clone()) else {
            return;
        };
        self.fetch(key, |shell, item| {
            if let Some(top) = shell.trail.borrow_mut().last_mut() {
                *top = item.clone();
            }
            shell.show_detail(item);
        });
    }

    /// Shows the detail page of `item`.
    fn show_detail(self: &Rc<Self>, item: &Item) {
        let Some(session) = self.session() else {
            return;
        };
        let (opened, played, back) =
            (Rc::downgrade(self), Rc::downgrade(self), Rc::downgrade(self));
        let detail = Detail::new(
            self.main_thread,
            item,
            &session.images,
            move |child| {
                if let Some(shell) = opened.upgrade() {
                    shell.open(child);
                }
            },
            move |item| {
                if let Some(shell) = played.upgrade() {
                    shell.play(item, Purpose::Watching);
                }
            },
            move || {
                if let Some(shell) = back.upgrade() {
                    shell.back();
                }
            },
        );
        layout::fill(&self.content, detail.view());
        *self.place.borrow_mut() = Place::Detail(detail.clone());

        if !matches!(item.kind, Kind::Show | Kind::Season) {
            return;
        }
        let key = item.rating_key.clone();
        self.load(
            &session,
            move |server| {
                let key = key.clone();
                async move { server.children(&key).await }
            },
            move |children: Vec<Item>| detail.show_children(children),
        );
    }

    /// Fetches the item with `key` and hands it to `then`.
    fn fetch(self: &Rc<Self>, key: RatingKey, then: impl FnOnce(&Rc<Self>, &Item) + 'static) {
        let Some(session) = self.session() else {
            return;
        };
        let server = session.server.clone();
        let load = async move { server.item(&key).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, load, move |item| match item {
            Ok(item) => then(&shell, &item),
            Err(error) => shell.show_message(&with_causes(&error)),
        });
    }

    /// Plays `item`, first fetching it in full: a listing leaves out the
    /// intro and credits markers that the player offers to skip.
    fn play(self: &Rc<Self>, item: &Item, purpose: Purpose) {
        // A second click on Play while the first is still fetching must
        // not open a second player behind the first.
        if self.playing.borrow().is_some() || self.opening_player.replace(true) {
            return;
        }
        let Some(session) = self.session() else {
            self.opening_player.set(false);
            return;
        };
        let (server, key) = (session.server.clone(), item.rating_key.clone());
        let load = async move { server.item(&key).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, load, move |item| {
            shell.opening_player.set(false);
            match item {
                Ok(item) => shell.open_player(&item, purpose),
                Err(error) => shell.show_message(&with_causes(&error)),
            }
        });
    }

    /// Replaces the whole window content with the player, playing `item`.
    fn open_player(self: &Rc<Self>, item: &Item, purpose: Purpose) {
        let Some(session) = self.session() else {
            return;
        };
        // Replacing a player without closing it would leave it running
        // unseen, with nothing left that could stop it.
        if self.playing.borrow().is_some() {
            return;
        }
        let closed = Rc::downgrade(self);
        let screen = PlayerScreen::open(
            self.main_thread,
            Rc::clone(&self.background),
            session.server.clone(),
            item,
            self.settings.get(),
            purpose,
            move |outcome| {
                if let Some(shell) = closed.upgrade() {
                    shell.player_closed(outcome, purpose);
                }
            },
        );
        match screen {
            Ok(screen) => {
                let controller = view_controller(self.main_thread);
                controller.setView(screen.view());
                self.set_window_content(&controller);
                screen.focus();
                *self.playing.borrow_mut() = Some(screen);
            }
            Err(error) => self.show_message(&with_causes(&error)),
        }
    }

    fn player_closed(self: &Rc<Self>, outcome: Outcome, purpose: Purpose) {
        let Some(screen) = self.playing.borrow_mut().take() else {
            return;
        };
        self.keep_settings(&screen);
        self.set_window_content(&self.split);
        // The page behind the player still says where the item was left
        // before it was played.
        if purpose == Purpose::Watching {
            self.refresh_page();
        }
        match outcome {
            Outcome::Closed => {}
            Outcome::Finished if screen.item().kind == Kind::Episode => {
                self.play_episode_after(screen.item(), purpose);
            }
            Outcome::Finished => {}
            Outcome::Previous => self.play_episode_before(screen.item(), purpose),
            Outcome::Failed(reason) => {
                let title = &screen.item().title;
                self.show_message(&format!("{title} could not be played: {reason}"));
            }
        }
    }

    /// Takes over the speed and volume the viewer set in `screen`, for
    /// whatever is played next and for the next launch.
    fn keep_settings(&self, screen: &PlayerScreen) {
        let settings = match screen.settings() {
            Ok(Some(settings)) => settings,
            Ok(None) => return,
            Err(error) => return eprintln!("plex-rust: {error}"),
        };
        self.settings.set(settings);
        if let Err(error) = settings.save() {
            eprintln!("plex-rust: {}", with_causes(&error));
        }
    }

    /// Plays the episode that follows `episode` in its season, if there is
    /// one. The end of a season is a natural place to stop.
    fn play_episode_after(self: &Rc<Self>, episode: &Item, purpose: Purpose) {
        let (Some(session), Some(season), Some(number)) =
            (self.session(), episode.parent_rating_key.clone(), episode.index)
        else {
            return;
        };
        let server = session.server.clone();
        let load = async move { server.children(&season).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, load, move |episodes| match episodes {
            Ok(episodes) => {
                // The lowest number above this one, not this one plus one: a
                // season can have a gap, and one file can hold two episodes.
                let later = episodes.iter().filter(|episode| episode.index > Some(number));
                let next = later.min_by_key(|episode| episode.index);
                if let Some(next) = next {
                    shell.play(next, purpose);
                }
            }
            Err(error) => shell.show_message(&with_causes(&error)),
        });
    }

    /// Plays the episode that comes before `episode` in its season, if
    /// there is one.
    fn play_episode_before(self: &Rc<Self>, episode: &Item, purpose: Purpose) {
        let (Some(session), Some(season), Some(number)) =
            (self.session(), episode.parent_rating_key.clone(), episode.index)
        else {
            return;
        };
        let server = session.server.clone();
        let load = async move { server.children(&season).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, load, move |episodes| match episodes {
            Ok(episodes) => {
                // The highest number below this one, for the reasons given
                // in `play_episode_after`. An episode with no number sorts
                // as `None` and is not one of them.
                let earlier = episodes
                    .iter()
                    .filter(|episode| episode.index.is_some_and(|index| index < number));
                if let Some(previous) = earlier.max_by_key(|episode| episode.index) {
                    shell.play(previous, purpose);
                }
            }
            Err(error) => shell.show_message(&with_causes(&error)),
        });
    }

    /// Presses `keys` in the player once it has had time to open its file.
    fn press_later(self: &Rc<Self>, keys: Vec<Key>) {
        // A timer can only be created on the runtime's own threads.
        let wait = async { tokio::time::sleep(SNAPSHOT_KEYS_AFTER).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, wait, move |()| {
            // Cloned so that a key that closes the player finds no borrow.
            let screen = shell.playing.borrow().clone();
            if let Some(screen) = screen {
                for key in keys {
                    screen.press(key);
                }
            }
        });
    }

    /// Swaps what the window shows without letting it change size. A
    /// window otherwise shrinks to fit a controller whose view has no size
    /// of its own.
    fn set_window_content(&self, controller: &NSViewController) {
        let frame = self.window.frame();
        self.window.setContentViewController(Some(controller));
        self.window.setFrame_display(frame, true);
    }

    fn snapshot_and_exit(&self, path: &Path) {
        // The content view's parent is the window's frame, which also draws
        // the background and the title bar.
        // SAFETY: the content view is in a window, so its parent is alive.
        let frame = self.window.contentView().and_then(|view| unsafe { view.superview() });
        let result = match frame {
            Some(view) => {
                // A window that was never on screen has not laid out yet.
                view.layoutSubtreeIfNeeded();
                snapshot::write_png(&view, path)
            }
            None => Err(Error::Snapshot),
        };
        if let Err(error) = result {
            eprintln!("plex-rust: {}", with_causes(&error));
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    fn or_show(&self, result: Result<()>) {
        if let Err(error) = result {
            self.show_message(&with_causes(&error));
        }
    }

    /// Fills the content area with one line of text.
    fn show_message(&self, message: &str) {
        let label = NSTextField::labelWithString(&NSString::from_str(message), self.main_thread);
        layout::center(&self.content, &label);
        *self.place.borrow_mut() = Place::Message;
    }
}

/// Where the server's answers are kept between launches.
fn responses_directory() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or(Error::NoHome)?;
    let path = PathBuf::from(home).join("Library/Caches/plex-rust/responses");
    std::fs::create_dir_all(&path).map_err(|source| Error::File { path: path.clone(), source })?;
    Ok(path)
}

fn window(main_thread: MainThreadMarker) -> Retained<NSWindow> {
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::FullSizeContentView;
    // SAFETY: the arguments are valid, and `setReleasedWhenClosed(false)`
    // below gives the window the ownership rules `Retained` expects.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(main_thread),
            NSRect::new(NSPoint::ZERO, theme::WINDOW_SIZE),
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: called before the window can be closed.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(ns_string!("plex-rust"));
    window.setContentMinSize(theme::WINDOW_MIN_SIZE);
    window
}

fn view_controller(main_thread: MainThreadMarker) -> Retained<NSViewController> {
    NSViewController::initWithNibName_bundle(NSViewController::alloc(main_thread), None, None)
}

/// The menu bar. Without it there is no Quit and no Cmd-Q.
fn main_menu(main_thread: MainThreadMarker) -> Retained<NSMenu> {
    let app_menu = NSMenu::new(main_thread);
    // SAFETY: `terminate:` is an NSApplication action, found through the
    // responder chain.
    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(main_thread),
            ns_string!("Quit plex-rust"),
            Some(sel!(terminate:)),
            ns_string!("q"),
        )
    };
    app_menu.addItem(&quit);

    let app_item = NSMenuItem::new(main_thread);
    app_item.setSubmenu(Some(&app_menu));
    let menu = NSMenu::new(main_thread);
    menu.addItem(&app_item);
    menu
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `AppDelegate`
    // does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustShellDelegate"]
    #[ivars = Rc<Shell>]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _app: &NSApplication) -> bool {
            true
        }

        /// Quitting mid-film must still leave the server with the place
        /// the viewer got to.
        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            if let Some(screen) = self.ivars().playing.borrow_mut().take() {
                self.ivars().keep_settings(&screen);
                screen.close_for_quit();
            }
        }
    }
);

impl AppDelegate {
    fn new(main_thread: MainThreadMarker, shell: Rc<Shell>) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(shell);
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}
