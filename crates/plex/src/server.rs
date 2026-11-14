use serde::de::DeserializeOwned;

use crate::client::{Client, Token, header};
use crate::discovery;
use crate::error::Result;

/// A Plex Media Server that is reachable and that we hold a token for.
#[derive(Debug, Clone)]
pub struct MediaServer {
    client: Client,
    address: String,
    token: Token,
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
        Ok(Self {
            client: client.clone(),
            address,
            token: server.access_token.clone(),
        })
    }

    /// The address in use, such as `https://192-168-1-10.abc.plex.direct:32400`.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Fetches `path`, which starts with `/`, and returns the body as text.
    pub async fn get_text(&self, path: &str) -> Result<String> {
        Ok(self
            .request(path)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }

    /// Fetches `path` and decodes the contents of its `MediaContainer`.
    pub(crate) async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let envelope: Envelope<T> = self
            .request(path)
            .query(query)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(envelope.media_container)
    }

    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .http
            .get(format!("{}{path}", self.address))
            .header(header("x-plex-token"), self.token.secret())
    }
}
