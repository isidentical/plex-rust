//! The main window: sidebar on the left, the chosen place on the right.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameDarkAqua, NSApplication, NSApplicationActivationPolicy,
    NSBackingStoreType, NSMenu, NSMenuItem, NSSplitViewController, NSSplitViewItem, NSTextField,
    NSView, NSViewController, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSString, ns_string};
use plex::MediaServer;
use plex::model::Section;

use super::home::Home;
use super::layout;
use super::sidebar::{Destination, Sidebar};
use super::snapshot;
use super::theme;
use crate::account;
use crate::background::Background;
use crate::error::{Error, Result, with_causes};
use crate::images::Images;

/// How many items each shelf of the home screen starts with.
const SHELF_LENGTH: u32 = 20;

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
}

/// What exists only while connected to a server.
struct Session {
    server: MediaServer,
    home: Retained<Home>,
}

/// How long a snapshot waits after the window has its content, so that
/// work started by showing it, such as loading artwork, can finish.
const SNAPSHOT_SETTLE: Duration = Duration::from_secs(3);

/// Opens the main window and runs the app until it quits.
///
/// With `snapshot`, the window stays off screen: once it has loaded, its
/// content is saved to that path as a PNG and the app exits.
pub fn run(snapshot: Option<PathBuf>) -> Result<()> {
    let main_thread =
        MainThreadMarker::new().unwrap_or_else(|| panic!("main() runs on the main thread"));
    let (client, token) = account::signed_in()?;
    let background = Rc::new(Background::new()?);

    let app = NSApplication::sharedApplication(main_thread);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.setMainMenu(Some(&main_menu(main_thread)));
    // A film library looks right on dark, whatever the system is set to.
    // SAFETY: the name is a constant that AppKit provides.
    app.setAppearance(
        NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }).as_deref(),
    );

    let shell = Shell::new(main_thread, Rc::clone(&background));
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
    let settle = Rc::clone(&background);
    background.run(main_thread, connect, move |result| {
        match result {
            Ok((server, sections)) => {
                if let Err(error) = loaded.connected(server, sections) {
                    loaded.show_message(&with_causes(&error));
                }
            }
            Err(error) => loaded.show_message(&with_causes(&error)),
        }
        if let Some(path) = snapshot {
            // A timer can only be created on the runtime's own threads.
            let wait = async { tokio::time::sleep(SNAPSHOT_SETTLE).await };
            settle.run(main_thread, wait, move |()| loaded.snapshot_and_exit(&path));
        }
    });

    app.run();
    Ok(())
}

impl Shell {
    fn new(main_thread: MainThreadMarker, background: Rc<Background>) -> Rc<Self> {
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

        let split = NSSplitViewController::new(main_thread);
        split.addSplitViewItem(&sidebar_item);
        split.addSplitViewItem(&NSSplitViewItem::splitViewItemWithViewController(
            &content_controller,
        ));
        shell.window.setContentViewController(Some(&split));
        shell.window.setContentSize(theme::WINDOW_SIZE);
        shell.window.center();

        // Cannot fail: the cell was created empty a few lines up.
        let _ = shell.sidebar.set(sidebar);
        shell.show_message("Connecting to your server");
        shell
    }

    fn connected(self: &Rc<Self>, server: MediaServer, sections: Vec<Section>) -> Result<()> {
        let images = Images::new(
            self.main_thread,
            Rc::clone(&self.background),
            server.clone(),
        )?;
        let home = Home::new(self.main_thread, Rc::new(images));
        // Cannot fail: the server is connected to once per launch.
        let _ = self.session.set(Session { server, home });
        self.show_destinations(sections);
        Ok(())
    }

    fn go_to(self: &Rc<Self>, destination: &Destination) {
        match (destination, self.session.get()) {
            (Destination::Home, Some(session)) => self.show_home(session),
            _ => self.show_message(destination.title()),
        }
    }

    fn show_home(self: &Rc<Self>, session: &Session) {
        layout::fill(&self.content, session.home.view());

        let server = session.server.clone();
        let shell = Rc::clone(self);
        let load = async move { server.home(SHELF_LENGTH).await };
        self.background.run(self.main_thread, load, move |hubs| {
            match (hubs, shell.session.get()) {
                (Ok(hubs), Some(session)) => session.home.show(hubs),
                (Err(error), _) => shell.show_message(&with_causes(&error)),
                (Ok(_), None) => {}
            }
        });
    }

    fn show_destinations(&self, sections: Vec<Section>) {
        let mut destinations = vec![Destination::Home];
        destinations.extend(sections.into_iter().map(Destination::Section));
        destinations.push(Destination::Downloads);
        if let Some(sidebar) = self.sidebar.get() {
            sidebar.show(destinations);
        }
    }

    fn snapshot_and_exit(&self, path: &Path) {
        // The content view's parent is the window's frame, which also draws
        // the background and the title bar.
        // SAFETY: the content view is in a window, so its parent is alive.
        let frame = self
            .window
            .contentView()
            .and_then(|view| unsafe { view.superview() });
        let result = match frame {
            Some(view) => {
                // A window that was never on screen has not laid out yet.
                view.layoutSubtreeIfNeeded();
                snapshot::write_png(&view, path)
            }
            None => Err(Error::Snapshot),
        };
        if let Err(error) = result {
            eprintln!("reel: {}", with_causes(&error));
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    /// Fills the content area with one line of text.
    fn show_message(&self, message: &str) {
        let label = NSTextField::labelWithString(&NSString::from_str(message), self.main_thread);
        layout::center(&self.content, &label);
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
    window.setTitle(ns_string!("Reel"));
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
            ns_string!("Quit Reel"),
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
