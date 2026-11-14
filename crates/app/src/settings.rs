//! What the app remembers between launches.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use plex::library::{Listing, Sort};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The viewer's standing choices. Each field falls back to its default when
/// the file is missing it, so a new setting never invalidates an old file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Playback speed in hundredths of normal: 125 is one and a quarter.
    /// Not called `speed`, which older files hold in tenths and which is
    /// now ignored.
    pub speed_percent: u32,
    /// Volume out of 100.
    pub volume: f64,
    /// Whether the picture is drawn the cheap way: see
    /// [`player::Player::set_low_power`]. Only the viewer turns this on.
    /// The system's own Low Power Mode is never looked at.
    pub low_power: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { speed_percent: 100, volume: 100.0, low_power: false }
    }
}

impl Settings {
    /// Reads the settings, or the defaults if none were ever saved.
    pub fn load() -> Result<Self> {
        Ok(read("settings.json")?.unwrap_or_default())
    }

    pub fn save(&self) -> Result<()> {
        write("settings.json", self)
    }
}

/// How the viewer last listed each library: in which order, and whether
/// only what is left to watch. Keyed by the library's key on the server.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Listings(BTreeMap<String, SavedListing>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SavedListing {
    /// The order by its title, such as "Date Added".
    sort: String,
    unwatched_only: bool,
}

impl Listings {
    /// Reads the listings, or none if none were ever saved.
    pub fn load() -> Result<Self> {
        Ok(read("listings.json")?.unwrap_or_default())
    }

    pub fn save(&self) -> Result<()> {
        write("listings.json", self)
    }

    /// How the library with `key` was last listed. A library never listed
    /// any other way, or in an order this version no longer has, is listed
    /// the default way.
    pub fn of(&self, key: &str) -> Listing {
        let Some(saved) = self.0.get(key) else {
            return Listing::default();
        };
        let sort = Sort::ALL.into_iter().find(|sort| sort.title() == saved.sort);
        Listing { sort: sort.unwrap_or_default(), unwatched_only: saved.unwatched_only }
    }

    pub fn set(&mut self, key: &str, listing: Listing) {
        let saved = SavedListing {
            sort: listing.sort.title().to_owned(),
            unwatched_only: listing.unwatched_only,
        };
        self.0.insert(key.to_owned(), saved);
    }
}

/// Where the viewer was when the app was last used, which is where the
/// next launch starts. A search is not a place to come back to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Place {
    Home,
    /// The library with this key on the server.
    Library {
        key: String,
    },
    /// The whole list at this server path, under this title.
    List {
        title: String,
        path: String,
    },
    /// The page of the item with this rating key.
    Item {
        key: String,
    },
    /// What is kept on this Mac.
    Downloads,
}

impl Place {
    /// Reads the place, or none if none was ever saved.
    pub fn load() -> Result<Option<Self>> {
        read("place.json")
    }

    pub fn save(&self) -> Result<()> {
        write("place.json", self)
    }
}

fn path(name: &str) -> Result<PathBuf> {
    Ok(crate::account::support_directory()?.join(name))
}

/// Reads the file called `name` in the app's folder. `None` if there is
/// no such file.
fn read<T: for<'de> Deserialize<'de>>(name: &str) -> Result<Option<T>> {
    let path = path(name)?;
    match fs::read_to_string(&path) {
        Ok(json) => {
            let value = serde_json::from_str(&json);
            value.map(Some).map_err(|source| Error::Settings { path, source })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::File { path, source }),
    }
}

fn write(name: &str, value: &impl Serialize) -> Result<()> {
    let path = path(name)?;
    let json = serde_json::to_string_pretty(value)
        .map_err(|source| Error::Settings { path: path.clone(), source })?;
    fs::write(&path, json).map_err(|source| Error::File { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_library_is_listed_the_way_it_was_left() {
        let mut listings = Listings::default();
        let left = Listing { sort: Sort::DateAdded, unwatched_only: true };
        listings.set("2", left);

        let json = serde_json::to_string(&listings).unwrap();
        let read: Listings = serde_json::from_str(&json).unwrap();
        assert_eq!(read.of("2"), left);
        assert_eq!(read.of("1"), Listing::default());
    }

    #[test]
    fn a_place_is_read_back_as_it_was_saved() {
        let places = [
            Place::Home,
            Place::Library { key: "2".to_owned() },
            Place::List { title: "Recently Added".to_owned(), path: "/hubs/recent".to_owned() },
            Place::Item { key: "370".to_owned() },
            Place::Downloads,
        ];
        for place in places {
            let json = serde_json::to_string(&place).unwrap();
            assert_eq!(serde_json::from_str::<Place>(&json).unwrap(), place);
        }
        // A kind of place a later version saved is not one this one knows.
        assert!(serde_json::from_str::<Place>(r#"{ "kind": "music" }"#).is_err());
    }

    #[test]
    fn an_order_that_no_longer_exists_falls_back_to_the_default() {
        let json = r#"{ "1": { "sort": "Popularity", "unwatched_only": true } }"#;
        let read: Listings = serde_json::from_str(json).unwrap();
        assert_eq!(read.of("1"), Listing { sort: Sort::Title, unwatched_only: true });
    }
}
