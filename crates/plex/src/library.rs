//! Browsing the library.

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::model::{Colors, Hub, Item, Kind, RatingKey, Role, Section, Tag};
use crate::server::MediaServer;

/// One page of a longer list.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub items: Vec<Item>,
    /// How many items the whole list has.
    pub total: u32,
}

/// The order a library is listed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Title,
    /// Newest in the library first.
    DateAdded,
    /// Newest release first.
    ReleaseDate,
    /// Best liked by viewers first.
    Rating,
    /// Most recently watched first.
    LastWatched,
}

impl Sort {
    pub const ALL: [Self; 5] =
        [Self::Title, Self::DateAdded, Self::ReleaseDate, Self::Rating, Self::LastWatched];

    pub fn title(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::DateAdded => "Date Added",
            Self::ReleaseDate => "Release Date",
            Self::Rating => "Rating",
            Self::LastWatched => "Last Watched",
        }
    }

    /// The value of the server's `sort` parameter, from the names it lists
    /// at `/library/sections/{id}/sorts`.
    fn parameter(self) -> &'static str {
        match self {
            Self::Title => "titleSort",
            Self::DateAdded => "addedAt:desc",
            Self::ReleaseDate => "originallyAvailableAt:desc",
            Self::Rating => "audienceRating:desc",
            Self::LastWatched => "lastViewedAt:desc",
        }
    }
}

/// Which items of a library to list, and in what order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Listing {
    pub sort: Sort,
    /// Leave out what was watched to the end. For shows, those with no
    /// episode left to watch.
    pub unwatched_only: bool,
}

impl Listing {
    fn query(self, kind: Kind) -> Vec<(&'static str, &'static str)> {
        let mut query = vec![("sort", self.sort.parameter())];
        if self.unwatched_only {
            // A show is unwatched while any of its episodes is.
            let filter = if kind == Kind::Show { "unwatchedLeaves" } else { "unwatched" };
            query.push((filter, "1"));
        }
        query
    }
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

/// Items left as the server wrote them.
#[derive(Deserialize)]
struct RawItems {
    #[serde(rename = "Metadata", default)]
    items: Vec<serde_json::Value>,
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
    /// The path of the whole list.
    key: Option<String>,
    #[serde(rename = "Metadata", default)]
    items: Vec<serde_json::Value>,
}

/// A character that titles of a library start with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstCharacter {
    /// The character as the server names it: a letter, or `#` for all
    /// that is not one.
    pub title: String,
    /// The index of the first item under it, in the library as listed.
    pub start: u32,
}

#[derive(Deserialize)]
struct FirstCharacters {
    #[serde(rename = "Directory", default)]
    characters: Vec<RawFirstCharacter>,
}

/// A character as the server lists it: with how many items it starts.
#[derive(Deserialize)]
struct RawFirstCharacter {
    title: String,
    size: u32,
}

impl FirstCharacters {
    /// Each character with where its items begin, which is after all
    /// the items of the characters before it.
    fn placed(self) -> Vec<FirstCharacter> {
        let mut start = 0;
        let place = |character: RawFirstCharacter| {
            let placed = FirstCharacter { title: character.title, start };
            start += character.size;
            placed
        };
        self.characters.into_iter().map(place).collect()
    }
}

