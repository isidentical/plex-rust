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

/// A day in the calendar, which the server writes as `2011-04-24`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Date {
    pub year: u16,
    /// From 1 for January.
    pub month: u8,
    pub day: u8,
}

impl TryFrom<String> for Date {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        let mut parts = text.splitn(3, '-');
        let year = parts.next().and_then(|part| part.parse::<u16>().ok());
        let mut small = || parts.next().and_then(|part| part.parse::<u8>().ok());
        match (year, small(), small()) {
            (Some(year), Some(month @ 1..=12), Some(day @ 1..=31)) => Ok(Self { year, month, day }),
            _ => Err(format!("{text:?} is not a date written as yyyy-mm-dd")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Movie,
    Show,
    Season,
    Episode,
    /// A trailer or other extra of a film.
    Clip,
    /// Music, photos and everything else plex-rust does not show.
    #[serde(other)]
    Other,
}

/// A library, such as "Movies".
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Section {
    /// The identifier used in `/library/sections/{key}`.
    pub key: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub title: String,
}

/// A titled row of items, such as "Continue Watching".
#[derive(Debug, Clone, PartialEq)]
pub struct Hub {
    pub title: String,
    /// A stable name for the row, such as `home.continue`.
    pub identifier: String,
    pub items: Vec<Item>,
    /// Server path of the whole list, when the server has more items for
    /// this row than it returned.
    pub more: Option<String>,
}

/// A movie, show, season or episode.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub rating_key: RatingKey,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub title: String,
    /// For an extra, what it is: `trailer`, `behindTheScenes` and so on.
    pub subtype: Option<String>,
    pub summary: Option<String>,
    /// The line from the poster, such as "In space no one can hear you".
    pub tagline: Option<String>,
    pub year: Option<u16>,
    /// The day it was first shown.
    pub originally_available_at: Option<Date>,
    /// The age rating, such as `PG-13` or `TV-14`.
    pub content_rating: Option<String>,
    /// What critics gave it, out of 10.
    pub rating: Option<f32>,
    /// Whose critics, as the name of a picture the server's own apps draw:
    /// `rottentomatoes://image.rating.ripe`, `imdb://image.rating`.
    pub rating_image: Option<String>,
    /// What viewers gave it, out of 10.
    pub audience_rating: Option<f32>,
    /// Whose viewers, named as for the critics.
    pub audience_rating_image: Option<String>,
    /// Who made it, such as "Lucasfilm Ltd.".
    pub studio: Option<String>,
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
    /// When it was last played, in seconds since 1970.
    pub last_viewed_at: Option<u64>,
    /// Episode number within the season, or season number within the show.
    pub index: Option<u32>,
    /// For an episode, its season number.
    pub parent_index: Option<u32>,
    /// For an episode, its season. For a season, its show.
    pub parent_rating_key: Option<RatingKey>,
    /// For an episode, its season's title. For a season, its show's.
    pub parent_title: Option<String>,
    /// For an episode, its show.
    pub grandparent_rating_key: Option<RatingKey>,
    /// For an episode, the show's title.
    pub grandparent_title: Option<String>,
    /// For an episode, the show's poster.
    pub grandparent_thumb: Option<String>,
    /// For an episode, the show's backdrop.
    pub grandparent_art: Option<String>,
    /// For a show, the number of seasons in it.
    pub child_count: Option<u32>,
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
    #[serde(rename = "Genre", default)]
    pub genres: Vec<Tag>,
    #[serde(rename = "Director", default)]
    pub directors: Vec<Tag>,
    /// The cast, most prominent first.
    #[serde(rename = "Role", default)]
    pub roles: Vec<Role>,
    /// Trailers and the like. Only present when the item was fetched with
    /// [`MediaServer::item`].
    ///
    /// [`MediaServer::item`]: crate::MediaServer::item
    #[serde(rename = "Extras", default)]
    pub extras: Extras,
    /// The library it is in, as in `/library/sections/{id}`. Only present
    /// when the item was fetched with [`MediaServer::item`].
    ///
    /// [`MediaServer::item`]: crate::MediaServer::item
    #[serde(rename = "librarySectionID")]
    pub library_section_id: Option<u32>,
}

impl Item {
    /// Whether it was watched to the end at least once.
    pub fn is_watched(&self) -> bool {
        match (self.leaf_count, self.viewed_leaf_count) {
            (Some(episodes), Some(watched)) => watched >= episodes,
            _ => self.view_count > 0,
        }
    }

