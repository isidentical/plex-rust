//! The main window: sidebar on the left, the chosen place on the right.

use std::cell::{Cell, OnceCell, RefCell};
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
use plex::model::{Item, Kind, RatingKey, Section};

use super::detail::Detail;
use super::input::Key;
use super::layout;
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
    /// Set once the server has answered.
    session: OnceCell<Session>,
    /// What the content area shows. Held here because views do not keep
    /// their data sources and button targets alive.
    place: RefCell<Place>,
    /// Where Back on a detail page returns to.
    destination: RefCell<Destination>,
    /// The sidebar and content area, which the player replaces while open.
    split: Retained<NSSplitViewController>,
    playing: RefCell<Option<Retained<PlayerScreen>>>,
    /// Speed and volume, kept from one item to the next and across launches.
    settings: Cell<Settings>,
}

/// What exists only while connected to a server.
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

    let connect = async move {
        let servers = plex::discovery::servers(&client, &token).await?;
        let server = servers.first().ok_or(Error::NoServer)?;
        let server = MediaServer::connect(&client, server, None).await?;
        let sections = server.sections().await?;
        Ok::<_, Error>((server, sections))
    };
    let loaded = Rc::clone(&shell);
    background.run(main_thread, connect, move |result| {
        let connected = result.and_then(|(server, sections)| {
            let subject = snapshot.as_ref().map(|(subject, _)| subject);
            loaded.connected(server, sections, subject)
        });
        if let Err(error) = connected {
            loaded.show_message(&with_causes(&error));
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
            session: OnceCell::new(),
            place: RefCell::new(Place::Message),
            destination: RefCell::new(Destination::Home),
            split: NSSplitViewController::new(main_thread),
            playing: RefCell::new(None),
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
        // Cannot fail: the server is connected to once per launch.
        let _ = self.session.set(Session { server, images: Rc::new(images) });

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

    /// Shows a place from the sidebar.
    fn go_to(self: &Rc<Self>, destination: &Destination) {
        *self.destination.borrow_mut() = destination.clone();
        let Some(session) = self.session.get() else {
            return;
        };
        match destination {
            Destination::Home => self.show_home(session),
            Destination::Section(section) => self.show_library(session, section),
            Destination::Downloads => self.show_message("Downloads are not built yet"),
        }
    }

    fn show_home(self: &Rc<Self>, session: &Session) {
        let posters = self.posters(session, Arrangement::Shelves, || {});
        let server = session.server.clone();
        let load = async move { server.home(SHELF_LENGTH).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, load, move |hubs| match hubs {
            Ok(hubs) => posters.show(
                hubs.into_iter().map(|hub| Shelf { title: hub.title, items: hub.items }).collect(),
            ),
            Err(error) => shell.show_message(&with_causes(&error)),
        });
    }

    fn show_library(self: &Rc<Self>, session: &Session, section: &Section) {
        let more = Rc::downgrade(self);
        let more_of = section.clone();
        let posters = self.posters(session, Arrangement::Grid(Shape::Poster), move || {
            if let Some(shell) = more.upgrade() {
                shell.load_page(&more_of);
            }
        });
        posters.show(vec![Shelf { title: String::new(), items: Vec::new() }]);
        self.load_page(section);
    }

    /// Fetches the next page of `section` into the grid on screen.
    fn load_page(self: &Rc<Self>, section: &Section) {
        let (Some(session), Place::Posters(posters)) = (self.session.get(), &*self.place.borrow())
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

    /// Shows the detail page of `item`.
    fn open(self: &Rc<Self>, item: &Item) {
        let Some(session) = self.session.get() else {
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
                    let destination = shell.destination.borrow().clone();
                    shell.go_to(&destination);
                }
            },
        );
        layout::fill(&self.content, detail.view());
        *self.place.borrow_mut() = Place::Detail(detail.clone());

        if !matches!(item.kind, Kind::Show | Kind::Season) {
            return;
        }
        let (server, key) = (session.server.clone(), item.rating_key.clone());
        let load = async move { server.children(&key).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, load, move |children| match children {
            Ok(children) => detail.show_children(children),
            Err(error) => shell.show_message(&with_causes(&error)),
        });
    }

    /// Fetches the item with `key` and hands it to `then`.
    fn fetch(self: &Rc<Self>, key: RatingKey, then: impl FnOnce(&Rc<Self>, &Item) + 'static) {
        let Some(session) = self.session.get() else {
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
        self.fetch(item.rating_key.clone(), move |shell, item| shell.open_player(item, purpose));
    }

    /// Replaces the whole window content with the player, playing `item`.
    fn open_player(self: &Rc<Self>, item: &Item, purpose: Purpose) {
        let Some(session) = self.session.get() else {
            return;
        };
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
        if outcome == Outcome::Finished && screen.item().kind == Kind::Episode {
            self.play_episode_after(screen.item(), purpose);
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
            (self.session.get(), episode.parent_rating_key.clone(), episode.index)
        else {
            return;
        };
        let server = session.server.clone();
        let load = async move { server.children(&season).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, load, move |episodes| match episodes {
            Ok(episodes) => {
                let next = episodes.iter().find(|episode| episode.index == Some(number + 1));
                if let Some(next) = next {
                    shell.play(next, purpose);
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

    /// Fills the content area with one line of text.
    fn show_message(&self, message: &str) {
        let label = NSTextField::labelWithString(&NSString::from_str(message), self.main_thread);
        layout::center(&self.content, &label);
        *self.place.borrow_mut() = Place::Message;
    }
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
