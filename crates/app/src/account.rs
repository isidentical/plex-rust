//! The signed-in Plex account and this installation's identity.

use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSProcessInfo, NSString, NSURL, NSUUID};
use plex::{Client, Identity, Token, auth};

use crate::error::{Error, Result};

/// Signs in through the browser and stores the token.
pub fn login() -> Result<()> {
    let client = Client::new(identity()?)?;
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
    Ok((Client::new(identity()?)?, token))
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

fn support_directory() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or(Error::NoHome)?;
    let path = PathBuf::from(home).join("Library/Application Support/Reel");
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

fn store_token(token: &Token) -> Result<()> {
    let path = token_path()?;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut file| file.write_all(token.secret().as_bytes()))
        .map_err(|source| Error::File { path, source })
}

fn open_in_browser(url: &str) {
    // The link is also printed, so a browser that fails to open is not fatal.
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}