    /// How much of it was watched, from 0 to 1. Zero when it was never
    /// started or was watched to the end.
    pub fn progress(&self) -> f64 {
        match (self.view_offset, self.duration) {
            (Some(offset), Some(duration)) if duration.0 > 0 => {
                (offset.0 as f64 / duration.0 as f64).clamp(0.0, 1.0)
            }
            _ => 0.0,
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
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub id: u64,
    /// `4k`, `1080`, `720` and so on.
    pub video_resolution: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    /// What sets the sound apart within its codec, such as `ma` for the
    /// lossless kind of DTS.
    pub audio_profile: Option<String>,
    /// 2 for stereo, 6 for 5.1 and so on.
    pub audio_channels: Option<u32>,
    /// Almost always exactly one. More when a film is split across files.
    #[serde(rename = "Part")]
    pub parts: Vec<Part>,
}

/// One file on the server.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Part {
    pub id: u64,
    /// Server path of the original file.
    pub key: String,
    /// Size in bytes. Absent for a trailer, which the server passes on
    /// from plex.tv and holds no file for.
    pub size: Option<u64>,
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
#[derive(Debug, Clone, PartialEq, Deserialize)]
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
    /// How the track is encoded: `hevc`, `eac3`, `srt` and so on.
    pub codec: Option<String>,
    /// What sets the track apart within its codec, such as `ma` for the
    /// lossless kind of DTS.
    pub profile: Option<String>,
    /// For sound, 2 for stereo, 6 for 5.1 and so on.
    pub channels: Option<u32>,
    /// For a picture, its transfer curve: `smpte2084` for HDR10,
    /// `arib-std-b67` for HLG, `bt709` or nothing for standard range.
    pub color_trc: Option<String>,
    /// Whether a picture carries Dolby Vision.
    #[serde(rename = "DOVIPresent", default)]
    pub dolby_vision: bool,
    /// The Dolby Vision profile. Profile 5 has no picture a player
    /// without Dolby Vision can show in the right colours.
    #[serde(rename = "DOVIProfile")]
    pub dolby_vision_profile: Option<u8>,
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
#[derive(Debug, Clone, PartialEq, Deserialize)]
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

/// A name attached to an item: a genre, a director.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Tag {
    /// The server's number for the name, which its filters take. Only
    /// present when the item was fetched in full.
    pub id: Option<u64>,
    pub tag: String,
}

/// A member of the cast.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Role {
    /// The server's number for the person, which its `actor` filter takes.
    /// Only present when the item was fetched in full.
    pub id: Option<u64>,
    /// The person's name.
    pub tag: String,
    /// Who they play.
    pub role: Option<String>,
    /// Address of a photo. Unlike other artwork this is a whole URL, on
    /// plex.tv, which the server's image endpoint fetches and scales too.
    pub thumb: Option<String>,
}

/// The extras of an item, wrapped the way the server sends them.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Extras {
    #[serde(rename = "Metadata", default)]
    pub items: Vec<Item>,
}

/// Four colours taken from an item's artwork, one per corner. Plex's own
/// apps paint page backgrounds with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Colors {
    pub top_left: Rgb,
    pub top_right: Rgb,
    pub bottom_left: Rgb,
    pub bottom_right: Rgb,
}

/// A colour, which the server writes as `rrggbb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl TryFrom<String> for Rgb {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        let digits = text.strip_prefix('#').unwrap_or(&text);
        let value = u32::from_str_radix(digits, 16).ok().filter(|_| digits.len() == 6);
        let value = value.ok_or_else(|| format!("{text:?} is not a colour written as rrggbb"))?;
        let [_, red, green, blue] = value.to_be_bytes();
        Ok(Self { red, green, blue })
    }
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

    #[test]
    fn a_colour_is_six_hex_digits_and_nothing_else() {
        let rgb = |text: &str| Rgb::try_from(text.to_owned());
        assert_eq!(rgb("8d2d05"), Ok(Rgb { red: 0x8d, green: 0x2d, blue: 0x05 }));
        assert_eq!(rgb("#8d2d05"), Ok(Rgb { red: 0x8d, green: 0x2d, blue: 0x05 }));
        assert!(rgb("8d2d").is_err());
        assert!(rgb("red").is_err());
    }

    #[test]
    fn a_date_is_a_year_a_month_and_a_day_that_exist() {
        let date = |text: &str| Date::try_from(text.to_owned());
        assert_eq!(date("2011-04-24"), Ok(Date { year: 2011, month: 4, day: 24 }));
        assert!(date("2011-13-01").is_err());
        assert!(date("2011-04-00").is_err());
        assert!(date("2011-04").is_err());
        assert!(date("yesterday").is_err());
    }
}
