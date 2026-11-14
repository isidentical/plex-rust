//! Signing in to a Plex account.
//!
//! The user approves a short-lived PIN in the browser and plex.tv then
//! hands the token to whoever created the PIN:
//!
//! 1. [`request_pin`] creates the PIN.
//! 2. The user opens [`Pin::approval_url`] and signs in.
//! 3. [`wait_for_token`] returns once they have.

use std::time::Duration;

use serde::Deserialize;
use url::form_urlencoded;

use crate::client::{Client, PRODUCT, Token};
use crate::error::{Error, Result};

const PINS: &str = "https://plex.tv/api/v2/pins";
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// A pending sign-in.
#[derive(Debug, Clone, Deserialize)]
pub struct Pin {
    id: u64,
    code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PinStatus {
    /// `None` until the user approves the PIN.
    auth_token: Option<String>,
    /// Seconds until plex.tv forgets the PIN.
    expires_in: u64,
}

impl Pin {
    /// The page where the user approves this sign-in.
    pub fn approval_url(&self, client: &Client) -> String {
        let query = form_urlencoded::Serializer::new(String::new())
            .append_pair("clientID", &client.identity.client_identifier)
            .append_pair("code", &self.code)
            .append_pair("context[device][product]", PRODUCT)
            .finish();
        format!("https://app.plex.tv/auth#?{query}")
    }
}

pub async fn request_pin(client: &Client) -> Result<Pin> {
    let pin = client
        .http
        .post(PINS)
        // A strong PIN is too long to guess, so it is safe to put in a link.
        .query(&[("strong", "true")])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(pin)
}

/// Waits until the user has approved `pin` and returns their token.
pub async fn wait_for_token(client: &Client, pin: &Pin) -> Result<Token> {
    let url = format!("{PINS}/{}", pin.id);
    loop {
        let status: PinStatus =
            client.http.get(&url).send().await?.error_for_status()?.json().await?;
        if let Some(secret) = status.auth_token {
            return Ok(Token::new(secret));
        }
        if status.expires_in == 0 {
            return Err(Error::PinExpired);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
