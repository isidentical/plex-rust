//! Subtitles the server finds online for an item, and adding one of them.
//!
//! Plex's API description (1.2.3) does not list either request. Both are
//! as python-plexapi makes them, in `Video.searchSubtitles` and
//! `Video.downloadSubtitles`
//! (<https://github.com/pkkid/python-plexapi/blob/master/plexapi/video.py>).
//! The server searches through its OpenSubtitles agent, which needs Plex
//! Pass (<https://support.plex.tv/articles/subtitle-search/>).

use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Deserializer};

use crate::error::{Error, Result};
use crate::model::{Part, RatingKey, Stream};
use crate::server::{Envelope, MediaServer};

/// How long to wait for subtitles the server was asked to add before
/// giving up on them. It adds them after it has answered the request.
const ADDING_TAKES_AT_MOST: Duration = Duration::from_secs(20);
/// How often to look in that time.
const ADDING_LOOKS_EVERY: Duration = Duration::from_secs(1);

/// Subtitles the server found and can add to an item.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    /// What the server is to be given to add these. Its own path for them,
    /// such as `/library/streams/21984`.
    pub key: String,
    /// The file's name where it was found, which says which release of the
    /// film it was timed to.
    pub title: Option<String>,
    /// Where they were found, such as "OpenSubtitles".
    pub provider_title: Option<String>,
    /// The language in words, such as "English".
    pub language: Option<String>,
    /// How they are encoded, such as `srt`.
    pub codec: Option<String>,
    #[serde(default)]
    pub hearing_impaired: bool,
    #[serde(default)]
    pub forced: bool,
    /// How well they are rated where they were found. python-plexapi
    /// calls it a download count. The server sends it as text.
    #[serde(default, deserialize_with = "number_in_text")]
    pub score: Option<u64>,
}

#[derive(Deserialize)]
struct Candidates {
    #[serde(rename = "Stream", default)]
    candidates: Vec<Candidate>,
}

impl MediaServer {
    /// Subtitles in `language`, a two letter ISO 639-1 code such as `en`,
    /// that the server finds for `item`, the highest scored first. Not kept
    /// for a later launch: each search is the server's latest word.
    pub async fn search_subtitles(
        &self,
        item: &RatingKey,
        language: &str,
    ) -> Result<Vec<Candidate>> {
        let path = format!("/library/metadata/{item}/subtitles");
        // 0 for both is python-plexapi's default: prefer subtitles that are
        // neither for the hard of hearing nor forced, without leaving
        // those out.
        let query = [("language", language), ("hearingImpaired", "0"), ("forced", "0")];
        let response = self.request(Method::GET, &path).query(&query).send().await?;
        let body = response.error_for_status()?.bytes().await?;
        let envelope: Envelope<Candidates> =
            serde_json::from_slice(&body).map_err(|source| Error::Shape { path, source })?;
        Ok(ranked(envelope.media_container.candidates))
    }

    /// Has the server add `candidate` to `item` and returns the subtitles
    /// as they now are among the streams of `part`, the file of `item`
    /// they were found for.
    ///
    /// The server answers before it has fetched them, so this looks at the
    /// item again until they are there.
    pub async fn add_subtitles(
        &self,
        item: &RatingKey,
        part: &Part,
        candidate: &Candidate,
    ) -> Result<Stream> {
        self.adding(item, candidate).send().await?.error_for_status()?;
        let tries = ADDING_TAKES_AT_MOST.as_secs() / ADDING_LOOKS_EVERY.as_secs();
        for _ in 0..tries {
            tokio::time::sleep(ADDING_LOOKS_EVERY).await;
            let now = self.item(item).await?;
            let parts = now.media.iter().flat_map(|media| &media.parts);
            let added = parts.filter(|now| now.id == part.id).find_map(|now| added(part, now));
            if let Some(stream) = added {
                return Ok(stream.clone());
            }
        }
        Err(Error::NotAdded(candidate.title.clone().unwrap_or_else(|| candidate.key.clone())))
    }

