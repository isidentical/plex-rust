//! The signed-in Plex account and this installation's identity.

use std::fs;
use std::path::PathBuf;

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSProcessInfo, NSString, NSURL, NSUUID};
use plex::{Client, Identity, Token, auth};
use security_framework::passwords::{get_generic_password, set_generic_password};

use crate::error::{Error, Result};

const KEYCHAIN_SERVICE: &str = "Reel";
const KEYCHAIN_ACCOUNT: &str = "plex.tv";

/// `errSecItemNotFound`: the Keychain has no such entry.
const ITEM_NOT_FOUND: i32 = -25300;

/// Signs in through the browser and stores the token in the Keychain.
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
    println!("Signed in. The token is in your Keychain.");
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
    let file_error = |source| Error::File {
        path: path.clone(),
        source,
    };

    let client_identifier = match fs::read_to_string(&path) {
        Ok(identifier) => identifier.trim().to_owned(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let identifier = NSUUID::UUID().UUIDString().to_string();
            fs::write(&path, &identifier).map_err(file_error)?;
            identifier
        }
        Err(error) => return Err(file_error(error)),
    };
    let device_name = NSProcessInfo::processInfo().hostName().to_string();
    Ok(Identity {
        client_identifier,
        device_name,
    })
}

fn support_directory() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or(Error::NoHome)?;
    let path = PathBuf::from(home).join("Library/Application Support/Reel");
    fs::create_dir_all(&path).map_err(|source| Error::File {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

fn stored_token() -> Result<Option<Token>> {
    match get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
        Ok(secret) => Ok(Some(Token::new(String::from_utf8_lossy(&secret)))),
        Err(error) if error.code() == ITEM_NOT_FOUND => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn store_token(token: &Token) -> Result<()> {
    set_generic_password(
        KEYCHAIN_SERVICE,
        KEYCHAIN_ACCOUNT,
        token.secret().as_bytes(),
    )?;
    Ok(())
}

fn open_in_browser(url: &str) {
    // The link is also printed, so a browser that fails to open is not fatal.
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}
