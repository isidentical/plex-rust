//! Reel, a native macOS client for Plex.

mod account;
mod background;
mod error;
mod images;
mod play;
mod progress;
mod ui;
mod video_layer;

use std::process::ExitCode;
use std::time::Duration;

use player::Source;
use plex::model::RatingKey;

use crate::error::{Error, Result};
use crate::progress::Progress;
use crate::ui::shell::Subject;

const USAGE: &str = "\
usage: reel
       reel snapshot home <png-path>
       reel snapshot library <index> <png-path>
       reel snapshot item <rating-key> <png-path>
       reel snapshot play <rating-key> <png-path>
       reel login
       reel servers [<address>]
       reel api [--post] <path>
       reel play <file-or-url> [--unattended <seconds>]
       reel play item:<rating-key> [--unattended <seconds>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("reel: {}", error::with_causes(&error));
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[&str]) -> Result<()> {
    match args {
        [] => ui::shell::run(None),
        ["snapshot", "home", path] => ui::shell::run(Some((Subject::Home, path.into()))),
        ["snapshot", "library", index, path] => {
            let index = index.parse().map_err(|_| Error::Usage(USAGE))?;
            ui::shell::run(Some((Subject::Library(index), path.into())))
        }
        ["snapshot", "item", key, path] => {
            ui::shell::run(Some((Subject::Item((*key).to_owned()), path.into())))
        }
        ["snapshot", "play", key, path] => {
            ui::shell::run(Some((Subject::Play((*key).to_owned()), path.into())))
        }
        ["login"] => account::login(),
        ["servers"] => servers(None),
        ["servers", address] => servers(Some(address)),
        ["api", path] => api(plex::Method::GET, path),
        ["api", "--post", path] => api(plex::Method::POST, path),
        ["play", what] => play(what, None),
        ["play", what, "--unattended", seconds] => {
            let seconds = seconds.parse().map_err(|_| Error::Usage(USAGE))?;
            play(what, Some(Duration::from_secs(seconds)))
        }
        _ => Err(Error::Usage(USAGE)),
    }
}

/// Plays a file, a URL, or with `item:<rating-key>` an item from the first
/// server, resuming where it was left and reporting progress.
fn play(what: &str, unattended: Option<Duration>) -> Result<()> {
    let Some(key) = what.strip_prefix("item:") else {
        return Ok(play::run(&Source::new(what), None, unattended)?);
    };

    let (client, token) = account::signed_in()?;
    let runtime = runtime()?;
    let (server, item) = runtime.block_on(async {
        let servers = plex::discovery::servers(&client, &token).await?;
        let server = servers.first().ok_or(Error::NoServer)?;
        let server = plex::MediaServer::connect(&client, server, None).await?;
        let item = server.item(&RatingKey::new(key)).await?;
        Ok::<_, Error>((server, item))
    })?;

    let part = item.media.first().and_then(|media| media.parts.first());
    let part = part.ok_or_else(|| Error::NotPlayable(item.title.clone()))?;
    let file = server.original_file(part);
    let source = Source {
        location: file.url,
        headers: file.headers,
        start: item.view_offset.map(Into::into).unwrap_or_default(),
    };
    // An unattended run is a check. It must not move anyone's place in a
    // film, so it tells the server nothing.
    let progress = unattended.is_none().then(|| Progress::new(runtime, server, item.rating_key));
    Ok(play::run(&source, progress, unattended)?)
}

/// Lists the account's servers and the address each one answers at.
/// `manual` is an extra address to try, such as a VPN address.
fn servers(manual: Option<&str>) -> Result<()> {
    let (client, token) = account::signed_in()?;
    runtime()?.block_on(async {
        for server in plex::discovery::servers(&client, &token).await? {
            match plex::discovery::connect(&client, &server, manual).await {
                Ok(address) => println!("{}: {address}", server.name),
                Err(error) => println!("{error}"),
            }
        }
        Ok(())
    })
}

/// Prints the first server's raw response to a request for `path`, such
/// as `/hubs`.
fn api(method: plex::Method, path: &str) -> Result<()> {
    let (client, token) = account::signed_in()?;
    runtime()?.block_on(async {
        let servers = plex::discovery::servers(&client, &token).await?;
        let server = servers.first().ok_or(Error::NoServer)?;
        let server = plex::MediaServer::connect(&client, server, None).await?;
        println!("{}", server.text(method, path).await?);
        Ok(())
    })
}

/// A runtime for commands that make a few requests and exit.
fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(Error::Runtime)
}
