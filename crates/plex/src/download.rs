//! Fetching the original file of an item to keep, in one go or carrying on
//! from where an earlier attempt stopped.

use std::io::ErrorKind;
use std::path::Path;
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::future::{Either, select};
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::{Method, Response, StatusCode};
use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;

use crate::error::{Error, Result};
use crate::model::Part;
use crate::server::MediaServer;

/// How long one download may take. The client's own limit of half a minute
/// is for answers, not for files of tens of gigabytes.
const LIMIT: Duration = Duration::from_secs(24 * 60 * 60);

/// How long the server may send nothing before the download gives up.
const STALL: Duration = Duration::from_secs(60);

/// What a download shares with whoever started it: how far it has got, and
/// the word to stop.
#[derive(Debug, Default)]
pub struct Transfer {
    received: AtomicU64,
    cancelled: Notify,
}

impl Transfer {
    /// How many bytes of the file are on disk.
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// Stops the download, which then fails with [`Error::Cancelled`]. What
    /// it wrote stays on disk.
    pub fn cancel(&self) {
        self.cancelled.notify_one();
    }
}

/// What is left to do for a file.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// Ask the server for the file from this byte on.
    Fetch { from: u64 },
    /// Every byte is there.
    Done,
}

/// What to do next for a file of `size` bytes with `on_disk` of them
/// written. More bytes than the file has cannot be put right by fetching.
fn next_step(on_disk: u64, size: u64) -> Result<Step> {
    match on_disk.cmp(&size) {
        std::cmp::Ordering::Less => Ok(Step::Fetch { from: on_disk }),
        std::cmp::Ordering::Equal => Ok(Step::Done),
        std::cmp::Ordering::Greater => Err(Error::SizeMismatch { expected: size, found: on_disk }),
    }
}

/// The first byte and the whole length in a `Content-Range` header such as
/// `bytes 100-999/1000`.
fn content_range(value: &str) -> Option<(u64, u64)> {
    let (range, length) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (first, _) = range.split_once('-')?;
    Some((first.parse().ok()?, length.parse().ok()?))
}

/// Where in the file the body of an answer begins, after asking for a file
/// of `size` bytes from byte `asked` on.
///
/// A server is free to ignore the range and send the whole file, which
/// then begins at 0. An answer that begins anywhere else than was asked,
/// or that is for a file of another length, is an error: writing it would
/// leave a file that is not the original.
fn body_start(
    status: StatusCode,
    range: Option<&str>,
    length: Option<u64>,
    asked: u64,
    size: u64,
) -> Result<u64> {
    let unexpected = || Error::Resume {
        asked,
        answer: format!("{status} with Content-Range {}", range.unwrap_or("missing")),
    };
    let (start, length) = match status {
        StatusCode::OK => (0, length),
        StatusCode::PARTIAL_CONTENT => {
            let (first, length) = range.and_then(content_range).ok_or_else(unexpected)?;
            (first, Some(length))
        }
        _ => return Err(unexpected()),
    };
    if start != 0 && start != asked {
        return Err(unexpected());
    }
    match length {
        Some(length) if length != size => {
            Err(Error::SizeMismatch { expected: size, found: length })
        }
        _ => Ok(start),
    }
}

impl MediaServer {
    /// Fetches the original file of `part` into the file at `partial`,
    /// after whatever an earlier attempt left in it, and returns once every
    /// byte is there. `transfer` hears how far it has got.
    ///
    /// Nothing is tried twice. An attempt that fails leaves what it wrote,
    /// for the next call to carry on from. A file that ends up with another
    /// number of bytes than the server gave for it fails with
    /// [`Error::SizeMismatch`].
    pub async fn download(&self, part: &Part, partial: &Path, transfer: &Transfer) -> Result<()> {
        let fetch = pin!(self.fetch(part, partial, transfer));
        let stop = pin!(transfer.cancelled.notified());
        match select(fetch, stop).await {
            Either::Left((fetched, _)) => fetched,
            Either::Right(((), _)) => Err(Error::Cancelled),
        }
    }

    async fn fetch(&self, part: &Part, partial: &Path, transfer: &Transfer) -> Result<()> {
        let file_error = |source| Error::File { path: partial.to_owned(), source };
        let size = part.size.ok_or_else(|| Error::NoFile(part.key.clone()))?;
        let on_disk = match tokio::fs::metadata(partial).await {
            Ok(file) => file.len(),
            Err(error) if error.kind() == ErrorKind::NotFound => 0,
            Err(error) => return Err(file_error(error)),
        };
        transfer.received.store(on_disk, Ordering::Relaxed);
        let Step::Fetch { from } = next_step(on_disk, size)? else {
            return Ok(());
        };

        let mut response = self.original(part, from).await?;
        let range = response.headers().get(CONTENT_RANGE).and_then(|range| range.to_str().ok());
        let start = body_start(response.status(), range, response.content_length(), from, size)?;

        let mut file =
            OpenOptions::new().create(true).append(true).open(partial).await.map_err(file_error)?;
        // Drops what is there when the server sends the file from its start.
        file.set_len(start).await.map_err(file_error)?;
        let written = write_body(&mut response, &mut file, partial, start, size, transfer).await;
        // A write may still be on its way to the disk. It has to land
        // before anyone measures the file to carry on from its end.
        let flushed = file.flush().await.map_err(file_error);
        let received = written?;
        flushed?;
        match next_step(received, size)? {
            Step::Done => Ok(()),
            Step::Fetch { .. } => Err(Error::SizeMismatch { expected: size, found: received }),
        }
    }