/// The colours of one picture.
#[derive(Deserialize)]
struct Tones {
    #[serde(rename = "UltraBlurColors")]
    colors: [Colors; 1],
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
        // Not kept: each pause in typing asks for what was typed so far,
        // and all of it would stay on disk for good.
        let hubs: Hubs = self.forgetful().get("/hubs/search", &query).await?;
        video_hubs(hubs, "/hubs/search")
    }

    /// `size` items of a library as `listing` selects and orders them,
    /// starting at index `start`.
    pub async fn section_items(
        &self,
        section: &Section,
        listing: Listing,
        start: u32,
        size: u32,
    ) -> Result<Page> {
        let path = format!("/library/sections/{}/all", section.key);
        self.page(&path, &listing.query(section.kind), start, size).await
    }

    /// The first characters of the titles in a library that `listing`
    /// selects, in the order [`Sort::Title`] lists them, each with where
    /// its items begin in [`section_items`](Self::section_items). They
    /// make a long library something to jump about in.
    pub async fn first_characters(
        &self,
        section: &Section,
        listing: Listing,
    ) -> Result<Vec<FirstCharacter>> {
        let path = format!("/library/sections/{}/firstCharacter", section.key);
        let characters: FirstCharacters = self.get(&path, &listing.query(section.kind)).await?;
        Ok(characters.placed())
    }

    /// `size` items of the list at server path `path`, starting at index
    /// `start`. For the paths the server hands out itself: the whole of a
    /// row ([`Hub::more`]) or what a filter selects.
    pub async fn list(&self, path: &str, start: u32, size: u32) -> Result<Page> {
        self.page(path, &[], start, size).await
    }

    async fn page(
        &self,
        path: &str,
        query: &[(&str, &str)],
        start: u32,
        size: u32,
    ) -> Result<Page> {
        let (start, size) = (start.to_string(), size.to_string());
        let mut query = query.to_vec();
        query.push(("X-Plex-Container-Start", &start));
        query.push(("X-Plex-Container-Size", &size));
        let Items { items, total_size } = self.get(path, &query).await?;
        let total = total_size.unwrap_or_else(|| u32::try_from(items.len()).unwrap_or(u32::MAX));
        Ok(Page { items, total })
    }

    /// One item in full, including its markers, its extras and the tracks
    /// of its files.
    pub async fn item(&self, key: &RatingKey) -> Result<Item> {
        item_from(self.item_json(key).await?)
    }

    /// One item in full, as the server wrote it. For keeping on disk:
    /// [`item_from`] reads it back.
    pub async fn item_json(&self, key: &RatingKey) -> Result<serde_json::Value> {
        let path = format!("/library/metadata/{key}");
        let query = [("includeMarkers", "1"), ("includeExtras", "1")];
        let RawItems { items } = self.get(&path, &query).await?;
        items.into_iter().next().ok_or_else(|| Error::Missing(key.clone()))
    }

    /// Rows of items that go with a film or show, such as others like it,
    /// each with up to `count` items.
    pub async fn related(&self, key: &RatingKey, count: u32) -> Result<Vec<Hub>> {
        let path = format!("/hubs/metadata/{key}/related");
        let hubs: Hubs = self.get(&path, &[("count", &count.to_string())]).await?;
        video_hubs(hubs, &path)
    }

    /// The four corner colours of the picture at server path `picture`,
    /// as an item carries them for its own artwork. For a picture shown
    /// with an item it does not belong to, such as a show's backdrop
    /// behind one of its episodes.
    pub async fn colors_of(&self, picture: &str) -> Result<Colors> {
        let Tones { colors: [colors] } =
            self.get("/services/ultrablur/colors", &[("url", picture)]).await?;
        Ok(colors)
    }

    /// The seasons of a show, or the episodes of a season.
    pub async fn children(&self, key: &RatingKey) -> Result<Vec<Item>> {
        let path = format!("/library/metadata/{key}/children");
        let Items { items, .. } = self.get(&path, &[]).await?;
        Ok(items)
    }

    /// Every episode of a show, in order, across all its seasons.
    pub async fn episodes(&self, show: &RatingKey) -> Result<Vec<Item>> {
        let path = format!("/library/metadata/{show}/allLeaves");
        let Items { items, .. } = self.get(&path, &[]).await?;
        Ok(items)
    }

    /// The items of `keys` that are still in the library, as a listing
    /// has them, in as few requests as the length of a path allows. The
    /// server takes rating keys joined by commas where it takes one.
    pub async fn items(&self, keys: &[RatingKey]) -> Result<Vec<Item>> {
        const KEYS_A_REQUEST: usize = 100;
        let mut found = Vec::with_capacity(keys.len());
        for keys in keys.chunks(KEYS_A_REQUEST) {
            let keys: Vec<&str> = keys.iter().map(RatingKey::as_str).collect();
            let path = format!("/library/metadata/{}", keys.join(","));
            let Items { items, .. } = self.get(&path, &[]).await?;
            found.extend(items);
        }
        Ok(found)
    }

    /// How many items of `kind` library `section` has, or how many of
    /// them are left to watch.
    pub async fn count(&self, section: &Section, kind: Kind, unwatched: bool) -> Result<u32> {
        // The numbers the server's `type` filter takes.
        let kind = match kind {
            Kind::Movie => "1",
            Kind::Show => "2",
            Kind::Season => "3",
            _ => "4",
        };
        let mut query = vec![("type", kind)];
        if unwatched {
            query.push(("unwatched", "1"));
        }
        let path = format!("/library/sections/{}/all", section.key);
        Ok(self.page(&path, &query, 0, 0).await?.total)
    }
}

