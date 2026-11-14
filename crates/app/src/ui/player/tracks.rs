//! What to call the audio and subtitle tracks of a file.

use objc2_foundation::{NSLocale, NSString};
use player::{Track, TrackKind};
use plex::model::{Part, StreamKind};

/// The server's kind of stream for the player's kind of track.
pub fn stream_kind(kind: TrackKind) -> StreamKind {
    match kind {
        TrackKind::Video => StreamKind::Video,
        TrackKind::Audio => StreamKind::Audio,
        TrackKind::Subtitles => StreamKind::Subtitle,
    }
}

/// What `track` is called in a menu.
///
/// The server's description is the best there is, such as "English (EAC3
/// 5.1)", and it is what every other Plex app shows. The player's id for a
/// track is its position among the tracks of its kind in `part`, which is
/// also the order the server lists them in. A track the server does not
/// list there goes by what the file says about it.
pub fn name(track: &Track, part: Option<&Part>) -> String {
    let position = usize::try_from(track.id).ok().and_then(|id| id.checked_sub(1));
    let stream = part
        .zip(position)
        .and_then(|(part, position)| part.embedded(stream_kind(track.kind)).nth(position));
    if let Some(title) = stream.and_then(|stream| stream.display_title.clone()) {
        return title;
    }
    let language = track.language.as_deref().map(language_name);
    match (&track.title, language) {
        (Some(title), Some(language)) => format!("{language} ({title})"),
        (Some(name), None) => name.clone(),
        (None, Some(name)) => name,
        (None, None) => format!("Track {}", track.id),
    }
}

/// The name, in the viewer's own language, of the language that files
/// refer to by a code such as `eng`. The code itself if it is not known.
fn language_name(code: &str) -> String {
    let name = NSLocale::currentLocale().localizedStringForLanguageCode(&NSString::from_str(code));
    name.map_or_else(|| code.to_owned(), |name| name.to_string())
}
