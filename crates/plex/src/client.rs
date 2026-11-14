use std::fmt;
use std::time::Duration;

use reqwest::header::{ACCEPT, HeaderMap, HeaderName, HeaderValue};

use crate::error::{Error, Result};

/// The name this app goes by in Plex's device lists and dashboards.
pub(crate) const PRODUCT: &str = "plex-rust";

/// How this installation presents itself to Plex.
#[derive(Debug, Clone)]
pub struct Identity {
    /// Unique per installation and stable across launches. Plex ties
    /// sign-ins and playback sessions to it.
    pub client_identifier: String,
    /// The name shown in Plex's list of authorized devices.
    pub device_name: String,
}

/// A Plex access token. Its `Debug` output never shows the secret.
#[derive(Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(transparent)]
pub struct Token(String);

impl Token {
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    /// The secret itself, for sending to Plex or storing.
    pub fn secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

/// An HTTP client that identifies itself to Plex on every request.
#[derive(Debug, Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
    pub(crate) identity: Identity,
}

impl Client {
    pub fn new(identity: Identity) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(header("x-plex-product"), HeaderValue::from_static(PRODUCT));
        headers.insert(header("x-plex-platform"), HeaderValue::from_static("macOS"));
        headers
            .insert(header("x-plex-version"), HeaderValue::from_static(env!("CARGO_PKG_VERSION")));
        headers
            .insert(header("x-plex-client-identifier"), header_value(&identity.client_identifier)?);
        headers.insert(header("x-plex-device-name"), header_value(&identity.device_name)?);

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { http, identity })
    }
}

pub(crate) fn header(name: &'static str) -> HeaderName {
    HeaderName::from_static(name)
}

/// Plex asks for header values as UTF-8, which HTTP allows as opaque bytes.
fn header_value(text: &str) -> Result<HeaderValue> {
    HeaderValue::from_bytes(text.as_bytes()).map_err(|_| Error::Header(text.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_named(device_name: &str) -> Result<Client> {
        Client::new(Identity {
            client_identifier: "identifier".to_owned(),
            device_name: device_name.to_owned(),
        })
    }

    #[test]
    fn a_name_that_cannot_be_a_header_is_an_error() {
        // A Mac can be named anything, and the name is sent as a header.
        assert!(matches!(client_named("Anna\u{2019}s Mac\n"), Err(Error::Header(_))));
        assert!(client_named("Anna\u{2019}s Mac").is_ok());
    }
}