/// Decodes an item from what [`MediaServer::item_json`] returned.
pub fn item_from(json: serde_json::Value) -> Result<Item> {
    let path = "/library/metadata".to_owned();
    serde_json::from_value(json).map_err(|source| Error::Shape { path, source })
}

/// Server path of the list of everything in library `section` that
/// `person` is in. `None` for a cast member the server gave no number,
/// which is every one in a listing: only an item fetched in full has them.
pub fn appearances(section: u32, person: &Role) -> Option<String> {
    Some(format!("/library/sections/{section}/all?actor={}", person.id?))
}

/// Server path of the list of everything in library `section` that
/// `person` directed. `None` for a director the server gave no number, as
/// for a member of the cast. A library of films lists films this way. One
/// of shows lists none, since it is their episodes that are directed.
pub fn directed(section: u32, person: &Role) -> Option<String> {
    Some(format!("/library/sections/{section}/all?director={}", person.id?))
}

/// Server path of the list of everything in library `section` of the
/// genre `genre`. `None` for a genre the server gave no number, which is
/// every one in a listing.
pub fn of_genre(section: u32, genre: &Tag) -> Option<String> {
    Some(format!("/library/sections/{section}/all?genre={}", genre.id?))
}

/// The episode to offer when the viewer asks to play a show, given all its
/// episodes in order: the one they stopped in the middle of, else the first
/// unwatched one after the last they finished, else the first unwatched
/// one, else the first.
pub fn next_up(episodes: &[Item]) -> Option<&Item> {
    let in_progress = episodes.iter().filter(|episode| episode.view_offset.is_some());
    if let Some(episode) = in_progress.max_by_key(|episode| episode.last_viewed_at) {
        return Some(episode);
    }
    let unwatched = |episode: &&Item| episode.view_count == 0;
    let after_last_watched = episodes.iter().rposition(|episode| episode.view_count > 0);
    let later = after_last_watched.and_then(|last| episodes[last + 1..].iter().find(unwatched));
    later.or_else(|| episodes.iter().find(unwatched)).or(episodes.first())
}

