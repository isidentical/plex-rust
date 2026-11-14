use std::path::{Path, PathBuf};
use std::sync::Arc;

use reqwest::Method;
use serde::de::DeserializeOwned;

use crate::client::{Client, Token, header};
use crate::discovery;
use crate::error::{Error, Result};

/// A Plex Media Server that we hold a token for.
#[derive(Debug, Clone)]
pub struct MediaServer {
    client: Client,
    address: String,
    token: Token,
    /// Where answers are kept between launches, if anywhere.
    cache: Option<Arc<Path>>,
    answers: Answers,
}

/// Where the answers to requests come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answers {
    /// From the server. Each answer is also written to the cache.
    Server,
    /// From the cache only, without touching the network.
    Cache,
}

/// Every response from the server is wrapped in this object.
#[derive(serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Envelope<T> {
    pub media_container: T,
}

impl MediaServer {
    /// Finds a working address for `server` and connects to it. See
    /// [`discovery::connect`] for what `manual` is.
    pub async fn connect(
        client: &Client,
        server: &discovery::Server,
        manual: Option<&str>,
    ) -> Result<Self> {
        let address = discovery::connect(client, server, manual).await?;
        Ok(Self::at(client, address, server.access_token.clone()))
    }

    /// The server at `address`, without checking that it answers there.
    /// For an address that worked the last time the app ran.
    pub fn at(client: &Client, address: String, token: Token) -> Self {
        Self { client: client.clone(), address, token, cache: None, answers: Answers::Server }
    }

    /// Keeps the answers to library requests in `directory`, so that
    /// [`remembered`](Self::remembered) can give them back later.
    pub fn with_cache(mut self, directory: PathBuf) -> Self {
        self.cache = Some(directory.into());
        self
    }

    /// The same server, answering only from what was kept on disk the last
    /// time each question was asked. Nothing touches the network, and a
    /// question never asked before fails with [`Error::NotRemembered`].
    ///
    /// It lets a screen draw at once with what it showed last time, before
    /// the server has said anything.
    pub fn remembered(&self) -> Self {
        Self { answers: Answers::Cache, ..self.clone() }
    }

    /// The same server, keeping no answers on disk and remembering none.
    pub(crate) fn forgetful(&self) -> Self {
        Self { cache: None, ..self.clone() }
    }

    /// The token for this server, for storing alongside its address.
    pub fn token(&self) -> &Token {
        &self.token
    }

    /// The address in use, such as `https://192-168-1-10.abc.plex.direct:32400`.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Sends a request for `path`, which starts with `/`, and returns the
    /// body as text. For looking at what the server says, unparsed.
    pub async fn text(&self, method: Method, path: &str) -> Result<String> {
        let response = self.request(method, path).send().await?;
        Ok(response.error_for_status()?.text().await?)
    }

    /// Fetches `path`, which may end in a query of its own, with `query`
    /// added to it, and decodes the contents of its `MediaContainer`.
    pub(crate) async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let file = self.cache.as_deref().map(|directory| {
            let question = format!("{}{path}?{query:?}", self.address);
            directory.join(format!("{:016x}.json", stable_hash(&question)))
        });

        let body = match (self.answers, &file) {
            (Answers::Cache, Some(file)) => {
                tokio::fs::read_to_string(file).await.map_err(|_| Error::NotRemembered)?
            }
            (Answers::Cache, None) => return Err(Error::NotRemembered),
            (Answers::Server, _) => {
                let request = self.request(Method::GET, path).query(query);
                let body = request.send().await?.error_for_status()?.text().await?;
                if let Some(file) = &file {
                    // A failed write only means a slower start next time.
                    let _ = tokio::fs::write(file, &body).await;
                }
                body
            }
        };
        let envelope: Envelope<T> = serde_json::from_str(&body)
            .map_err(|source| Error::Shape { path: path.to_owned(), source })?;
        Ok(envelope.media_container)
    }

    /// A request to this server that carries its token.
    pub(crate) fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .http
            .request(method, format!("{}{path}", self.address))
            .header(header("x-plex-token"), self.token.secret())
    }

    pub(crate) fn client(&self) -> &Client {
        &self.client
    }
}

/// FNV-1a. Unlike the standard library's hasher it gives the same value in
/// every build, which a name for a file on disk needs.
pub fn stable_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
