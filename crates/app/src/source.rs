//! Turning a library item into something the player can load.

use player::Source;
use plex::MediaServer;
use plex::model::{Item, StreamKind};

use crate::error::{Error, Result};

/// The original file of `item` on `server`, starting where the viewer left
/// off, with the audio and subtitle tracks they last chose.
///
/// Subtitles stay off unless the viewer chose some. A file may mark a
/// subtitle track as its default, and that is not the viewer asking for it.
pub fn of(server: &MediaServer, item: &Item) -> Result<Source> {
    let part = item.media.first().and_then(|media| media.parts.first());
    let part = part.ok_or_else(|| Error::NotPlayable(item.title.clone()))?;
    let file = server.original_file(part);
    Ok(Source {
        location: file.url,
        headers: file.headers,
        start: item.view_offset.map(Into::into).unwrap_or_default(),
        audio: part.chosen(StreamKind::Audio),
        subtitles: part.chosen(StreamKind::Subtitle),
    })
}