    /// The request that has the server add `candidate` to `item`.
    fn adding(&self, item: &RatingKey, candidate: &Candidate) -> reqwest::RequestBuilder {
        let path = format!("/library/metadata/{item}/subtitles");
        self.request(Method::PUT, &path).query(&[("key", candidate.key.as_str())])
    }
}

/// `candidates` with the highest score first. The server lists them in no
/// order of score.
fn ranked(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));
    candidates
}

/// The subtitle file that `after` has and `before`, the same part earlier,
/// did not.
fn added<'a>(before: &Part, after: &'a Part) -> Option<&'a Stream> {
    let was_there = |stream: &Stream| before.streams.iter().any(|old| old.id == stream.id);
    after.sidecars().find(|stream| !was_there(stream))
}

/// A whole number the server writes as text, such as `"1232"`.
fn number_in_text<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error> {
    let Some(text) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };
    text.parse().map(Some).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{Client, Identity, Token};
    use crate::model::StreamKind;

    fn candidates() -> Vec<Candidate> {
        let json = include_str!("../tests/fixtures/subtitles.json");
        let envelope: Envelope<Candidates> = serde_json::from_str(json).unwrap();
        envelope.media_container.candidates
    }

    #[test]
    fn candidates_decode() {
        let candidates = candidates();
        assert_eq!(candidates.len(), 10);
        let first = &candidates[0];
        assert_eq!(first.key, "/library/streams/21984");
        assert_eq!(first.provider_title.as_deref(), Some("OpenSubtitles"));
        assert_eq!(first.language.as_deref(), Some("English"));
        assert_eq!(first.codec.as_deref(), Some("srt"));
        assert_eq!(first.score, Some(1232));
        assert!(!first.hearing_impaired && !first.forced);
    }

    #[test]
    fn a_score_that_is_not_a_number_is_an_error() {
        let json = r#"{ "key": "/library/streams/1", "score": "many" }"#;
        assert!(serde_json::from_str::<Candidate>(json).is_err());
    }

    #[test]
    fn the_highest_score_comes_first() {
        let scores: Vec<Option<u64>> =
            ranked(candidates()).iter().map(|candidate| candidate.score).collect();
        assert_eq!(scores[..3], [Some(590_295), Some(527_488), Some(49_676)]);
        assert!(scores.is_sorted_by(|a, b| a >= b));
    }

    #[test]
    fn the_added_subtitles_are_the_new_file() {
        let stream = |id: u64, kind: u8, key: Option<&str>| {
            let json = serde_json::json!({ "id": id, "streamType": kind, "key": key });
            serde_json::from_value::<Stream>(json).unwrap()
        };
        let part = |streams: Vec<Stream>| Part { id: 1, key: String::new(), size: None, streams };
        let before = part(vec![stream(10, 1, None), stream(12, 3, Some("/library/streams/12"))]);
        let mut after = before.clone();
        assert_eq!(added(&before, &after), None);
        after.streams.push(stream(30, 3, Some("/library/streams/30")));
        assert_eq!(added(&before, &after).map(|stream| stream.id), Some(30));
        assert_eq!(added(&before, &after).map(|stream| stream.kind), Some(StreamKind::Subtitle));
    }

    #[test]
    fn adding_puts_only_the_candidates_key() {
        let identity =
            Identity { client_identifier: "identifier".to_owned(), device_name: "Mac".to_owned() };
        let client = Client::new(identity).unwrap();
        let server = MediaServer::at(
            &client,
            "http://192.168.1.10:32400".to_owned(),
            "machine".to_owned(),
            Token::new("secret"),
        );
        let candidate = &candidates()[0];
        let request = server.adding(&RatingKey::new("370"), candidate).build().unwrap();
        assert_eq!(request.method(), Method::PUT);
        assert_eq!(
            request.url().as_str(),
            "http://192.168.1.10:32400/library/metadata/370/subtitles?key=%2Flibrary%2Fstreams%2F21984"
        );
    }
}
