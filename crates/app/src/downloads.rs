//! Downloads as the app runs them: what is kept, what is on its way, and
//! word to the screens whenever either changes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use objc2::MainThreadMarker;
use plex::model::{Kind, RatingKey};
use plex::{MediaServer, Transfer};

use crate::background::Background;
use crate::error::{Error, Result, with_causes};
use crate::store::{Download, OnDisk, Store};

/// The size the poster of a download is kept at, in pixels: the one a
/// detail page asks for, which is the largest a poster is shown.
const POSTER_SIZE: (u32, u32) = (464, 696);

/// How often the screens hear how far the running downloads have got.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(1);

/// Where a download stands.
#[derive(Clone)]
pub enum State {
    /// On its way.
    Downloading(Arc<Transfer>),
    /// Stopped short with this many bytes, and why, if it stopped during
    /// this run of the app. Starting it again carries on from there.
    Incomplete {
        received: u64,
        error: Option<String>,
    },
    Downloaded,
}

#[derive(Clone)]
pub struct Entry {
    pub download: Arc<Download>,
    pub state: State,
}

pub struct Downloads {
    main_thread: MainThreadMarker,
    background: Rc<Background>,
    store: Store,
    entries: RefCell<Vec<Entry>>,
    on_change: RefCell<Option<Rc<dyn Fn()>>>,
    /// Whether the screens are being told of progress once a second.
    reporting: Cell<bool>,
}

impl Downloads {
    /// The downloads found on disk. None of them is on its way: one that
    /// was cut short by the app quitting waits to be started again.
    pub fn open(main_thread: MainThreadMarker, background: Rc<Background>) -> Result<Rc<Self>> {
        let store = Store::open()?;
        let entry = |download: Download| {
            let state = match download.on_disk()? {
                OnDisk::Whole => State::Downloaded,
                OnDisk::Partial(received) => State::Incomplete { received, error: None },
            };
            Ok(Entry { download: Arc::new(download), state })
        };
        let entries = store.list()?.into_iter().map(entry).collect::<Result<_>>()?;
        Ok(Rc::new(Self {
            main_thread,
            background,
            store,
            entries: RefCell::new(entries),
            on_change: RefCell::new(None),
            reporting: Cell::new(false),
        }))
    }

