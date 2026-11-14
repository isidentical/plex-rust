//! The downloads on disk.
//!
//! Each one is a few files side by side in `~/Movies/plex-rust`, named
//! after the item: the original file, which ends in `.part` until all of
//! it is there, a `.json` with the item as the server described it, a
//! `.jpg` poster, and for as long as the server has yet to hear where a
//! viewing stopped, a `.position` that says where. Nothing else records a
//! download, so the folder can be looked at, backed up and tidied by hand.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use plex::library::item_from;
use plex::model::{Item, Kind, Millis, Part, RatingKey};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The folder the downloads are in.
#[derive(Debug, Clone)]
pub struct Store {
    directory: PathBuf,
}

/// One item kept on disk, whole or in part.
#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    /// The item as the server described it when it was asked for.
    pub item: Item,
    /// The file on the server.
    pub part: Part,
    /// How long the whole file is, in bytes.
    pub size: u64,
    /// The original file, once all of it is there.
    pub file: PathBuf,
    /// The original file until then.
    pub partial: PathBuf,
    pub poster: PathBuf,
    record: PathBuf,
    position: PathBuf,
}

/// How much of a download is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDisk {
    Whole,
    /// This many bytes of it.
    Partial(u64),
}

/// Where a viewing stopped that the server could not be told about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unsent {
    pub position_ms: u64,
    pub duration_ms: u64,
}

impl Unsent {
    pub fn new(position: Duration, duration: Duration) -> Self {
        let millis = |time: Duration| u64::try_from(time.as_millis()).unwrap_or(u64::MAX);
        Self { position_ms: millis(position), duration_ms: millis(duration) }
    }
}

impl Store {
    /// The folder in the user's Movies, created if it is not there.
    pub fn open() -> Result<Self> {
        let home = std::env::var_os("HOME").ok_or(Error::NoHome)?;
        let directory = PathBuf::from(home).join("Movies/plex-rust");
        fs::create_dir_all(&directory)
            .map_err(|source| Error::File { path: directory.clone(), source })?;
        Ok(Self { directory })
    }

    /// Every download in the folder, by title.
    ///
    /// A `.json` that is not the record of a download is logged and left
    /// alone: one stray file must not hide the films next to it.
    pub fn list(&self) -> Result<Vec<Download>> {
        let folder_error = |source| Error::File { path: self.directory.clone(), source };
        let mut downloads = Vec::new();
        for entry in fs::read_dir(&self.directory).map_err(folder_error)? {
            let path = entry.map_err(folder_error)?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            match read(&path).and_then(|item| self.download(item)) {
                Ok(download) => downloads.push(download),
                Err(error) => eprintln!("plex-rust: {}", crate::error::with_causes(&error)),
            }
        }
        downloads.sort_by_cached_key(|download| name(&download.item));
        Ok(downloads)
    }

    /// Makes room for a download of `item`, the server's full description
    /// of a film or an episode, by writing its record. Its file follows.
    ///
    /// Of an item kept in several versions, the one whose media has the
    /// id `version` is downloaded, and without one the first. The record
    /// lists that version first, which is how every reader of it knows
    /// the version that is here.
    ///
    /// Fails if the item already has a download.
    pub fn add(&self, item: &serde_json::Value, version: Option<u64>) -> Result<Download> {
        let item = &with_first(item.clone(), version);
        let download = self.download(item.clone())?;
        let created = fs::OpenOptions::new().write(true).create_new(true).open(&download.record);
        created
            .and_then(|mut file| file.write_all(item.to_string().as_bytes()))
            .map_err(|source| Error::File { path: download.record.clone(), source })?;
        Ok(download)
    }

    /// Deletes every file of `download`.
    pub fn remove(&self, download: &Download) -> Result<()> {
        let Download { file, partial, poster, record, position, .. } = download;
        [file, partial, poster, record, position].into_iter().try_for_each(|path| remove(path))
    }

    /// Keeps where a viewing of `download` stopped until the server can
    /// be told, or with `None` forgets it once the server has been.
    pub fn set_unsent(&self, download: &Download, unsent: Option<Unsent>) -> Result<()> {
        let path = &download.position;
        let Some(unsent) = unsent else {
            return remove(path);
        };
        let json = serde_json::to_string(&unsent)
            .map_err(|source| Error::Record { path: path.clone(), source })?;
        fs::write(path, json).map_err(|source| Error::File { path: path.clone(), source })
    }

    /// Where a viewing of `download` stopped, if the server has yet to
    /// hear of it.
    pub fn unsent(&self, download: &Download) -> Result<Option<Unsent>> {
        let path = &download.position;
        let json = match fs::read_to_string(path) {
            Ok(json) => json,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(Error::File { path: path.clone(), source }),
        };
        let unsent = serde_json::from_str(&json);
        unsent.map(Some).map_err(|source| Error::Record { path: path.clone(), source })
    }

    /// The item of `download` as it was last left: where a viewing on
    /// this Mac stopped, if the server has yet to hear of that, and
    /// otherwise where the server said it was when it was downloaded.
    pub fn as_left(&self, download: &Download) -> Result<Item> {
        let mut item = download.item.clone();
        if let Some(unsent) = self.unsent(download)? {
            item.view_offset = Some(Millis(unsent.position_ms));
        }
        Ok(item)
    }

