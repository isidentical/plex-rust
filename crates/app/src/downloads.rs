//! Downloads as the app runs them: what is kept, what is on its way, and
//! word to the screens whenever either changes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use objc2::MainThreadMarker;
use objc2_foundation::NSUUID;
use plex::model::{Item, Kind, RatingKey};
use plex::playback::{PlaybackState, Progress};
use plex::{MediaServer, Transfer};

use crate::background::Background;
use crate::error::{Error, Result, with_causes};
use crate::store::{Download, OnDisk, Store, Unsent};

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

    /// The folder the downloads are in.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The item of `download` as it was last left: where a viewing on
    /// this Mac stopped, if the server has yet to hear of that.
    pub fn as_left(&self, download: &Download) -> Result<Item> {
        self.store.as_left(download)
    }

    /// The downloaded episodes of `show` in the order they are watched
    /// in, each as it was last left. They are what one episode leads on
    /// to while the server is not there to list the show.
    pub fn episodes(&self, show: &RatingKey) -> Result<Vec<Item>> {
        let of_show = |entry: &&Entry| {
            matches!(entry.state, State::Downloaded)
                && entry.download.item.grandparent_rating_key.as_ref() == Some(show)
        };
        let entries = self.entries.borrow();
        let episodes = entries.iter().filter(of_show).map(|entry| self.as_left(&entry.download));
        let mut episodes = episodes.collect::<Result<Vec<Item>>>()?;
        episodes.sort_by_key(|episode| (episode.parent_index, episode.index));
        Ok(episodes)
    }

    /// Keeps where a viewing of the download with `key` stopped, for the
    /// server to hear of when it is next there.
    pub fn keep_position(&self, key: &RatingKey, position: Duration, duration: Duration) {
        let Some(entry) = self.find(key) else {
            return;
        };
        let unsent = Unsent::new(position, duration);
        if let Err(error) = self.store.set_unsent(&entry.download, Some(unsent)) {
            eprintln!("plex-rust: progress was not kept: {}", with_causes(&error));
        }
    }

    pub fn entries(&self) -> Vec<Entry> {
        self.entries.borrow().clone()
    }

    pub fn find(&self, key: &RatingKey) -> Option<Entry> {
        self.entries.borrow().iter().find(|entry| entry.download.key() == key).cloned()
    }

    /// The download of the item with `key`, if all of its file is here.
    pub fn whole(&self, key: &RatingKey) -> Option<Arc<Download>> {
        let entry = self.find(key)?;
        matches!(entry.state, State::Downloaded).then_some(entry.download)
    }

    /// The download that holds the file `item` plays, if all of it is
    /// here. A download of another version of the item does not.
    pub fn holding(&self, item: &Item) -> Option<Arc<Download>> {
        let plays = item.media.first().and_then(|media| media.parts.first())?;
        self.whole(&item.rating_key).filter(|download| download.part.id == plays.id)
    }

    /// Starts downloading `item`, the server's full description of a film
    /// or an episode that has no download yet, from `server`: the version
    /// of it whose media has the id `version`, or without one its first.
    pub fn add(
        self: &Rc<Self>,
        server: &MediaServer,
        item: &serde_json::Value,
        version: Option<u64>,
    ) -> Result<()> {
        let download = Arc::new(self.store.add(item, version)?);
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
/// what an earlier attempt left, and its poster and subtitle files with
/// it. `transfer` hears how far the file has got.
///
/// The subtitles come first, so that a download that is whole has them:
/// without the server there is nowhere else to get them from.
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

    for subtitles in &download.subtitles {
        let (file, partial) = (&subtitles.file, &subtitles.partial);
        if tokio::fs::try_exists(file).await.map_err(file_error(file))? {
            continue;
        }
        let text = server.sidecar(&subtitles.stream).await?;
        tokio::fs::write(partial, text).await.map_err(file_error(partial))?;
        tokio::fs::rename(partial, file).await.map_err(file_error(partial))?;
    }

    server.download(&download.part, &download.partial, transfer).await?;
    tokio::fs::rename(&download.partial, &download.file)
        .await
        .map_err(file_error(&download.partial))
}

/// Tells `server` where each viewing it could not hear of stopped, with
/// one report each, and forgets each one the server has taken.
pub async fn send_unsent(server: &MediaServer, store: &Store) -> Result<()> {
    for download in store.list()? {
        let Some(unsent) = store.unsent(&download)? else {
            continue;
        };
        let session = NSUUID::UUID().UUIDString().to_string();
        let progress = Progress {
            item: download.key(),
            session: &session,
            state: PlaybackState::Stopped,
            position: Duration::from_millis(unsent.position_ms),
            duration: Duration::from_millis(unsent.duration_ms),
            offline: true,
        };
        server.report(&progress).await?;
        store.set_unsent(&download, None)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use plex::{Client, Identity, Token};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::store::Unsent;
    use crate::store::tests::{episode, store};

    /// Answers every request on a local port with `status` and no body.
    /// Returns the server and the request lines as they arrive.
    async fn serve(status: &'static str) -> (MediaServer, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    head.push(socket.read_u8().await.unwrap());
                }
                let head = String::from_utf8(head).unwrap();
                log.lock().unwrap().push(head.lines().next().unwrap().to_owned());
                let answer = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n");
                socket.write_all(answer.as_bytes()).await.unwrap();
            }
        });
        let identity = Identity { client_identifier: "test".into(), device_name: "test".into() };
        let client = Client::new(identity).unwrap();
        (MediaServer::at(&client, address, "machine".to_owned(), Token::new("token")), seen)
    }

    #[tokio::test]
    async fn the_server_hears_once_where_a_viewing_without_it_stopped() {
        let store = store("sent");
        let download = store.add(&episode(), None).unwrap();
        let left = Unsent { position_ms: 900_000, duration_ms: 1_500_000 };
        store.set_unsent(&download, Some(left)).unwrap();

        // A server that is there but refuses is told again later.
        let (refusing, _) = serve("500 Internal Server Error").await;
        assert!(send_unsent(&refusing, &store).await.is_err());
        assert_eq!(store.unsent(&download).unwrap(), Some(left));

        let (server, seen) = serve("200 OK").await;
        send_unsent(&server, &store).await.unwrap();
        send_unsent(&server, &store).await.unwrap();
        assert_eq!(store.unsent(&download).unwrap(), None);
        let seen = seen.lock().unwrap();
        let [told] = seen.as_slice() else {
            panic!("the server was told {} times", seen.len());
        };
        assert!(told.starts_with("POST /:/timeline?ratingKey=5640&"), "{told}");
        for said in ["state=stopped", "time=900000", "duration=1500000", "offline=1"] {
            assert!(told.contains(said), "{said} is not in {told}");
        }
        std::fs::remove_dir_all(download.file.parent().unwrap()).unwrap();
    }
}