    /// Has `on_change` called whenever a download starts, ends, is removed
    /// or, once a second, has got further.
    pub fn listen(&self, on_change: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Rc::new(on_change));
    }

    pub fn entries(&self) -> Vec<Entry> {
        self.entries.borrow().clone()
    }

    pub fn find(&self, key: &RatingKey) -> Option<Entry> {
        self.entries.borrow().iter().find(|entry| entry.download.key() == key).cloned()
    }

    /// Starts downloading `item`, the server's full description of a film
    /// or an episode that has no download yet, from `server`.
    pub fn add(self: &Rc<Self>, server: &MediaServer, item: &serde_json::Value) -> Result<()> {
        let download = Arc::new(self.store.add(item)?);
        let key = download.key().clone();
        let state = State::Incomplete { received: 0, error: None };
        self.entries.borrow_mut().push(Entry { download, state });
        self.start(server, &key);
        Ok(())
    }

    /// Carries on with the download of the item with `key`, if it stopped
    /// short.
    pub fn start(self: &Rc<Self>, server: &MediaServer, key: &RatingKey) {
        let Some(Entry { download, state: State::Incomplete { .. } }) = self.find(key) else {
            return;
        };
        let transfer = Arc::new(Transfer::default());
        self.set_state(key, State::Downloading(Arc::clone(&transfer)));

        let (server, this, key) = (server.clone(), Rc::clone(self), key.clone());
        let work = async move { fetch(&server, &download, &transfer).await };
        self.background.run(self.main_thread, work, move |fetched| this.finished(&key, fetched));
        self.changed();
        if !self.reporting.replace(true) {
            self.report_progress();
        }
    }

    /// Deletes the download of the item with `key`, stopping it first if
    /// it is on its way.
    pub fn remove(&self, key: &RatingKey) {
        match self.find(key) {
            // The download ends as cancelled, and is deleted when it has.
            Some(Entry { state: State::Downloading(transfer), .. }) => transfer.cancel(),
            Some(entry) => self.delete(&entry.download),
            None => {}
        }
    }

    fn finished(&self, key: &RatingKey, fetched: Result<()>) {
        let Some(Entry { download, state: State::Downloading(transfer) }) = self.find(key) else {
            return;
        };
        match fetched {
            Ok(()) => self.set_state(key, State::Downloaded),
            Err(Error::Plex(plex::Error::Cancelled)) => return self.delete(&download),
            Err(error) => {
                let error = with_causes(&error);
                eprintln!("plex-rust: {} was not downloaded: {error}", download.item.title);
                let received = transfer.received();
                self.set_state(key, State::Incomplete { received, error: Some(error) });
            }
        }
        self.changed();
    }

    fn delete(&self, download: &Download) {
        // What could not be deleted is still there, and stays listed.
        if let Err(error) = self.store.remove(download) {
            return eprintln!("plex-rust: {}", with_causes(&error));
        }
        self.entries.borrow_mut().retain(|entry| entry.download.key() != download.key());
        self.changed();
    }

    fn set_state(&self, key: &RatingKey, state: State) {
        let mut entries = self.entries.borrow_mut();
        if let Some(entry) = entries.iter_mut().find(|entry| entry.download.key() == key) {
            entry.state = state;
        }
    }

    fn changed(&self) {
        // Cloned so that the listener is free to call back into this.
        let on_change = self.on_change.borrow().clone();
        if let Some(on_change) = on_change {
            on_change();
        }
    }

    /// Tells the listener of progress now and then, for as long as
    /// anything is on its way. Nothing runs while nothing is.
    fn report_progress(self: &Rc<Self>) {
        let downloading = |entry: &Entry| matches!(entry.state, State::Downloading(_));
        if !self.entries.borrow().iter().any(downloading) {
            return self.reporting.set(false);
        }
        // A timer can only be created on the runtime's own threads.
        let wait = async { tokio::time::sleep(PROGRESS_INTERVAL).await };
        let this = Rc::clone(self);
        self.background.run(self.main_thread, wait, move |()| {
            this.changed();
            this.report_progress();
        });
    }
}

/// Brings the file of `download` to disk from `server`, carrying on from
/// what an earlier attempt left, and its poster with it. `transfer` hears
/// how far the file has got.
pub async fn fetch(server: &MediaServer, download: &Download, transfer: &Transfer) -> Result<()> {
    let file_error = |path: &std::path::Path| {
        let path = path.to_owned();
        move |source| Error::File { path, source }
    };
    if download.on_disk()? == OnDisk::Whole {
        return Ok(());
    }
    // An episode's own artwork is a wide frame, so it borrows its show's.
    let item = &download.item;
    let poster = match item.kind {
        Kind::Episode => item.grandparent_thumb.as_deref().or(item.thumb.as_deref()),
        _ => item.thumb.as_deref(),
    };
    let missing =
        !tokio::fs::try_exists(&download.poster).await.map_err(file_error(&download.poster))?;
    if let (Some(poster), true) = (poster, missing) {
        let (width, height) = POSTER_SIZE;
        match server.image(poster, width, height).await {
            Ok(jpeg) => tokio::fs::write(&download.poster, jpeg)
                .await
                .map_err(file_error(&download.poster))?,
            // The film is what was asked for. The list shows it without a
            // picture.
            Err(error) => eprintln!("plex-rust: poster not kept: {}", with_causes(&error)),
        }
    }

    server.download(&download.part, &download.partial, transfer).await?;
    tokio::fs::rename(&download.partial, &download.file)
        .await
        .map_err(file_error(&download.partial))
}
