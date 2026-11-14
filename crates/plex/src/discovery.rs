//! Finding the account's servers and a working address for one of them.

use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use serde::Deserialize;

use crate::client::{Client, Token, header};
use crate::error::{Error, Result};
use crate::server::Envelope;

const RESOURCES: &str = "https://clients.plex.tv/api/v2/resources";

/// Long enough for a LAN or VPN round trip, short enough that an address
/// that is not routable from here does not hold up the app's start.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// A Plex Media Server the account can use, as listed by plex.tv.
#[derive(Debug, Clone)]
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

/// One entry of plex.tv's list, which also holds players and other
/// devices. Those can lack what a server always has, so nothing beyond the
/// roles is required until the entry is known to be a server.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Resource {
    /// Comma-separated roles, such as `server` or `client,player`.
    provides: String,
    name: String,
    client_identifier: String,
    access_token: Option<Token>,
    #[serde(default)]
    connections: Vec<Connection>,
}

impl Resource {
    /// The server this entry describes, if it is one we can sign in to.
    fn into_server(self) -> Option<Server> {
        let is_server = self.provides.split(',').any(|role| role == "server");
        Some(Server {
            name: self.name,
            client_identifier: self.client_identifier,
            access_token: self.access_token.filter(|_| is_server)?,
            connections: self.connections,
        })
    }
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

    Ok(resources.into_iter().filter_map(Resource::into_server).collect())
}

/// An address worth trying, and how much it is preferred.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    address: String,
    tier: Tier,
}

/// Kinds of address, most preferred first. Relays are not a tier: they are
/// never tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    LocalSecure,
    LocalPlain,
    /// The address the user typed.
    Manual,
    RemoteSecure,
}

const TIERS: usize = 4;

/// Finds an address at which `server` answers from this machine.
///
/// `manual` is an address the user typed, for networks plex.tv does not
/// know about, such as a VPN.
///
/// Every candidate is tried at once. The winner is the first to answer in
/// the most preferred tier that has an answer, and it is returned as soon
/// as no better tier can still produce one. A server often lists several
/// local addresses of which only one is reachable from here, and waiting
/// for the dead ones to time out would hold up every launch.
pub async fn connect(client: &Client, server: &Server, manual: Option<&str>) -> Result<String> {
    let candidates = candidates(&server.connections, manual);
    let mut probes: FuturesUnordered<_> =
        candidates
            .iter()
            .map(|candidate| async move {
                (candidate, answers(client, server, &candidate.address).await)
            })
            .collect();

    let mut pending = [0_usize; TIERS];
    for candidate in &candidates {
        pending[candidate.tier as usize] += 1;
    }
    let mut answered: [Option<&str>; TIERS] = [None; TIERS];

    while let Some((candidate, did_answer)) = probes.next().await {
        let tier = candidate.tier as usize;
        pending[tier] -= 1;
        if did_answer && answered[tier].is_none() {
            answered[tier] = Some(&candidate.address);
        }
        for tier in 0..TIERS {
            if let Some(address) = answered[tier] {
                return Ok(address.to_owned());
            }
            if pending[tier] > 0 {
                // A better tier than any answer so far is still undecided.
                break;
            }
        }
    }
    let tried = candidates.iter().map(|candidate| candidate.address.clone()).collect();
    Err(Error::Unreachable { server: server.name.clone(), tried })
}

/// The addresses worth trying, most preferred first.
fn candidates(connections: &[Connection], manual: Option<&str>) -> Vec<Candidate> {
    let tier = |connection: &Connection| {
        let secure = connection.uri.starts_with("https://");
        match (connection.relay, connection.local, secure) {
            (true, ..) => None,
            (false, true, true) => Some(Tier::LocalSecure),
            (false, true, false) => Some(Tier::LocalPlain),
            (false, false, true) => Some(Tier::RemoteSecure),
            // A remote address without TLS would send the token in clear.
            (false, false, false) => None,
        }
    };
    let listed = connections.iter().filter_map(|connection| {
        Some(Candidate { address: connection.uri.clone(), tier: tier(connection)? })
    });
    let manual = manual.map(|address| Candidate {
        address: address.trim_end_matches('/').to_owned(),
        tier: Tier::Manual,
    });

    let mut candidates: Vec<Candidate> = listed.chain(manual).collect();
    candidates.sort_by_key(|candidate| candidate.tier);
    candidates
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

        let candidates = candidates(&connections, Some("http://100.64.0.5:32400/"));
        let tried: Vec<(Tier, &str)> = candidates
            .iter()
            .map(|candidate| (candidate.tier, candidate.address.as_str()))
            .collect();
        assert_eq!(
            tried,
            [
                (Tier::LocalSecure, "https://192-168-1-10.abc.plex.direct:32400"),
                (Tier::LocalPlain, "http://192.168.1.10:32400"),
                (Tier::Manual, "http://100.64.0.5:32400"),
                (Tier::RemoteSecure, "https://remote.plex.direct:32400"),
            ]
        );
    }

    #[test]
    fn devices_that_are_not_servers_are_skipped_whatever_they_lack() {
        let resources: Vec<Resource> = serde_json::from_value(serde_json::json!([
            { "name": "TV", "clientIdentifier": "a", "provides": "client,player",
              "accessToken": null },
            { "name": "homelab", "clientIdentifier": "b", "provides": "server",
              "accessToken": "secret",
              "connections": [{ "uri": "http://192.168.1.10:32400", "local": true, "relay": false }] },
        ]))
        .unwrap();

        let servers: Vec<Server> =
            resources.into_iter().filter_map(Resource::into_server).collect();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "homelab");
    }
}