/// Keeps the rows that hold movies, shows or episodes and decodes them.
fn video_hubs(hubs: Hubs, path: &str) -> Result<Vec<Hub>> {
    const VIDEO: [&str; 5] = ["movie", "show", "season", "episode", "mixed"];

    hubs.hubs
        .into_iter()
        .filter(|hub| VIDEO.contains(&hub.kind.as_str()) && !hub.items.is_empty())
        .map(|hub| {
            let items = serde_json::from_value(serde_json::Value::Array(hub.items))
                .map_err(|source| Error::Shape { path: path.to_owned(), source })?;
            let more = hub.key.filter(|_| hub.more);
            Ok(Hub { title: hub.title, identifier: hub.hub_identifier, items, more })
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
    use crate::model::{MarkerKind, Rgb, StreamKind};
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
        assert!(page.items[0].media[0].parts[0].key.starts_with("/library/parts/"));
        let colors = page.items[0].colors.unwrap();
        assert_eq!(colors.top_left, Rgb { red: 0x49, green: 0x21, blue: 0x0f });
    }

    #[test]
    fn a_listing_filters_shows_by_their_episodes() {
        let listing = Listing { sort: Sort::DateAdded, unwatched_only: true };
        assert_eq!(listing.query(Kind::Movie), [("sort", "addedAt:desc"), ("unwatched", "1")]);
        assert_eq!(listing.query(Kind::Show), [("sort", "addedAt:desc"), ("unwatchedLeaves", "1")]);
        assert_eq!(Listing::default().query(Kind::Show), [("sort", "titleSort")]);
    }

    #[test]
    fn shows_seasons_and_episodes_decode() {
        let shows: Items = fixture!("shows");
        let seasons: Items = fixture!("seasons");
        let episodes: Items = fixture!("episodes");
        assert!(shows.items.iter().all(|show| show.kind == Kind::Show && show.media.is_empty()));
        assert!(seasons.items.iter().all(|season| season.kind == Kind::Season));
        assert!(episodes.items.iter().all(|episode| episode.kind == Kind::Episode));
    }

    #[test]
    fn a_full_episode_has_markers_and_tracks() {
        let episode = fixture!("episode" as Items).items.remove(0);

        assert_eq!(episode.markers[0].kind, MarkerKind::Intro);
        let kinds: Vec<StreamKind> =
            episode.media[0].parts[0].streams.iter().map(|stream| stream.kind).collect();
        assert_eq!(kinds, [StreamKind::Video, StreamKind::Audio, StreamKind::Subtitle]);
    }

    #[test]
    fn a_full_film_has_its_cast_and_its_extras() {
        let film = fixture!("film" as Items).items.remove(0);

        assert_eq!(film.library_section_id, Some(1));
        let actor = &film.roles[0];
        assert!(actor.id.is_some() && actor.role.is_some());
        assert!(actor.thumb.as_deref().is_some_and(|photo| photo.starts_with("https://")));
        // A trailer is passed on from plex.tv, so its part has no size.
        let trailer = &film.extras.items[0];
        assert_eq!(trailer.kind, Kind::Clip);
        assert_eq!(trailer.media[0].parts[0].size, None);
    }

    #[test]
    fn a_related_row_names_its_whole_list() {
        let hubs = video_hubs(fixture!("related"), "/hubs/metadata/370/related").unwrap();
        assert_eq!(hubs[0].identifier, "movie.similar");
        assert_eq!(hubs[0].more.as_deref(), Some("/library/metadata/370/similar"));
    }

    #[test]
    fn a_first_character_starts_after_those_before_it() {
        let characters = fixture!("first_characters" as FirstCharacters).placed();
        let placed: Vec<(&str, u32)> = characters
            .iter()
            .map(|character| (character.title.as_str(), character.start))
            .collect();
        assert_eq!(placed, [("#", 0), ("A", 1), ("B", 3), ("S", 8)]);
    }

    #[test]
    fn the_colours_of_a_picture_decode() {
        let Tones { colors: [colors] } = fixture!("colors");
        assert_eq!(colors.bottom_left, Rgb { red: 0x23, green: 0x52, blue: 0xb8 });
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

    /// Episodes numbered from 1, each given as (times watched, offset).
    fn episodes(states: &[(u32, Option<u64>)]) -> Vec<Item> {
        let episode = |(number, (views, offset)): (usize, &(u32, Option<u64>))| {
            serde_json::from_value(serde_json::json!({
                "ratingKey": number.to_string(), "type": "episode", "title": "Title",
                "index": number + 1, "viewCount": views, "viewOffset": offset,
                "lastViewedAt": offset,
            }))
            .unwrap()
        };
        states.iter().enumerate().map(episode).collect()
    }

    fn next_number(states: &[(u32, Option<u64>)]) -> Option<u32> {
        next_up(&episodes(states)).and_then(|episode| episode.index)
    }

    #[test]
    fn the_next_episode_is_the_one_left_unfinished() {
        assert_eq!(next_number(&[(1, None), (0, Some(5)), (0, None)]), Some(2));
        // Of two unfinished ones, the one played last.
        assert_eq!(next_number(&[(0, Some(9)), (0, Some(5)), (0, None)]), Some(1));
    }

    #[test]
    fn the_next_episode_follows_the_last_one_watched() {
        assert_eq!(next_number(&[(0, None), (1, None), (0, None)]), Some(3));
        // Nothing after the last watched one: back to the first gap.
        assert_eq!(next_number(&[(0, None), (1, None)]), Some(1));
        assert_eq!(next_number(&[(0, None), (0, None)]), Some(1));
        // All watched: start again.
        assert_eq!(next_number(&[(1, None), (1, None)]), Some(1));
        assert_eq!(next_number(&[]), None);
    }

    #[test]
    fn home_keeps_only_rows_of_video() {
        let hubs = video_hubs(fixture!("home"), "/hubs").unwrap();
        let identifiers: Vec<&str> = hubs.iter().map(|hub| hub.identifier.as_str()).collect();
        assert_eq!(identifiers, ["home.continue", "home.movies.recent", "home.television.recent"]);
    }

    #[test]
    fn search_drops_people_and_music() {
        let hubs = video_hubs(fixture!("search"), "/hubs/search").unwrap();
        let identifiers: Vec<&str> = hubs.iter().map(|hub| hub.identifier.as_str()).collect();
        assert_eq!(identifiers, ["show", "movie", "episode"]);
    }
}
