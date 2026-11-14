//! The window's toolbar: back and forward on the left, search on the right.

use objc2::rc::Retained;
use objc2::runtime::{ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControl, NSControlTextEditingDelegate, NSImage, NSSearchField, NSSearchFieldDelegate,
    NSSearchToolbarItem, NSTextFieldDelegate, NSTextView, NSToolbar, NSToolbarDelegate,
    NSToolbarDisplayMode, NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem,
    NSToolbarItemIdentifier, NSToolbarSidebarTrackingSeparatorItemIdentifier,
    NSToolbarToggleSidebarItemIdentifier, NSWindow, NSWindowTitleVisibility, NSWindowToolbarStyle,
};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSString, ns_string};

use super::callback::Callback;

const BACK: &str = "back";
const FORWARD: &str = "forward";
const SEARCH: &str = "search";

pub struct Ivars {
    toolbar: Retained<NSToolbar>,
    back: Retained<NSToolbarItem>,
    forward: Retained<NSToolbarItem>,
    search: Retained<NSSearchToolbarItem>,
    on_leave_search: Box<dyn Fn()>,
    /// Kept because toolbar items do not retain their targets.
    _targets: [Retained<Callback>; 3],
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Toolbar` does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustToolbar"]
    #[ivars = Ivars]
    pub struct Toolbar;

    unsafe impl NSObjectProtocol for Toolbar {}

    unsafe impl NSToolbarDelegate for Toolbar {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(
            &self,
            _toolbar: &NSToolbar,
            identifier: &NSToolbarItemIdentifier,
            _inserting: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            let ivars = self.ivars();
            match identifier.to_string().as_str() {
                BACK => Some(ivars.back.clone()),
                FORWARD => Some(ivars.forward.clone()),
                SEARCH => Some(Retained::into_super(ivars.search.clone())),
                // The system makes its own items itself.
                _ => None,
            }
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_items(
            &self,
            _toolbar: &NSToolbar,
        ) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            identifiers()
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_items(
            &self,
            _toolbar: &NSToolbar,
        ) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            identifiers()
        }
    }

    unsafe impl NSControlTextEditingDelegate for Toolbar {
        /// The Down arrow steps out of the search field into what it found,
        /// as it does from the search field of Finder or Music. A field
        /// of one line has no other use for the key.
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _control: &NSControl, _editor: &NSTextView, command: Sel) -> bool {
            let leaves = command == sel!(moveDown:);
            if leaves {
                (self.ivars().on_leave_search)();
            }
            leaves
        }
    }

    unsafe impl NSTextFieldDelegate for Toolbar {}

    unsafe impl NSSearchFieldDelegate for Toolbar {}
);

impl Toolbar {
    /// `on_search` is called with the text of the search field as the
    /// viewer types, after each short pause. `on_leave_search` is called
    /// when the viewer steps out of the field towards the content.
    pub fn new(
        main_thread: MainThreadMarker,
        on_back: impl Fn() + 'static,
        on_forward: impl Fn() + 'static,
        on_search: impl Fn(&str) + 'static,
        on_leave_search: impl Fn() + 'static,
    ) -> Retained<Self> {
        let went_back = Callback::new(main_thread, move |_| on_back());
        let went_forward = Callback::new(main_thread, move |_| on_forward());
        let searched = Callback::new(main_thread, move |sender| {
            let field = sender.and_then(|sender| sender.downcast_ref::<NSSearchField>());
            if let Some(field) = field {
                on_search(field.stringValue().to_string().trim());
            }
        });

        let back = arrow(BACK, "chevron.left", "Back", &went_back, main_thread);
        let forward = arrow(FORWARD, "chevron.right", "Forward", &went_forward, main_thread);
        let search = NSSearchToolbarItem::initWithItemIdentifier(
            NSSearchToolbarItem::alloc(main_thread),
            &NSString::from_str(SEARCH),
        );
        let field = search.searchField();
        field.setPlaceholderString(Some(ns_string!("Search")));
        // SAFETY: the toolbar keeps the target for as long as it keeps the
        // field.
        unsafe {
            field.setTarget(Some(&searched));
            field.setAction(Some(Callback::action()));
        }

        let toolbar =
            NSToolbar::initWithIdentifier(NSToolbar::alloc(main_thread), ns_string!("main"));
        toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
        let this = Self::alloc(main_thread).set_ivars(Ivars {
            toolbar,
            back,
            forward,
            search,
            on_leave_search: Box::new(on_leave_search),
            _targets: [went_back, went_forward, searched],
        });
        // SAFETY: `init` is NSObject's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        // The toolbar does not retain its delegate. Whoever owns `Toolbar`
        // keeps it for as long as the window has the toolbar.
        this.ivars().toolbar.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        // SAFETY: the field holds its delegate weakly, and `this` implements
        // the protocol.
        unsafe {
            this.ivars().search.searchField().setDelegate(Some(ProtocolObject::from_ref(&*this)));
        }
        this
    }

    pub fn install(&self, window: &NSWindow) {
        window.setToolbar(Some(&self.ivars().toolbar));
        window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        // Every page says what it is in its own heading or in the sidebar,
        // and a title in the toolbar is one more thing for artwork to
        // scroll under. The window keeps its title for the Window menu.
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    }

    /// Shows or hides the toolbar. The player hides it.
    pub fn set_visible(&self, visible: bool) {
        self.ivars().toolbar.setVisible(visible);
    }

    /// Greys out the arrows that lead nowhere.
    pub fn set_history(&self, can_go_back: bool, can_go_forward: bool) {
        self.ivars().back.setEnabled(can_go_back);
        self.ivars().forward.setEnabled(can_go_forward);
    }

    /// Puts the keyboard in the search field.
    pub fn focus_search(&self) {
        let field = self.ivars().search.searchField();
        if let Some(window) = field.window() {
            window.makeFirstResponder(Some(&field));
        }
    }

    /// Writes `text` in the search field without searching for it.
    pub fn set_search_text(&self, text: &str) {
        self.ivars().search.searchField().setStringValue(&NSString::from_str(text));
    }
}

fn identifiers() -> Retained<NSArray<NSToolbarItemIdentifier>> {
    let (back, forward, search) =
        (NSString::from_str(BACK), NSString::from_str(FORWARD), NSString::from_str(SEARCH));
    // SAFETY: the identifiers are constants that AppKit provides.
    let (toggle, separator, space) = unsafe {
        (
            NSToolbarToggleSidebarItemIdentifier,
            NSToolbarSidebarTrackingSeparatorItemIdentifier,
            NSToolbarFlexibleSpaceItemIdentifier,
        )
    };
    NSArray::from_slice(&[toggle, separator, &*back, &*forward, space, &*search])
}

fn arrow(
    identifier: &str,
    symbol: &str,
    label: &str,
    target: &Callback,
    main_thread: MainThreadMarker,
) -> Retained<NSToolbarItem> {
    let item = NSToolbarItem::initWithItemIdentifier(
        NSToolbarItem::alloc(main_thread),
        &NSString::from_str(identifier),
    );
    // Without a size or weight of its own, so that the toolbar draws it
    // as it draws its own symbols, such as the sidebar's next to it.
    let symbol = NSString::from_str(symbol);
    item.setImage(
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&symbol, None).as_deref(),
    );
    item.setLabel(&NSString::from_str(label));
    item.setToolTip(Some(&NSString::from_str(label)));
    item.setBordered(true);
    // Keeps the arrows at the leading edge, next to the sidebar.
    item.setNavigational(true);
    // Enabled by `set_history`, not by asking the target.
    item.setAutovalidates(false);
    item.setEnabled(false);
    // SAFETY: `Toolbar` keeps the target for as long as it keeps the item.
    unsafe {
        item.setTarget(Some(target));
        item.setAction(Some(Callback::action()));
    }
    item
}
