//! plex-rust, a native macOS client for Plex.

mod account;
mod background;
mod error;
mod images;
mod pacing;
mod play;
mod progress;
mod seeks;
mod settings;
mod source;
mod ui;
mod video_layer;

use std::process::ExitCode;
use std::time::Duration;

use player::Source;
use plex::library::Sort;
use plex::model::RatingKey;

use crate::error::{Error, Result};
use crate::play::Check;
use crate::progress::Progress;
use crate::ui::player::input::Key;
use crate::ui::shell::{Link, Snapshot, Subject, Tweaks};

const USAGE: &str = "\
usage: plex-rust
       plex-rust snapshot home <png-path> [<tweak>...]
       plex-rust snapshot library <index> <png-path> [<tweak>...]
       plex-rust snapshot item <rating-key> <png-path> [<tweak>...]
       plex-rust snapshot search <query> <png-path> [<tweak>...]
       plex-rust snapshot list <server-path> <png-path> [<tweak>...]
       plex-rust snapshot signin <png-path>
       plex-rust snapshot play <rating-key> <png-path> [<key>...]
         tweaks: narrow, hover, link, season, scroll=<points>,
                 across=<points>, unwatched,
                 sort=title|added|released|rating|watched
       plex-rust login
       plex-rust servers [<address>]
       plex-rust api [--post] <path>
       plex-rust play <file-or-url> [--unattended <seconds>]
       plex-rust play item:<rating-key> [--unattended <seconds>]
       plex-rust play <file-url-or-item> --seeks <seconds>x<presses>[@<ms>],...
       plex-rust pacing <file-or-url-or-item:<rating-key>> <seconds> <speed>...";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("plex-rust: {}", error::with_causes(&error));
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[&str]) -> Result<()> {
    match args {
        [] => ui::shell::run(None),
        ["snapshot", "home", path, tweaks @ ..] => snapshot(Subject::Home, path, tweaks),
        ["snapshot", "library", index, path, tweaks @ ..] => {
            let index = index.parse().map_err(|_| Error::Usage(USAGE))?;
            snapshot(Subject::Library(index), path, tweaks)
        }
        ["snapshot", "item", key, path, tweaks @ ..] => {
            snapshot(Subject::Item((*key).to_owned()), path, tweaks)
        }
        ["snapshot", "search", query, path, tweaks @ ..] => {
            snapshot(Subject::Search((*query).to_owned()), path, tweaks)
        }
        ["snapshot", "list", list, path, tweaks @ ..] => {
            snapshot(Subject::List((*list).to_owned()), path, tweaks)
        }
        ["snapshot", "signin", path] => snapshot(Subject::SignIn, path, &[]),
        ["snapshot", "play", key, path, keys @ ..] => {
            let keys: Option<Vec<Key>> = keys.iter().map(|name| Key::named(name)).collect();
            let keys = keys.ok_or(Error::Usage(USAGE))?;
            snapshot(Subject::Play((*key).to_owned(), keys), path, &[])
        }
        ["login"] => account::login(),
        ["servers"] => servers(None),
        ["servers", address] => servers(Some(address)),
        ["api", path] => api(plex::Method::GET, path),
        ["api", "--post", path] => api(plex::Method::POST, path),
        ["play", what] => play(what, None),
        ["play", what, "--unattended", seconds] => {
            let seconds = seconds.parse().map_err(|_| Error::Usage(USAGE))?;
            play(what, Some(Check::State(Duration::from_secs(seconds))))
        }
        ["play", what, "--seeks", script] => {
            let steps = seeks::parse(script).ok_or(Error::Usage(USAGE))?;
            play(what, Some(Check::Seeks(steps)))
        }
        ["pacing", what, seconds, speeds @ ..] => pacing::run(what, seconds, speeds),
        _ => Err(Error::Usage(USAGE)),
    }
}

/// Draws `subject` into a PNG at `path`, in the state the tweaks describe.
fn snapshot(subject: Subject, path: &str, tweaks: &[&str]) -> Result<()> {
    let mut parsed = Tweaks::default();
    for tweak in tweaks {
        match tweak.split_once('=') {
            None if *tweak == "narrow" => parsed.narrow = true,
            None if *tweak == "hover" => parsed.hover = true,
            None if *tweak == "link" => parsed.follow = Some(Link::Show),
            None if *tweak == "season" => parsed.follow = Some(Link::Season),
            None if *tweak == "unwatched" => parsed.listing.unwatched_only = true,
            Some(("scroll", points)) => {
                parsed.scroll = points.parse().map_err(|_| Error::Usage(USAGE))?;
            }
            Some(("across", points)) => {
                parsed.across = points.parse().map_err(|_| Error::Usage(USAGE))?;
            }
            Some(("sort", "title")) => parsed.listing.sort = Sort::Title,
            Some(("sort", "added")) => parsed.listing.sort = Sort::DateAdded,
            Some(("sort", "released")) => parsed.listing.sort = Sort::ReleaseDate,
            Some(("sort", "rating")) => parsed.listing.sort = Sort::Rating,
            Some(("sort", "watched")) => parsed.listing.sort = Sort::LastWatched,
            _ => return Err(Error::Usage(USAGE)),
        }
    }
    ui::shell::run(Some(Snapshot { subject, tweaks: parsed, path: path.into() }))
}

/// Plays a file, a URL, or with `item:<rating-key>` an item from the first
/// server, resuming where it was left and reporting progress.
fn play(what: &str, check: Option<Check>) -> Result<()> {
    let Some(key) = what.strip_prefix("item:") else {
        return Ok(play::run(&Source::new(what), None, check)?);
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

    let source = source::of(&server, &item)?;
    // A check must not move anyone's place in a film, so it tells the
    // server nothing.
    let progress = check.is_none().then(|| Progress::new(runtime, server, item.rating_key));
    Ok(play::run(&source, progress, check)?)
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
