//! What to call the audio and subtitle tracks of a file.

use objc2_foundation::{NSLocale, NSString};
use player::{Track, TrackKind};
use plex::model::{Part, Stream, StreamKind};

use crate::ui::formats;

/// The server's kind of stream for the player's kind of track.
pub fn stream_kind(kind: TrackKind) -> StreamKind {
    match kind {
        TrackKind::Video => StreamKind::Video,
        TrackKind::Audio => StreamKind::Audio,
        TrackKind::Subtitles => StreamKind::Subtitle,
    }
}

/// The server's entry for `track` among the streams of `part`.
///
/// For a track of the file itself, the player's id is its position among
/// the tracks of its kind, which is also the order the server lists them
/// in. Subtitles added from a file of their own are found by where that
/// file is, which ends in the server's path for it.
pub fn stream<'a>(track: &Track, part: &'a Part) -> Option<&'a Stream> {
    if let Some(location) = &track.external {
        let is_it =
            |stream: &&Stream| stream.key.as_deref().is_some_and(|key| location.ends_with(key));
        return part.sidecars().find(is_it);
    }
    let position = usize::try_from(track.id).ok()?.checked_sub(1)?;
    part.embedded(stream_kind(track.kind)).nth(position)
}

/// What `track` is called in a list.
///
/// The server's description is the best there is, such as "English (EAC3
/// 5.1)", and it is what every other Plex app shows. A track the server
/// does not list goes by what the file says about it.
pub fn name(track: &Track, part: Option<&Part>) -> String {
    let listed = part.and_then(|part| Some((stream(track, part)?, part)));
    if let Some((stream, part)) = listed.filter(|(stream, _)| stream.display_title.is_some()) {
        return formats::track_name(stream, part);
    }
    let language = track.language.as_deref().map(language_name);
    match (&track.title, language) {
        (Some(title), Some(language)) => format!("{language} ({title})"),
        (Some(name), None) => name.clone(),
        (None, Some(name)) => name,
        (None, None) => format!("Track {}", track.id),
    }
}

/// What the file itself calls `track`, such as "SDH" or "Director's
/// commentary", when its [`name`] does not say so already. Two subtitle
/// tracks that are both "English" are told apart by this.
pub fn detail(track: &Track, part: Option<&Part>) -> Option<String> {
    let title = track.title.as_deref()?.trim();
    let name = name(track, part).to_lowercase();
    (!title.is_empty() && !name.contains(&title.to_lowercase())).then(|| title.to_owned())
}

/// A detail longer than this many characters is a sentence about the
/// track, not a name for it, and is left to the list of tracks.
const LONGEST_DETAIL_IN_LINE: usize = 30;

/// The name of `track` on one line, with its detail if that is short.
pub fn described(track: &Track, part: Option<&Part>) -> String {
    let name = name(track, part);
    let detail = detail(track, part);
    match detail.filter(|detail| detail.chars().count() <= LONGEST_DETAIL_IN_LINE) {
        Some(detail) => format!("{name} \u{b7} {detail}"),
        None => name,
    }
}

/// The name, in the viewer's own language, of the language that files
/// refer to by a code such as `eng`. The code itself if it is not known.
fn language_name(code: &str) -> String {
    let name = NSLocale::currentLocale().localizedStringForLanguageCode(&NSString::from_str(code));
    name.map_or_else(|| code.to_owned(), |name| name.to_string())
}

#[cfg(test)]
mod tests {
    use player::{Track, TrackKind};
    use plex::model::Part;

    use super::{described, detail, stream};

    fn subtitles(id: i64, title: Option<&str>) -> Track {
        Track {
            id,
            kind: TrackKind::Subtitles,
            title: title.map(str::to_owned),
            language: Some("eng".to_owned()),
            selected: false,
            external: None,
        }
    }

    #[test]
    fn the_files_own_title_tells_tracks_of_one_language_apart() {
        let part: Part = serde_json::from_value(serde_json::json!({
            "id": 1, "key": "/library/parts/1/0/file.mkv", "size": 1, "Stream": [
                { "id": 1, "streamType": 3, "displayTitle": "English" },
                { "id": 2, "streamType": 3, "displayTitle": "English" },
                { "id": 3, "streamType": 3, "displayTitle": "English (SDH)" },
            ],
        }))
        .unwrap();
        let part = Some(&part);
        assert_eq!(detail(&subtitles(1, None), part), None);
        assert_eq!(described(&subtitles(2, Some("SDH")), part), "English \u{b7} SDH");
        // Said once is enough.
        assert_eq!(detail(&subtitles(3, Some("sdh")), part), None);
    }

    #[test]
    fn subtitles_from_their_own_file_are_found_by_where_it_is() {
        let part: Part = serde_json::from_value(serde_json::json!({
            "id": 1, "key": "/library/parts/1/0/file.mkv", "size": 1, "Stream": [
                { "id": 10, "streamType": 3, "displayTitle": "English" },
                { "id": 11, "streamType": 3, "key": "/library/streams/11",
                  "displayTitle": "Turkish (SRT External)" },
                { "id": 12, "streamType": 3, "key": "/library/streams/12",
                  "displayTitle": "German (SRT External)" },
            ],
        }))
        .unwrap();
        // The player numbers the added tracks after the file's own one.
        let added = |id: i64, location: &str| Track {
            external: Some(location.to_owned()),
            ..subtitles(id, None)
        };
        let german = added(3, "https://server:32400/library/streams/12");

        assert_eq!(stream(&german, &part).map(|stream| stream.id), Some(12));
        assert_eq!(described(&german, Some(&part)), "German (SRT External)");
        assert_eq!(stream(&subtitles(1, None), &part).map(|stream| stream.id), Some(10));
        // A track from a file the server does not list is not guessed at.
        assert_eq!(stream(&added(2, "/Users/anna/other.srt"), &part), None);
    }
}