    /// Asks for the original file of `part` from byte `from` on.
    ///
    /// With `download=1`, as Plex's API description has it for a file to
    /// keep. An account can be denied downloads, so a server that refuses
    /// is asked again the way a player asks.
    async fn original(&self, part: &Part, from: u64) -> Result<Response> {
        let ask = |query: &[(&str, &str)]| {
            self.request(Method::GET, &part.key)
                .query(query)
                .header(RANGE, format!("bytes={from}-"))
                .timeout(LIMIT)
                .send()
        };
        let response = ask(&[("download", "1")]).await?;
        let response = match response.status().is_client_error() {
            true => ask(&[]).await?,
            false => response,
        };
        Ok(response.error_for_status()?)
    }
}

/// Appends the body of `response` to `file`, which holds `start` bytes,
/// and returns how many it holds then.
async fn write_body(
    response: &mut Response,
    file: &mut File,
    path: &Path,
    start: u64,
    size: u64,
    transfer: &Transfer,
) -> Result<u64> {
    let mut received = start;
    transfer.received.store(received, Ordering::Relaxed);
    loop {
        let chunk = tokio::time::timeout(STALL, response.chunk()).await;
        let Some(chunk) = chunk.map_err(|_| Error::Stalled)?? else {
            return Ok(received);
        };
        // A body that does not state its length could go on for ever.
        if received + chunk.len() as u64 > size {
            let found = received + chunk.len() as u64;
            return Err(Error::SizeMismatch { expected: size, found });
        }
        file.write_all(&chunk)
            .await
            .map_err(|source| Error::File { path: path.to_owned(), source })?;
        received += chunk.len() as u64;
        transfer.received.store(received, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    use super::*;
    use crate::client::{Client, Identity, Token};

    #[test]
    fn a_download_carries_on_from_the_bytes_on_disk() {
        assert_eq!(next_step(0, 1000).unwrap(), Step::Fetch { from: 0 });
        assert_eq!(next_step(400, 1000).unwrap(), Step::Fetch { from: 400 });
        assert_eq!(next_step(1000, 1000).unwrap(), Step::Done);
        // Too many bytes: fetching more cannot make that the original.
        let too_many = next_step(1001, 1000).unwrap_err();
        assert!(matches!(too_many, Error::SizeMismatch { expected: 1000, found: 1001 }));
    }

    #[test]
    fn the_body_begins_where_the_server_says_it_does() {
        let start = |status: u16, range: Option<&str>, length: Option<u64>| {
            body_start(StatusCode::from_u16(status).unwrap(), range, length, 400, 1000)
        };
        assert_eq!(start(206, Some("bytes 400-999/1000"), Some(600)).unwrap(), 400);
        // The range was ignored: the whole file comes, from its start.
        assert_eq!(start(200, None, Some(1000)).unwrap(), 0);
        assert_eq!(start(200, None, None).unwrap(), 0);

        // From somewhere that was not asked for.
        let elsewhere = |result| matches!(result, Err(Error::Resume { asked: 400, .. }));
        assert!(elsewhere(start(206, Some("bytes 500-999/1000"), None)));
        assert!(elsewhere(start(206, None, Some(600))));
        assert!(elsewhere(start(206, Some("items 4-9/10"), None)));
        // A file of another length than the one that was begun.
        let other = |result| matches!(result, Err(Error::SizeMismatch { expected: 1000, .. }));
        assert!(other(start(206, Some("bytes 400-1099/1100"), Some(700))));
        assert!(other(start(200, None, Some(900))));
    }

    /// What a stand-in server saw of one request.
    #[derive(Debug, Clone, PartialEq)]
    struct Asked {
        /// The path and query.
        target: String,
        /// The first byte asked for.
        from: usize,
    }

    /// Answers every request on a local port with what `answer` returns:
    /// the status line with the headers, and the body. The connection
    /// closes after each answer, which is how a download is cut short
    /// here. Returns the server and the requests as they arrive.
    async fn serve(
        answer: impl Fn(&Asked) -> (String, Vec<u8>) + Send + 'static,
    ) -> (MediaServer, Arc<Mutex<Vec<Asked>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    head.push(socket.read_u8().await.unwrap());
                }
                let head = String::from_utf8(head).unwrap().to_lowercase();
                let target = head.split(' ').nth(1).unwrap().to_owned();
                let range = head.lines().find_map(|line| line.strip_prefix("range: bytes="));
                let from = range.unwrap().trim_end_matches('-').parse().unwrap();
                let asked = Asked { target, from };
                let (head, body) = answer(&asked);
                log.lock().unwrap().push(asked);
                socket.write_all(format!("HTTP/1.1 {head}\r\n\r\n").as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            }
        });
        let identity = Identity { client_identifier: "test".into(), device_name: "test".into() };
        let client = Client::new(identity).unwrap();
        (MediaServer::at(&client, address, Token::new("token")), seen)
    }

    fn part(size: usize) -> Part {
        serde_json::from_value(serde_json::json!({
            "id": 1, "key": "/library/parts/1/2/file.mkv", "size": size,
        }))
        .unwrap()
    }

    /// A place for a partial file that no other test uses.
    fn partial(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("plex-rust-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn original() -> Vec<u8> {
        (0..100_000u32).flat_map(u32::to_le_bytes).collect()
    }

    #[tokio::test]
    async fn a_download_cut_short_is_finished_by_the_next_attempt() {
        const CUT: usize = 150_000;
        let (file, path) = (original(), partial("cut"));
        let whole = file.clone();
        let (server, seen) = serve(move |asked| {
            let rest = &whole[asked.from..];
            let (length, last) = (rest.len(), whole.len() - 1);
            let range = format!("bytes {}-{last}/{}", asked.from, whole.len());
            let head = format!(
                "206 Partial Content\r\nContent-Length: {length}\r\nContent-Range: {range}"
            );
            // The first answer promises the rest of the file and stops early.
            let sent = if asked.from == 0 { &rest[..CUT] } else { rest };
            (head, sent.to_vec())
        })
        .await;

        let (part, transfer) = (part(file.len()), Transfer::default());
        assert!(server.download(&part, &path, &transfer).await.is_err());
        let kept = usize::try_from(std::fs::metadata(&path).unwrap().len()).unwrap();
        assert!(kept > 0 && kept <= CUT);
        assert_eq!(transfer.received(), kept as u64);

        server.download(&part, &path, &transfer).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), file);
        assert_eq!(transfer.received(), file.len() as u64);
        let asked: Vec<usize> = seen.lock().unwrap().iter().map(|asked| asked.from).collect();
        assert_eq!(asked, [0, kept]);

        // With every byte there, the server is not asked again.
        server.download(&part, &path, &transfer).await.unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2);
        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn a_server_that_sends_the_whole_file_again_replaces_what_was_there() {
        let (file, path) = (original(), partial("whole"));
        std::fs::write(&path, b"left by an earlier attempt").unwrap();
        let whole = file.clone();
        let (server, _) =
            serve(move |_| (format!("200 OK\r\nContent-Length: {}", whole.len()), whole.clone()))
                .await;

        server.download(&part(file.len()), &path, &Transfer::default()).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), file);
        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn a_file_of_another_size_is_an_error_and_is_not_asked_for_again() {
        let path = partial("size");
        let (server, seen) = serve(|_| {
            let head = "206 Partial Content\r\nContent-Length: 5\r\nContent-Range: bytes 0-4/5";
            (head.to_owned(), b"short".to_vec())
        })
        .await;

        let failed = server.download(&part(9), &path, &Transfer::default()).await.unwrap_err();
        assert!(matches!(failed, Error::SizeMismatch { expected: 9, found: 5 }));
        assert_eq!(seen.lock().unwrap().len(), 1);
        // Nothing of the wrong file was written.
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_server_that_refuses_downloads_is_asked_as_a_player_asks() {
        let path = partial("refused");
        let (server, seen) = serve(|asked| match asked.target.contains("download=1") {
            true => ("403 Forbidden\r\nContent-Length: 0".to_owned(), Vec::new()),
            false => ("200 OK\r\nContent-Length: 4".to_owned(), b"file".to_vec()),
        })
        .await;

        server.download(&part(4), &path, &Transfer::default()).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"file");
        let targets: Vec<String> =
            seen.lock().unwrap().iter().map(|asked| asked.target.clone()).collect();
        let plain = "/library/parts/1/2/file.mkv";
        assert_eq!(targets, [format!("{plain}?download=1"), plain.to_owned()]);
        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn a_cancelled_download_fails_as_cancelled() {
        let path = partial("cancelled");
        let (server, _) =
            serve(|_| ("200 OK\r\nContent-Length: 4".to_owned(), b"file".to_vec())).await;

        let transfer = Transfer::default();
        transfer.cancel();
        let stopped = server.download(&part(4), &path, &transfer).await.unwrap_err();
        assert!(matches!(stopped, Error::Cancelled));
    }
}