    fn download(&self, item: serde_json::Value) -> Result<Download> {
        let item = item_from(item)?;
        let parts = item.media.first().map_or(&[][..], |media| &media.parts);
        let part = match (parts, item.kind) {
            ([part], Kind::Movie | Kind::Episode) => part.clone(),
            // One file is what a download is, and what the player plays.
            ([_, _, ..], Kind::Movie | Kind::Episode) => {
                return Err(Error::InParts { parts: parts.len(), title: item.title });
            }
            _ => return Err(Error::NotPlayable(item.title)),
        };
        // A trailer, which the server only passes on, has no size.
        let Some(size) = part.size else {
            return Err(Error::NotPlayable(item.title));
        };

        let name = name(&item);
        let file = match Path::new(&part.key).extension().and_then(|extension| extension.to_str()) {
            Some(extension) => format!("{name}.{extension}"),
            None => name.clone(),
        };
        let at = |name: String| self.directory.join(name);
        Ok(Download {
            partial: at(format!("{file}.part")),
            file: at(file),
            poster: at(format!("{name}.jpg")),
            record: at(format!("{name}.json")),
            position: at(format!("{name}.position")),
            item,
            part,
            size,
        })
    }
}

impl Download {
    pub fn key(&self) -> &RatingKey {
        &self.item.rating_key
    }

    /// How much of the file is there. Before the first byte, none of it.
    pub fn on_disk(&self) -> Result<OnDisk> {
        if self.file.exists() {
            return Ok(OnDisk::Whole);
        }
        match fs::metadata(&self.partial) {
            Ok(partial) => Ok(OnDisk::Partial(partial.len())),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(OnDisk::Partial(0)),
            Err(source) => Err(Error::File { path: self.partial.clone(), source }),
        }
    }
}

/// What the files of `item` are called, without their endings: what it is,
/// for whoever looks into the folder, and its key on the server, which
/// keeps two films of one title apart.
fn name(item: &Item) -> String {
    let title = match (&item.grandparent_title, item.parent_index, item.index) {
        (Some(show), Some(season), Some(episode)) => {
            format!("{show} - S{season:02}E{episode:02} - {}", item.title)
        }
        _ => match item.year {
            Some(year) => format!("{} ({year})", item.title),
            None => item.title.clone(),
        },
    };
    // A slash would name a folder. The Finder shows a colon as a slash.
    let title: String = title
        .chars()
        .map(|letter| if matches!(letter, '/' | ':') || letter.is_control() { '-' } else { letter })
        .collect();
    // A leading dot would hide the file.
    format!("{} [{}]", title.trim_start_matches('.'), item.rating_key)
}

/// `item`, the server's description of a film or an episode, with the
/// version whose media has the id `version` put first. The others keep
/// their order behind it. An item that has no such version is left as it
/// is.
fn with_first(mut item: serde_json::Value, version: Option<u64>) -> serde_json::Value {
    let id = |media: &serde_json::Value| media.get("id").and_then(serde_json::Value::as_u64);
    let versions = item.get_mut("Media").and_then(serde_json::Value::as_array_mut);
    if let (Some(version), Some(versions)) = (version, versions) {
        versions.sort_by_key(|media| id(media) != Some(version));
    }
    item
}

fn read(path: &Path) -> Result<serde_json::Value> {
    let json =
        fs::read_to_string(path).map_err(|source| Error::File { path: path.to_owned(), source })?;
    serde_json::from_str(&json).map_err(|source| Error::Record { path: path.to_owned(), source })
}

