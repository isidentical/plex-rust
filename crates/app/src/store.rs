//! The downloads on disk.
//!
//! Each one is up to three files side by side in `~/Movies/plex-rust`,
//! named after the item: the original file, which ends in `.part` until
//! all of it is there, a `.json` with what the server said about the item,
//! and a `.jpg` poster. Nothing else records a download, so the folder can
//! be looked at, backed up and tidied by hand.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use plex::library::item_from;
use plex::model::{Item, Kind, Part, RatingKey};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The folder the downloads are in.
pub struct Store {
    directory: PathBuf,
}

/// What the `.json` next to a download holds.
#[derive(Serialize, Deserialize)]
struct Record {
    /// The item in full, as the server described it when it was asked for.
    item: serde_json::Value,
}

/// One item kept on disk, whole or in part.
#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    /// The item as the server described it.
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
}

/// How much of a download is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDisk {
    Whole,
    /// This many bytes of it.
    Partial(u64),
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
            match read(&path).and_then(|record| self.download(&record)) {
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
    /// Fails if the item already has one, which may hold what this one
    /// would not.
    pub fn add(&self, item: &serde_json::Value) -> Result<Download> {
        let record = Record { item: item.clone() };
        let download = self.download(&record)?;
        let json = json(&download.record, &record)?;
        let created = fs::OpenOptions::new().write(true).create_new(true).open(&download.record);
        created
            .and_then(|mut file| file.write_all(json.as_bytes()))
            .map_err(|source| Error::File { path: download.record.clone(), source })?;
        Ok(download)
    }

    fn download(&self, record: &Record) -> Result<Download> {
        let item = item_from(record.item.clone())?;
        let part = item.media.first().and_then(|media| media.parts.first()).cloned();
        let (Some(part), Kind::Movie | Kind::Episode) = (part, item.kind) else {
            return Err(Error::NotPlayable(item.title));
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

fn read(path: &Path) -> Result<Record> {
    let json =
        fs::read_to_string(path).map_err(|source| Error::File { path: path.to_owned(), source })?;
    serde_json::from_str(&json).map_err(|source| Error::Record { path: path.to_owned(), source })
}

fn json(path: &Path, record: &Record) -> Result<String> {
    serde_json::to_string(record).map_err(|source| Error::Record { path: path.to_owned(), source })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty folder that no other test uses.
    fn store(test: &str) -> Store {
        let name = format!("plex-rust-store-{}-{test}", std::process::id());
        let directory = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        Store { directory }
    }

    fn episode() -> serde_json::Value {
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
        let download = store.add(&episode()).unwrap();
        assert_eq!(file_name(&download.file), "A Show - S02E07 - Who-What- Why? [5640].mkv");
        assert_eq!(
            file_name(&download.partial),
            "A Show - S02E07 - Who-What- Why? [5640].mkv.part"
        );
        assert_eq!(file_name(&download.poster), "A Show - S02E07 - Who-What- Why? [5640].jpg");

        let film = serde_json::json!({
            "ratingKey": "370", "type": "movie", "title": "...And a Film", "year": 2015,
            "Media": [{ "id": 1, "Part": [{ "id": 1, "key": "/library/parts/1/2/file", "size": 8 }]}],
        });
        let download = store.add(&film).unwrap();
        assert_eq!(file_name(&download.file), "And a Film (2015) [370]");
        fs::remove_dir_all(&store.directory).unwrap();
    }

    #[test]
    fn only_a_film_or_an_episode_with_a_file_of_known_size_is_kept() {
        let store = store("kinds");
        let show = serde_json::json!({ "ratingKey": "1", "type": "show", "title": "A Show" });
        assert!(matches!(store.add(&show), Err(Error::NotPlayable(_))));
        let trailer = serde_json::json!({
            "ratingKey": "2", "type": "movie", "title": "A Film",
            "Media": [{ "id": 1, "Part": [{ "id": 1, "key": "/library/parts/1/2/file.mkv" }]}],
        });
        assert!(matches!(store.add(&trailer), Err(Error::NotPlayable(_))));
        assert_eq!(store.list().unwrap(), []);
        fs::remove_dir_all(&store.directory).unwrap();
    }

    #[test]
    fn a_download_is_whole_once_its_file_has_lost_the_part_ending() {
        let store = store("states");
        let download = store.add(&episode()).unwrap();
        assert_eq!(download.on_disk().unwrap(), OnDisk::Partial(0));
        fs::write(&download.partial, b"half").unwrap();
        assert_eq!(download.on_disk().unwrap(), OnDisk::Partial(4));
        fs::rename(&download.partial, &download.file).unwrap();
        assert_eq!(download.on_disk().unwrap(), OnDisk::Whole);

        // The folder alone says what is in it.
        fs::write(store.directory.join("notes.json"), "{}").unwrap();
        assert_eq!(store.list().unwrap(), std::slice::from_ref(&download));
        // A second download of it would write over the record of the first.
        assert!(matches!(store.add(&episode()), Err(Error::File { .. })));
        fs::remove_dir_all(&store.directory).unwrap();
    }
}
