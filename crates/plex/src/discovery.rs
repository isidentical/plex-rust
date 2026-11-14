//! Finding the account's servers and a working address for one of them.

use std::time::Duration;

use futures_util::future::join_all;
use serde::Deserialize;

use crate::client::{Client, Token, header};
use crate::error::{Error, Result};
use crate::server::Envelope;

const RESOURCES: &str = "https://clients.plex.tv/api/v2/resources";

/// Long enough for a LAN or VPN round trip, short enough that an address
/// that is not routable from here does not hold up the app's start.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// A Plex Media Server the account can use, as listed by plex.tv.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub name: String,
    /// The server's machine identifier.
    pub client_identifier: String,
    /// The token for this server. It differs from the account token for
    /// servers shared by someone else.
    pub access_token: Token,
    pub connections: Vec<Connection>,
}

/// One address plex.tv knows for a server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Connection {
    pub uri: String,
    /// On the same network as the server, as far as plex.tv can tell.
    pub local: bool,
    /// Through Plex's bandwidth-limited relay.
    pub relay: bool,
}

#[derive(Deserialize)]
struct Resource {
    /// Comma-separated roles, such as `server` or `client,player`.
    provides: String,
    #[serde(flatten)]
    server: Server,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Identity {
    machine_identifier: String,
}

/// Lists the servers the account has access to.
pub async fn servers(client: &Client, token: &Token) -> Result<Vec<Server>> {
    let resources: Vec<Resource> = client
        .http
        .get(RESOURCES)
        .query(&[("includeHttps", "1")])
        .header(header("x-plex-token"), token.secret())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let is_server = |resource: &Resource| resource.provides.split(',').any(|role| role == "server");
    Ok(resources.into_iter().filter(is_server).map(|resource| resource.server).collect())
}

/// Finds an address at which `server` answers from this machine.
///
/// `manual` is an address the user typed, for networks plex.tv does not
/// know about, such as a VPN. All candidates are tried at once and the most
/// preferred one that answers wins.
pub async fn connect(client: &Client, server: &Server, manual: Option<&str>) -> Result<String> {
    let candidates = candidates(&server.connections, manual);
    let answers = join_all(candidates.iter().map(|address| answers(client, server, address))).await;

    candidates.iter().zip(answers).find(|(_, answered)| *answered).map_or_else(
        || Err(Error::Unreachable { server: server.name.clone(), tried: candidates.clone() }),
        |(address, _)| Ok(address.clone()),
    )
}

/// The addresses worth trying, most preferred first: local over TLS, local
/// without, the manual address, then remote over TLS. Relays are left out.
fn candidates(connections: &[Connection], manual: Option<&str>) -> Vec<String> {
    let direct = || connections.iter().filter(|connection| !connection.relay);
    let secure = |connection: &&Connection| connection.uri.starts_with("https://");

    let local_secure = direct().filter(|c| c.local).filter(secure);
    let local_plain = direct().filter(|c| c.local).filter(|c| !secure(c));
    let remote_secure = direct().filter(|c| !c.local).filter(secure);

    local_secure
        .chain(local_plain)
        .map(|connection| connection.uri.clone())
        .chain(manual.map(|address| address.trim_end_matches('/').to_owned()))
        .chain(remote_secure.map(|connection| connection.uri.clone()))
        .collect()
}

/// Whether `server` itself, and not some other machine, answers at `address`.
async fn answers(client: &Client, server: &Server, address: &str) -> bool {
    let identity = async {
        let response = client
            .http
            .get(format!("{address}/identity"))
            .timeout(PROBE_TIMEOUT)
            .send()
            .await?
            .error_for_status()?;
        response.json::<Envelope<Identity>>().await
    };
    // Any failure means the same thing here: this address is not usable.
    identity.await.is_ok_and(|identity| {
        identity.media_container.machine_identifier == server.client_identifier
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(uri: &str, local: bool, relay: bool) -> Connection {
        Connection { uri: uri.to_owned(), local, relay }
    }

    #[test]
    fn candidates_are_ordered_by_preference_and_skip_relays() {
        let connections = [
            connection("https://relay.plex.direct:8443", false, true),
            connection("https://remote.plex.direct:32400", false, false),
            connection("http://192.168.1.10:32400", true, false),
            connection("http://203.0.113.7:32400", false, false),
            connection("https://192-168-1-10.abc.plex.direct:32400", true, false),
        ];

        assert_eq!(
            candidates(&connections, Some("http://100.64.0.5:32400/")),
            [
                "https://192-168-1-10.abc.plex.direct:32400",
                "http://192.168.1.10:32400",
                "http://100.64.0.5:32400",
                "https://remote.plex.direct:32400",
            ]
        );
    }
}
