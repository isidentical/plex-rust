//! What was watched, when and on what: the server's play history and the
//! statistics it keeps of time watched and data streamed. Plex Web's
//! Dashboard is drawn from the same answers.

use serde::{Deserialize, Deserializer};

use crate::error::Result;
use crate::model::{Kind, RatingKey};
use crate::server::MediaServer;

/// How many plays of the history are asked for at a time.
const HISTORY_PAGE: u32 = 1000;

/// One play the server counted: an item watched far enough to be marked
/// as watched.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Play {
    /// Missing once the item is gone from the library.
    pub rating_key: Option<RatingKey>,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub title: String,
    /// For an episode, its show.
    pub grandparent_title: Option<String>,
    #[serde(rename = "grandparentKey", default, deserialize_with = "key_of_path")]
    pub grandparent_rating_key: Option<RatingKey>,
    pub thumb: Option<String>,
    pub grandparent_thumb: Option<String>,
    /// For an episode, its season and its number in it.
    pub parent_index: Option<u32>,
    pub index: Option<u32>,
    /// Seconds since 1970.
    pub viewed_at: u64,
    #[serde(rename = "accountID")]
    pub account: u32,
    #[serde(rename = "deviceID")]
    pub device: u32,
}

/// The rating key at the end of a path such as `/library/metadata/4134`.
fn key_of_path<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<RatingKey>, D::Error> {
    let path: Option<String> = Option::deserialize(deserializer)?;
    Ok(path.and_then(|path| path.rsplit('/').next().map(RatingKey::new)))
}

/// How long each row of the statistics covers. The values are the
/// server's own numbers for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    Month = 1,
    Hour = 4,
}

/// Time spent watching in one span, on one device, by one account, of
/// one kind of item.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Watched {
    /// When the span starts, in seconds since 1970.
    pub at: u64,
    #[serde(rename = "accountID")]
    pub account: u32,
    #[serde(rename = "deviceID")]
    pub device: u32,
    #[serde(rename = "metadataType", deserialize_with = "kind_of_number")]
    pub kind: Kind,
    /// Plays that ended in the span.
    #[serde(rename = "count")]
    pub plays: u32,
    #[serde(rename = "duration")]
    pub seconds: u64,
}

/// The kind of item the server numbers `number`, from the numbers its
/// `type` filter takes.
fn kind_of_number<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Kind, D::Error> {
    Ok(match u32::deserialize(deserializer)? {
        1 => Kind::Movie,
        2 => Kind::Show,
        3 => Kind::Season,
        4 => Kind::Episode,
        12 => Kind::Clip,
        _ => Kind::Other,
    })
}

/// Data sent in one span to one device of one account.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Streamed {
    pub at: u64,
    #[serde(rename = "accountID")]
    pub account: u32,
    #[serde(rename = "deviceID")]
    pub device: u32,
    /// Whether the device was on the server's own network.
    pub lan: bool,
    pub bytes: u64,
}

/// A device that has played from the server.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Device {
    pub id: u32,
    /// Empty for some, such as the server's own work.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub platform: String,
}

/// An account that has played from the server.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Account {
    pub id: u32,
    pub name: String,
}

/// Rows of statistics, with the accounts and devices they name.
#[derive(Debug, Clone, PartialEq)]
pub struct Statistics<T> {
    pub rows: Vec<T>,
    pub accounts: Vec<Account>,
    pub devices: Vec<Device>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct History {
    #[serde(rename = "Metadata", default)]
    plays: Vec<Play>,
    total_size: Option<u32>,
}

#[derive(Deserialize)]
struct RawStatistics<T> {
    #[serde(rename = "StatisticsMedia", alias = "StatisticsBandwidth", default = "Vec::new")]
    rows: Vec<T>,
    #[serde(rename = "Account", default)]
    accounts: Vec<Account>,
    #[serde(rename = "Device", default)]
    devices: Vec<Device>,
}

impl<T> From<RawStatistics<T>> for Statistics<T> {
    fn from(raw: RawStatistics<T>) -> Self {
        Self { rows: raw.rows, accounts: raw.accounts, devices: raw.devices }
    }
}

impl MediaServer {
    /// Every play the server has kept, newest first.
    pub async fn history(&self) -> Result<Vec<Play>> {
        let mut plays = Vec::new();
        loop {
            let start = u32::try_from(plays.len()).unwrap_or(u32::MAX).to_string();
            let size = HISTORY_PAGE.to_string();
            let query = [
                ("sort", "viewedAt:desc"),
                ("X-Plex-Container-Start", start.as_str()),
                ("X-Plex-Container-Size", size.as_str()),
            ];
            let page: History = self.get("/status/sessions/history/all", &query).await?;
            let (came, total) = (page.plays.len(), page.total_size);
            plays.extend(page.plays);
            let all = total.is_none_or(|total| plays.len() >= total as usize);
            if came == 0 || all {
                return Ok(plays);
            }
        }
    }

    /// Time watched, in rows that each cover a `span`.
    pub async fn watch_time(&self, span: Span) -> Result<Statistics<Watched>> {
        self.statistics("/statistics/media", span).await
    }

    /// Data streamed, in rows that each cover a `span`.
    pub async fn streamed(&self, span: Span) -> Result<Statistics<Streamed>> {
        self.statistics("/statistics/bandwidth", span).await
    }

    async fn statistics<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        span: Span,
    ) -> Result<Statistics<T>> {
        let span = (span as u32).to_string();
        let raw: RawStatistics<T> = self.get(path, &[("timespan", &span)]).await?;
        Ok(raw.into())
    }
}

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;

    use super::*;
    use crate::server::Envelope;

    fn fixture<T: DeserializeOwned>(json: &str) -> T {
        let envelope: Envelope<T> = serde_json::from_str(json).unwrap();
        envelope.media_container
    }

    #[test]
    fn a_play_of_an_episode_names_its_show_by_rating_key() {
        let history: History = fixture(include_str!("../tests/fixtures/history.json"));
        let episode = &history.plays[0];
        assert_eq!(episode.kind, Kind::Episode);
        assert_eq!(episode.grandparent_rating_key, Some(RatingKey::new("4134")));
        assert_eq!((episode.parent_index, episode.index), (Some(3), Some(6)));

        let film = history.plays.last().unwrap();
        assert_eq!(film.kind, Kind::Movie);
        assert_eq!(film.grandparent_rating_key, None);
        assert_eq!(history.total_size, Some(202));
    }

    #[test]
    fn watch_time_rows_say_what_kind_of_item_was_watched() {
        let statistics: Statistics<Watched> =
            fixture::<RawStatistics<Watched>>(include_str!("../tests/fixtures/watch_time.json"))
                .into();
        let kinds: Vec<Kind> = statistics.rows.iter().map(|row| row.kind).collect();
        assert_eq!(kinds, [Kind::Episode, Kind::Movie, Kind::Episode]);
        assert_eq!((statistics.rows[2].plays, statistics.rows[2].seconds), (1, 541));
        assert_eq!(statistics.devices[1].name, "Mac");
    }

    #[test]
    fn bandwidth_rows_say_whether_the_device_was_nearby() {
        let statistics: Statistics<Streamed> =
            fixture::<RawStatistics<Streamed>>(include_str!("../tests/fixtures/streamed.json"))
                .into();
        let near: Vec<bool> = statistics.rows.iter().map(|row| row.lan).collect();
        assert_eq!(near, [true, false]);
        assert_eq!(statistics.devices[0].name, "");
    }
}
