//! What the server says about the library, as typed values.
//!
//! Field names follow the server's JSON. A field is an `Option` only when
//! real responses were seen without it.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;

/// The server's identifier for one library item.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(transparent)]
pub struct RatingKey(String);

impl RatingKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RatingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A length of time in milliseconds, the unit the server uses throughout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(transparent)]
pub struct Millis(pub u64);

impl From<Millis> for Duration {
    fn from(millis: Millis) -> Self {
        Self::from_millis(millis.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Movie,
    Show,
    Season,
    Episode,
    /// Music, photos and everything else plex-rust does not show.
    #[serde(other)]
    Other,
}

/// A library, such as "Movies".
#[derive(Debug, Clone, Deserialize)]
pub struct Section {
    /// The identifier used in `/library/sections/{key}`.
    pub key: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub title: String,
}

/// A titled row of items, such as "Continue Watching".
#[derive(Debug, Clone)]
pub struct Hub {
    pub title: String,
    /// A stable name for the row, such as `home.continue`.
    pub identifier: String,
    pub items: Vec<Item>,
    /// Whether the server has more items for this row than it returned.
    pub more: bool,
}

/// A movie, show, season or episode.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub rating_key: RatingKey,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub title: String,
    pub summary: Option<String>,
    pub year: Option<u16>,
    /// Server path of the poster.
    pub thumb: Option<String>,
    /// Server path of the backdrop.
    pub art: Option<String>,
    pub duration: Option<Millis>,
    /// Where playback stopped. Absent when unwatched or finished.
    pub view_offset: Option<Millis>,
    /// How many times it was watched to the end.
    #[serde(default)]
    pub view_count: u32,
    /// Episode number within the season, or season number within the show.
    pub index: Option<u32>,
    /// For an episode, its season number.
    pub parent_index: Option<u32>,
    /// For an episode, its season. For a season, its show.
    pub parent_rating_key: Option<RatingKey>,
    /// For an episode, the show's title.
    pub grandparent_title: Option<String>,
    /// For an episode, the show's poster.
    pub grandparent_thumb: Option<String>,
    /// For a show or season, the number of episodes in it.
    pub leaf_count: Option<u32>,
    /// For a show or season, the number of watched episodes in it.
    pub viewed_leaf_count: Option<u32>,
    /// The playable versions. Empty for shows and seasons.
    #[serde(rename = "Media", default)]
    pub media: Vec<Media>,
    /// Only present when the item was fetched with [`MediaServer::item`].
    ///
    /// [`MediaServer::item`]: crate::MediaServer::item
    #[serde(rename = "Marker", default)]
    pub markers: Vec<Marker>,
    #[serde(rename = "UltraBlurColors")]
    pub colors: Option<Colors>,
}

impl Item {
    /// Whether it was watched to the end at least once.
    pub fn is_watched(&self) -> bool {
        match (self.leaf_count, self.viewed_leaf_count) {
            (Some(episodes), Some(watched)) => watched >= episodes,
            _ => self.view_count > 0,
        }
    }

    /// The marker that covers `position`, if any.
    pub fn marker_at(&self, position: Duration) -> Option<&Marker> {
        self.markers.iter().find(|marker| {
            (marker.start_time_offset.into()..marker.end_time_offset.into()).contains(&position)
        })
    }
}

/// One playable version of an item, such as its 4K encode.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub id: u64,
    /// `4k`, `1080`, `720` and so on.
    pub video_resolution: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    /// Almost always exactly one. More when a film is split across files.
    #[serde(rename = "Part")]
    pub parts: Vec<Part>,
}

/// One file on the server.
#[derive(Debug, Clone, Deserialize)]
pub struct Part {
    pub id: u64,
    /// Server path of the original file.
    pub key: String,
    /// Size in bytes.
    pub size: u64,
    /// Only present when the item was fetched with [`MediaServer::item`].
    ///
    /// [`MediaServer::item`]: crate::MediaServer::item
    #[serde(rename = "Stream", default)]
    pub streams: Vec<Stream>,
}

impl Part {
    /// The tracks of `kind` that are inside the file itself, in file order.
    /// A player numbers its tracks the same way, from 1.
    pub fn embedded(&self, kind: StreamKind) -> impl Iterator<Item = &Stream> {
        self.streams.iter().filter(move |stream| stream.kind == kind && stream.key.is_none())
    }

    /// The position, from 1, of the track of `kind` that the viewer last
    /// chose, among the embedded tracks of that kind.
    pub fn chosen(&self, kind: StreamKind) -> Option<i64> {
        let position = self.embedded(kind).position(|stream| stream.selected)?;
        i64::try_from(position + 1).ok()
    }
}

/// A video, audio or subtitle track inside a file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    pub id: u64,
    #[serde(rename = "streamType")]
    pub kind: StreamKind,
    /// A description for menus, such as "English (EAC3 5.1)".
    pub display_title: Option<String>,
    /// Whether this is the track the user last chose for this file.
    #[serde(default)]
    pub selected: bool,
    /// Set for subtitles that live in their own file next to the video.
    pub key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(from = "u8")]
pub enum StreamKind {
    Video,
    Audio,
    Subtitle,
    /// Lyrics and anything added later.
    Other,
}

impl From<u8> for StreamKind {
    fn from(kind: u8) -> Self {
        match kind {
            1 => Self::Video,
            2 => Self::Audio,
            3 => Self::Subtitle,
            _ => Self::Other,
        }
    }
}

/// A stretch of an episode the viewer may want to skip.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    #[serde(rename = "type")]
    pub kind: MarkerKind,
    pub start_time_offset: Millis,
    pub end_time_offset: Millis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkerKind {
    Intro,
    /// Plex's API description spells this both ways.
    #[serde(alias = "credit")]
    Credits,
    #[serde(other)]
    Other,
}

/// Four colours taken from an item's artwork, one per corner, as `#rrggbb`.
/// Plex's own apps paint page backgrounds with them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Colors {
    pub top_left: String,
    pub top_right: String,
    pub bottom_left: String,
    pub bottom_right: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chosen_track_is_counted_among_embedded_tracks_of_its_kind() {
        let part: Part = serde_json::from_value(serde_json::json!({
            "id": 1, "key": "/library/parts/1/0/file.mkv", "size": 1,
            "Stream": [
                { "id": 10, "streamType": 1 },
                { "id": 11, "streamType": 2, "selected": true },
                { "id": 12, "streamType": 3, "key": "/library/streams/12" },
                { "id": 13, "streamType": 3 },
                { "id": 14, "streamType": 3, "selected": true },
            ],
        }))
        .unwrap();

        assert_eq!(part.chosen(StreamKind::Audio), Some(1));
        // The first subtitle stream is a separate file, so it is not one of
        // the file's own tracks and does not count.
        assert_eq!(part.chosen(StreamKind::Subtitle), Some(2));
        assert_eq!(part.chosen(StreamKind::Video), None);
    }
}
