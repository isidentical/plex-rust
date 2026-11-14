//! Browsing the library.

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::model::{Hub, Item, Kind, RatingKey, Section};
use crate::server::MediaServer;

/// One page of a longer list.
#[derive(Debug, Clone)]
pub struct Page {
    pub items: Vec<Item>,
    /// How many items the whole list has.
    pub total: u32,
}

#[derive(Deserialize)]
struct Sections {
    #[serde(rename = "Directory", default)]
    sections: Vec<Section>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Items {
    #[serde(rename = "Metadata", default)]
    items: Vec<Item>,
    /// Missing when the server did not page the response.
    total_size: Option<u32>,
}

#[derive(Deserialize)]
struct Hubs {
    #[serde(rename = "Hub", default)]
    hubs: Vec<RawHub>,
}

/// A hub before its items are decoded. Rows of music, photos or people hold
/// objects that are not library items, so only rows of video are decoded.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHub {
    title: String,
    hub_identifier: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    more: bool,
    #[serde(rename = "Metadata", default)]
    items: Vec<serde_json::Value>,
}

impl MediaServer {
    /// The movie and TV libraries, in the server's order.
    pub async fn sections(&self) -> Result<Vec<Section>> {
        let Sections { sections } = self.get("/library/sections/all", &[]).await?;
        let shown = |section: &Section| matches!(section.kind, Kind::Movie | Kind::Show);
        Ok(sections.into_iter().filter(shown).collect())
    }

    /// The rows of the home screen, each with up to `count` items.
    pub async fn home(&self, count: u32) -> Result<Vec<Hub>> {
        let hubs: Hubs = self.get("/hubs", &[("count", &count.to_string())]).await?;
        video_hubs(hubs, "/hubs")
    }

    /// Results for `query`, one row per kind of item, up to `limit` each.
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<Hub>> {
        let query = [("query", query), ("limit", &limit.to_string())];
        let hubs: Hubs = self.get("/hubs/search", &query).await?;
        video_hubs(hubs, "/hubs/search")
    }

    /// `size` items of a library, starting at index `start`.
    pub async fn section_items(&self, section: &Section, start: u32, size: u32) -> Result<Page> {
        let path = format!("/library/sections/{}/all", section.key);
        let query = [
            ("X-Plex-Container-Start", start.to_string()),
            ("X-Plex-Container-Size", size.to_string()),
        ];
        let query: Vec<(&str, &str)> = query.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let Items { items, total_size } = self.get(&path, &query).await?;
        let total = total_size.unwrap_or_else(|| u32::try_from(items.len()).unwrap_or(u32::MAX));
        Ok(Page { items, total })
    }

    /// One item in full, including its markers and the tracks of its files.
    pub async fn item(&self, key: &RatingKey) -> Result<Item> {
        let path = format!("/library/metadata/{key}");
        let Items { items, .. } = self.get(&path, &[("includeMarkers", "1")]).await?;
        items
            .into_iter()
            .next()
            .ok_or_else(|| Error::Missing(key.clone()))
    }

    /// The seasons of a show, or the episodes of a season.
    pub async fn children(&self, key: &RatingKey) -> Result<Vec<Item>> {
        let path = format!("/library/metadata/{key}/children");
        let Items { items, .. } = self.get(&path, &[]).await?;
        Ok(items)
    }
}

/// Keeps the rows that hold movies, shows or episodes and decodes them.
fn video_hubs(hubs: Hubs, path: &'static str) -> Result<Vec<Hub>> {
    const VIDEO: [&str; 5] = ["movie", "show", "season", "episode", "mixed"];

    hubs.hubs
        .into_iter()
        .filter(|hub| VIDEO.contains(&hub.kind.as_str()) && !hub.items.is_empty())
        .map(|hub| {
            let items = serde_json::from_value(serde_json::Value::Array(hub.items))
                .map_err(|source| Error::Shape { path, source })?;
            Ok(Hub {
                title: hub.title,
                identifier: hub.hub_identifier,
                items,
                more: hub.more,
            })
        })
        .collect()
}

/// Decoding of responses recorded from a real server. Titles, summaries and
/// file names in the fixtures were replaced; the structure is untouched.
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde::de::DeserializeOwned;

    use super::*;
    use crate::model::{MarkerKind, StreamKind};
    use crate::server::Envelope;

    macro_rules! fixture {
        ($name:literal) => {
            decode(include_str!(concat!("../tests/fixtures/", $name, ".json")))
        };
        ($name:literal as $type:ty) => {
            decode::<$type>(include_str!(concat!("../tests/fixtures/", $name, ".json")))
        };
    }

    fn decode<T: DeserializeOwned>(json: &str) -> T {
        let envelope: Envelope<T> = serde_json::from_str(json).unwrap();
        envelope.media_container
    }

    #[test]
    fn sections_decode() {
        let Sections { sections } = fixture!("sections");
        let kinds: Vec<Kind> = sections.iter().map(|section| section.kind).collect();
        assert_eq!(kinds, [Kind::Movie, Kind::Show]);
    }

    #[test]
    fn a_page_of_movies_knows_the_library_size() {
        let page: Items = fixture!("movies");
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.total_size, Some(49));
        assert!(page.items.iter().all(|movie| movie.kind == Kind::Movie));
        assert!(
            page.items[0].media[0].parts[0]
                .key
                .starts_with("/library/parts/")
        );
    }

    #[test]
    fn shows_seasons_and_episodes_decode() {
        let shows: Items = fixture!("shows");
        let seasons: Items = fixture!("seasons");
        let episodes: Items = fixture!("episodes");
        assert!(
            shows
                .items
                .iter()
                .all(|show| show.kind == Kind::Show && show.media.is_empty())
        );
        assert!(
            seasons
                .items
                .iter()
                .all(|season| season.kind == Kind::Season)
        );
        assert!(
            episodes
                .items
                .iter()
                .all(|episode| episode.kind == Kind::Episode)
        );
    }

    #[test]
    fn a_full_episode_has_markers_and_tracks() {
        let episode = fixture!("episode" as Items).items.remove(0);

        assert_eq!(episode.markers[0].kind, MarkerKind::Intro);
        let kinds: Vec<StreamKind> = episode.media[0].parts[0]
            .streams
            .iter()
            .map(|stream| stream.kind)
            .collect();
        assert_eq!(
            kinds,
            [StreamKind::Video, StreamKind::Audio, StreamKind::Subtitle]
        );
    }

    #[test]
    fn marker_lookup_includes_the_start_and_excludes_the_end() {
        let episode = fixture!("episode" as Items).items.remove(0);
        let intro = &episode.markers[0];
        let (start, end): (Duration, Duration) =
            (intro.start_time_offset.into(), intro.end_time_offset.into());

        assert!(episode.marker_at(start).is_some());
        assert!(episode.marker_at(end - Duration::from_millis(1)).is_some());
        assert!(episode.marker_at(end).is_none());
    }

    #[test]
    fn home_keeps_only_rows_of_video() {
        let hubs = video_hubs(fixture!("home"), "/hubs").unwrap();
        let identifiers: Vec<&str> = hubs.iter().map(|hub| hub.identifier.as_str()).collect();
        assert_eq!(
            identifiers,
            [
                "home.continue",
                "home.movies.recent",
                "home.television.recent"
            ]
        );
    }

    #[test]
    fn search_drops_people_and_music() {
        let hubs = video_hubs(fixture!("search"), "/hubs/search").unwrap();
        let identifiers: Vec<&str> = hubs.iter().map(|hub| hub.identifier.as_str()).collect();
        assert_eq!(identifiers, ["show", "movie", "episode"]);
    }
}