/// Deletes the file at `path`, if there is one.
fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::File { path: path.to_owned(), source }),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// An empty folder that no other test uses.
    pub fn store(test: &str) -> Store {
        let name = format!("plex-rust-store-{}-{test}", std::process::id());
        let directory = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        Store { directory }
    }

    pub fn episode() -> serde_json::Value {
        serde_json::json!({
            "ratingKey": "5640", "type": "episode", "title": "Who/What: Why?",
            "grandparentTitle": "A Show", "parentIndex": 2, "index": 7,
            "duration": 1_500_000, "viewOffset": 60_000,
            "Media": [{ "id": 1, "Part": [
                { "id": 1, "key": "/library/parts/9/1700000000/file.mkv", "size": 8 },
            ]}],
        })
    }

    fn file_name(path: &Path) -> &str {
        path.file_name().unwrap().to_str().unwrap()
    }

    #[test]
    fn the_files_are_named_after_the_item() {
        let store = store("names");
        let download = store.add(&episode(), None).unwrap();
        let name = "A Show - S02E07 - Who-What- Why? [5640]";
        assert_eq!(file_name(&download.file), format!("{name}.mkv"));
        assert_eq!(file_name(&download.partial), format!("{name}.mkv.part"));
        assert_eq!(file_name(&download.poster), format!("{name}.jpg"));

        let film = serde_json::json!({
            "ratingKey": "370", "type": "movie", "title": "...And a Film", "year": 2015,
            "Media": [{ "id": 1, "Part": [{ "id": 1, "key": "/library/parts/1/2/file", "size": 8 }]}],
        });
        let download = store.add(&film, None).unwrap();
        assert_eq!(file_name(&download.file), "And a Film (2015) [370]");
        fs::remove_dir_all(&store.directory).unwrap();
    }

    #[test]
    fn only_a_film_or_an_episode_with_a_file_of_known_size_is_kept() {
        let store = store("kinds");
        let show = serde_json::json!({ "ratingKey": "1", "type": "show", "title": "A Show" });
        assert!(matches!(store.add(&show, None), Err(Error::NotPlayable(_))));
        let trailer = serde_json::json!({
            "ratingKey": "2", "type": "movie", "title": "A Film",
            "Media": [{ "id": 1, "Part": [{ "id": 1, "key": "/library/parts/1/2/file.mkv" }]}],
        });
        assert!(matches!(store.add(&trailer, None), Err(Error::NotPlayable(_))));
        assert_eq!(store.list().unwrap(), []);
        fs::remove_dir_all(&store.directory).unwrap();
    }

    #[test]
    fn the_version_picked_is_the_one_kept_and_listed_first() {
        let store = store("versions");
        let version = |id: u64, size: u64| {
            let key = format!("/library/parts/{id}/1/file.mkv");
            serde_json::json!({ "id": id, "Part": [{ "id": id, "key": key, "size": size }] })
        };
        let film = |key: &str| {
            serde_json::json!({
                "ratingKey": key, "type": "movie", "title": "A Film",
                "Media": [version(1, 10), version(2, 20), version(3, 30)],
            })
        };
        assert_eq!(store.add(&film("1"), None).unwrap().size, 10);
        assert_eq!(store.add(&film("2"), Some(3)).unwrap().size, 30);
        // A version the item no longer has leaves the first.
        assert_eq!(store.add(&film("3"), Some(9)).unwrap().size, 10);

        // Read back from the folder, each is still the version it was.
        let kept = store.list().unwrap();
        let sizes: Vec<u64> = kept.iter().map(|download| download.size).collect();
        assert_eq!(sizes, [10, 30, 10]);
        let versions: Vec<u64> = kept[1].item.media.iter().map(|media| media.id).collect();
        assert_eq!(versions, [3, 1, 2]);
        fs::remove_dir_all(&store.directory).unwrap();
    }

    #[test]
    fn an_item_split_across_files_is_refused() {
        let store = store("parts");
        let reel = |id: u64| serde_json::json!({ "id": id, "key": "/library/parts/1/2/reel.mkv", "size": 8 });
        let film = serde_json::json!({
            "ratingKey": "101", "type": "movie", "title": "Two Reels",
            "Media": [{ "id": 1, "Part": [reel(1), reel(2)] }],
        });
        assert!(matches!(store.add(&film, None), Err(Error::InParts { parts: 2, .. })));
        assert_eq!(store.list().unwrap(), []);
        fs::remove_dir_all(&store.directory).unwrap();
    }

    #[test]
    fn a_download_is_whole_once_its_file_has_lost_the_part_ending() {
        let store = store("states");
        let download = store.add(&episode(), None).unwrap();
        assert_eq!(download.on_disk().unwrap(), OnDisk::Partial(0));
        fs::write(&download.partial, b"half").unwrap();
        assert_eq!(download.on_disk().unwrap(), OnDisk::Partial(4));
        fs::rename(&download.partial, &download.file).unwrap();
        assert_eq!(download.on_disk().unwrap(), OnDisk::Whole);

        // The folder alone says what is in it.
        fs::write(store.directory.join("notes.json"), "{}").unwrap();
        assert_eq!(store.list().unwrap(), std::slice::from_ref(&download));
        // A second download of it would write over the record of the first.
        assert!(matches!(store.add(&episode(), None), Err(Error::File { .. })));

        // Removing it leaves what is not its own.
        store.set_unsent(&download, Some(Unsent { position_ms: 1, duration_ms: 2 })).unwrap();
        store.remove(&download).unwrap();
        let left: Vec<_> = fs::read_dir(&store.directory).unwrap().collect();
        assert_eq!(left.len(), 1);
        fs::remove_dir_all(&store.directory).unwrap();
    }

    #[test]
    fn a_download_resumes_where_it_was_left_here_until_the_server_knows() {
        let store = store("unsent");
        let download = store.add(&episode(), None).unwrap();
        assert_eq!(store.as_left(&download).unwrap().view_offset, Some(Millis(60_000)));

        let left = Unsent::new(Duration::from_secs(900), Duration::from_secs(1500));
        store.set_unsent(&download, Some(left)).unwrap();
        assert_eq!(store.unsent(&download).unwrap(), Some(left));
        assert_eq!(store.as_left(&download).unwrap().view_offset, Some(Millis(900_000)));

        store.set_unsent(&download, None).unwrap();
        assert_eq!(store.unsent(&download).unwrap(), None);
        assert_eq!(store.as_left(&download).unwrap().view_offset, Some(Millis(60_000)));
        fs::remove_dir_all(&store.directory).unwrap();
    }
}
