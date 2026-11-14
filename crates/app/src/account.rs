//! The signed-in Plex account and this installation's identity.

use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSProcessInfo, NSString, NSURL, NSUUID};
use plex::{Client, Identity, MediaServer, Token, auth};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Signs in through the browser and stores the token.
pub fn login() -> Result<()> {
    let client = client()?;
    let token = crate::runtime()?.block_on(async {
        let pin = auth::request_pin(&client).await?;
        let url = pin.approval_url(&client);
        println!("Approve the sign-in in your browser:\n\n  {url}\n");
        open_in_browser(&url);
        auth::wait_for_token(&client, &pin).await
    })?;

    store_token(&token)?;
    println!("Signed in.");
    Ok(())
}

/// A client and the token of the signed-in account.
pub fn signed_in() -> Result<(Client, Token)> {
    let token = stored_token()?.ok_or(Error::SignedOut)?;
    Ok((client()?, token))
}

/// A client that presents itself to Plex as this installation.
pub fn client() -> Result<Client> {
    Ok(Client::new(identity()?)?)
}

/// Loads this installation's identity, creating it on first use.
fn identity() -> Result<Identity> {
    let path = support_directory()?.join("client-identifier");
    let file_error = |source| Error::File { path: path.clone(), source };

    let client_identifier = match fs::read_to_string(&path) {
        Ok(identifier) => identifier.trim().to_owned(),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let identifier = NSUUID::UUID().UUIDString().to_string();
            fs::write(&path, &identifier).map_err(file_error)?;
            identifier
        }
        Err(error) => return Err(file_error(error)),
    };
    let device_name = NSProcessInfo::processInfo().hostName().to_string();
    Ok(Identity { client_identifier, device_name })
}

/// The folder for the app's own files, created if it is not there.
pub fn support_directory() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or(Error::NoHome)?;
    let path = PathBuf::from(home).join("Library/Application Support/plex-rust");
    fs::create_dir_all(&path).map_err(|source| Error::File { path: path.clone(), source })?;
    Ok(path)
}

/// The token lives in a file only its owner can read. It was in the
/// Keychain at first, but the Keychain ties access to the exact binary, so
/// every rebuild of an ad hoc signed app asked for the login password again.
fn token_path() -> Result<PathBuf> {
    Ok(support_directory()?.join("token"))
}

fn stored_token() -> Result<Option<Token>> {
    let path = token_path()?;
    match fs::read_to_string(&path) {
        Ok(secret) => Ok(Some(Token::new(secret.trim()))),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::File { path, source }),
    }
}

/// Keeps the account's token for the next launch.
pub fn store_token(token: &Token) -> Result<()> {
    write_private(token_path()?, token.secret().as_bytes())
}

pub fn open_in_browser(url: &str) {
    // A browser that fails to open is not fatal: the command line prints
    // the link as well, and the window offers to try again.
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

/// What is kept of a server between launches: enough to talk to it again
/// without asking plex.tv where it is.
#[derive(Serialize, Deserialize)]
struct SavedServer {
    address: String,
    token: String,
}

/// The server used the last time the app ran, if there was one. Nothing
/// checks here that it still answers at that address.
pub fn saved_server(client: &Client) -> Result<Option<MediaServer>> {
    let path = support_directory()?.join("server.json");
    let json = match fs::read_to_string(&path) {
        Ok(json) => json,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(Error::File { path, source }),
    };
    let saved: SavedServer =
        serde_json::from_str(&json).map_err(|source| Error::Settings { path, source })?;
    Ok(Some(MediaServer::at(client, saved.address, Token::new(saved.token))))
}

/// Keeps `server` for the next launch. Its token goes into a file only the
/// owner can read, like the account token.
pub fn save_server(server: &MediaServer) -> Result<()> {
    let path = support_directory()?.join("server.json");
    let saved = SavedServer {
        address: server.address().to_owned(),
        token: server.token().secret().to_owned(),
    };
    let json = serde_json::to_string(&saved)
        .map_err(|source| Error::Settings { path: path.clone(), source })?;
    write_private(path, json.as_bytes())
}

fn write_private(path: PathBuf, contents: &[u8]) -> Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut file| file.write_all(contents))
        .map_err(|source| Error::File { path, source })
}
