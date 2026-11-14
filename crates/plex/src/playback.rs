//! Playing an item: where its file is, and telling the server how far the
//! viewer got.

use std::fmt;
use std::time::Duration;

use reqwest::Method;

use crate::client::{PRODUCT, header};
use crate::error::Result;
use crate::model::{Part, RatingKey};
use crate::server::MediaServer;

/// The identifier of the server's library, which several endpoints require.
const LIBRARY: &str = "com.plexapp.plugins.library";

/// An HTTP request that returns a file from the server. Its `Debug`
/// output names the headers without their values.
#[derive(Clone)]
pub struct FileRequest {
    pub url: String,
    /// Headers the request must carry, as name and value. The token is one
    /// of them, so it never appears in the URL.
    pub headers: Vec<(String, String)>,
}

impl fmt::Debug for FileRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("FileRequest").field("url", &self.url).field("headers", &names).finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Playing,
    Paused,
    Buffering,
    Stopped,
}

/// A moment of one playback, as reported to the server.
#[derive(Debug, Clone)]
pub struct Progress<'a> {
    pub item: &'a RatingKey,
    /// Unique for each playback. It lets the server tell two playbacks of
    /// the same item apart.
    pub session: &'a str,
    pub state: PlaybackState,
    pub position: Duration,
    pub duration: Duration,
    /// Whether this is told after the fact, about a viewing the server
    /// could not hear of while it went on.
    pub offline: bool,
}

impl MediaServer {
    /// The request for the original file of `part`, untouched by the server.
    pub fn original_file(&self, part: &Part) -> FileRequest {
        let identity = &self.client().identity;
        FileRequest {
            url: format!("{}{}", self.address(), part.key),
            headers: vec![
                ("X-Plex-Token".into(), self.token().secret().into()),
                ("X-Plex-Client-Identifier".into(), identity.client_identifier.clone()),
                ("X-Plex-Product".into(), PRODUCT.into()),
            ],
        }
    }

    /// Tells the server where a playback is. The server uses this for the
    /// resume position, for marking items watched, and for its dashboard.
    ///
    /// Plex asks for a report on every state change and every 10 seconds
    /// while playing.
    pub async fn report(&self, progress: &Progress<'_>) -> Result<()> {
        let state = match progress.state {
            PlaybackState::Playing => "playing",
            PlaybackState::Paused => "paused",
            PlaybackState::Buffering => "buffering",
            PlaybackState::Stopped => "stopped",
        };
        let offline: &[(&str, &str)] = if progress.offline { &[("offline", "1")] } else { &[] };
        self.request(Method::POST, "/:/timeline")
            .header(header("x-plex-session-identifier"), progress.session)
            .query(&[
                ("ratingKey", progress.item.as_str()),
                ("key", &format!("/library/metadata/{}", progress.item)),
                ("state", state),
                ("time", &progress.position.as_millis().to_string()),
                ("duration", &progress.duration.as_millis().to_string()),
            ])
            .query(offline)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Remembers which audio and subtitle tracks the viewer chose for
    /// `part`, by stream id. `None` leaves a choice as it is, and
    /// `Some(None)` for subtitles means none.
    pub async fn choose_streams(
        &self,
        part: &Part,
        audio: Option<u64>,
        subtitles: Option<Option<u64>>,
    ) -> Result<()> {
        let mut query = Vec::new();
        query.extend(audio.map(|id| ("audioStreamID", id.to_string())));
        // The server reads 0 as "no subtitles".
        query.extend(subtitles.map(|id| ("subtitleStreamID", id.unwrap_or(0).to_string())));
        self.request(Method::PUT, &format!("/library/parts/{}", part.id))
            .query(&query)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Marks an item as watched or unwatched.
    pub async fn set_watched(&self, item: &RatingKey, watched: bool) -> Result<()> {
        let path = if watched { "/:/scrobble" } else { "/:/unscrobble" };
        self.request(Method::PUT, path)
            .query(&[("identifier", LIBRARY), ("key", item.as_str())])
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{Client, Identity, Token};

    #[test]
    fn a_file_request_does_not_print_its_token() {
        let identity =
            Identity { client_identifier: "identifier".to_owned(), device_name: "Mac".to_owned() };
        let client = Client::new(identity).unwrap();
        let server =
            MediaServer::at(&client, "http://192.168.1.10:32400".to_owned(), Token::new("s3cret"));
        let part: Part = serde_json::from_value(serde_json::json!({
            "id": 1, "key": "/library/parts/1/0/file.mkv", "size": 1,
        }))
        .unwrap();

        let printed = format!("{:?}", server.original_file(&part));
        assert!(!printed.contains("s3cret"), "{printed}");
        assert!(
            printed.contains("X-Plex-Token") && printed.contains("/library/parts/1/0/file.mkv")
        );
    }
}
