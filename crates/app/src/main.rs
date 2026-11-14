//! Reel, a native macOS client for Plex.

mod account;
mod error;
mod play;
mod video_layer;

use std::process::ExitCode;
use std::time::Duration;

use crate::error::Result;

const USAGE: &str = "\
usage: reel login
       reel servers [<address>]
       reel api <path>
       reel play <file-or-url> [--report-after <seconds>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("reel: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[&str]) -> Result<()> {
    match args {
        ["login"] => account::login(),
        ["servers"] => servers(None),
        ["servers", address] => servers(Some(address)),
        ["api", path] => api(path),
        ["play", source] => Ok(play::run(source, None)?),
        ["play", source, "--report-after", seconds] => {
            let seconds = seconds.parse().map_err(|_| error::Error::Usage(USAGE))?;
            Ok(play::run(source, Some(Duration::from_secs(seconds)))?)
        }
        _ => Err(error::Error::Usage(USAGE)),
    }
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

/// Prints the first server's raw response for `path`, such as `/hubs`.
fn api(path: &str) -> Result<()> {
    let (client, token) = account::signed_in()?;
    runtime()?.block_on(async {
        let servers = plex::discovery::servers(&client, &token).await?;
        let server = servers.first().ok_or(error::Error::NoServer)?;
        let server = plex::MediaServer::connect(&client, server, None).await?;
        println!("{}", server.get_text(path).await?);
        Ok(())
    })
}

/// A runtime for commands that make a few requests and exit.
fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(error::Error::Runtime)
}
