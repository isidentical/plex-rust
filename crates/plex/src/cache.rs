//! What a server said, kept on disk between launches.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// A folder of files, each the answer to one question, that is kept under
/// a size by deleting the files that went unused the longest.
///
/// A cache answers for one scope: one server as seen by one account. Two
/// scopes can share a folder and its size and never read each other's
/// files. Every call does its disk work off the calling thread.
///
/// A file is named by a hash of its scope and question, and two of those
/// can share a hash. So each file starts with the scope and question it
/// answers, and one found under the hash of another is not read as its
/// answer.
#[derive(Debug, Clone)]
pub struct Cache(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    directory: PathBuf,
    scope: String,
    limit: u64,
    /// Bytes written since the folder was last brought under its limit.
    unswept: AtomicU64,
}

/// A file as the sweep sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Kept {
    path: PathBuf,
    size: u64,
    used: SystemTime,
}

/// Keeps temporary names apart within the process.
static WRITES: AtomicU64 = AtomicU64::new(0);

impl Cache {
    /// A cache in `directory`, which must exist, for the answers of
    /// `scope`, holding about `limit` bytes with everything else in the
    /// folder.
    pub fn new(directory: PathBuf, scope: &str, limit: u64) -> Self {
        // As if a whole limit had been written, so that the first write
        // sweeps a folder left over the limit by an earlier launch.
        let unswept = AtomicU64::new(limit);
        Self(Arc::new(Inner { directory, scope: scope.to_owned(), limit, unswept }))
    }

    /// The answer kept for `key`, if there is one. Reading it counts as
    /// using it.
    pub async fn read(&self, key: &str) -> Option<Vec<u8>> {
        let (name, path) = (self.name(key), self.path(key));
        let file = blocking(move || {
            let file = fs::read(&path)?;
            // A file whose time cannot be set is only deleted sooner.
            let _ = File::open(&path).and_then(|file| file.set_modified(SystemTime::now()));
            Ok(file)
        });
        opened(&name, file.await.ok()?)
    }

    /// Keeps `contents` as the answer for `key`. A reader sees the old
    /// answer or the new one, never part of one: the file is written
    /// under another name and then renamed.
    pub async fn write(&self, key: &str, contents: Vec<u8>) -> io::Result<()> {
        let (path, inner) = (self.path(key), Arc::clone(&self.0));
        let file = sealed(&self.name(key), &contents);
        blocking(move || {
            let written = u64::try_from(file.len()).unwrap_or(u64::MAX);
            let unique = WRITES.fetch_add(1, Ordering::Relaxed);
            let temporary = path.with_extension(format!("{}-{unique}.part", std::process::id()));
            fs::write(&temporary, file)?;
            fs::rename(&temporary, &path)?;

            let unswept = inner.unswept.fetch_add(written, Ordering::Relaxed) + written;
            if unswept >= inner.limit / SWEEPS_PER_LIMIT {
                inner.unswept.store(0, Ordering::Relaxed);
                sweep(&inner.directory, inner.limit)?;
            }
            Ok(())
        })
        .await
    }

    /// Forgets the answer for `key`.
    pub async fn remove(&self, key: &str) -> io::Result<()> {
        let path = self.path(key);
        blocking(move || fs::remove_file(path)).await
    }

    /// What sets the answer for `key` apart from every other in the
    /// folder: the scope and the question.
    fn name(&self, key: &str) -> String {
        format!("{}\n{key}", self.0.scope)
    }

    fn path(&self, key: &str) -> PathBuf {
        self.0.directory.join(format!("{:016x}", stable_hash(&self.name(key))))
    }
}

/// How many bytes at the start of a file say how long the name after
/// them is.
const NAME_LENGTH_BYTES: usize = 4;

/// `contents` as a file holds them: after the name they are kept under
/// and, before that, how many bytes long the name is.
fn sealed(name: &str, contents: &[u8]) -> Vec<u8> {
    // A name is a few hundred bytes. One too long to count is cut short
    // here and so never matches when read, which only costs the answer.
    let length = u32::try_from(name.len()).unwrap_or(u32::MAX);
    let mut file = Vec::with_capacity(NAME_LENGTH_BYTES + name.len() + contents.len());
    file.extend_from_slice(&length.to_le_bytes());
    file.extend_from_slice(name.as_bytes());
    file.extend_from_slice(contents);
    file
}

/// The contents of `file` if it is the one kept under `name`. `None` for
/// the file of another name with the same hash, and for one that does not
/// start with a name at all, as those of earlier versions did not.
fn opened(name: &str, mut file: Vec<u8>) -> Option<Vec<u8>> {
    let length = file.first_chunk::<NAME_LENGTH_BYTES>().copied().map(u32::from_le_bytes)?;
    let start = NAME_LENGTH_BYTES.checked_add(usize::try_from(length).ok()?)?;
    if file.get(NAME_LENGTH_BYTES..start)? != name.as_bytes() {
        return None;
    }
    file.drain(..start);
    Some(file)
}

/// The folder is swept this many times for each limit's worth of bytes
/// written to it, so it outgrows its limit by at most one part in this.
const SWEEPS_PER_LIMIT: u64 = 8;

/// Runs disk work on the threads tokio keeps for work that blocks.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(work).await.map_err(io::Error::other)?
}

/// Deletes the least recently used files of `directory` if it holds more
/// than `limit` bytes.
fn sweep(directory: &Path, limit: u64) -> io::Result<()> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            files.push(Kept {
                path: entry.path(),
                size: metadata.len(),
                used: metadata.modified()?,
            });
        }
    }
    for path in excess(files, limit) {
        // Another launch of the app may have deleted it first.
        let _ = fs::remove_file(path);
    }
    Ok(())
}

/// Which of `files` to delete so that the rest fit in `limit` bytes with a
/// quarter of it to spare: the ones used longest ago. None if they fit as
/// they are. The spare room keeps a full folder from being swept on every
/// write.
fn excess(mut files: Vec<Kept>, limit: u64) -> Vec<PathBuf> {
    let mut total: u64 = files.iter().map(|file| file.size).sum();
    if total <= limit {
        return Vec::new();
    }
    files.sort_by_key(|file| file.used);
    let enough = limit - limit / 4;
    let mut deleted = Vec::new();
    for file in files {
        if total <= enough {
            break;
        }
        total -= file.size;
        deleted.push(file.path);
    }
    deleted
}

/// FNV-1a. Unlike the standard library's hasher it gives the same value in
/// every build, which a name for a file on disk needs.
fn stable_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn run<T>(work: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(work)
    }

    fn kept(name: &str, size: u64, used_at: u64) -> Kept {
        let used = SystemTime::UNIX_EPOCH + Duration::from_secs(used_at);
        Kept { path: PathBuf::from(name), size, used }
    }

    #[test]
    fn the_files_used_longest_ago_are_deleted_first() {
        let files = vec![kept("new", 40, 30), kept("old", 40, 10), kept("middle", 40, 20)];

        assert!(excess(files.clone(), 120).is_empty());
        // Over 100, so down to 75: one file is not enough, two are.
        let deleted = excess(files, 100);
        assert_eq!(deleted, [PathBuf::from("old"), PathBuf::from("middle")]);
    }

    #[test]
    fn an_answer_is_kept_for_its_own_scope_only() {
        let folder = tempfile::tempdir().unwrap();
        let cache = |scope| Cache::new(folder.path().to_owned(), scope, 1 << 20);
        let (mine, theirs) = (cache("account-a\nserver"), cache("account-b\nserver"));
        run(async {
            mine.write("/hubs", b"mine".to_vec()).await.unwrap();
            assert_eq!(mine.read("/hubs").await.as_deref(), Some(&b"mine"[..]));
            assert_eq!(theirs.read("/hubs").await, None);

            mine.write("/hubs", b"newer".to_vec()).await.unwrap();
            assert_eq!(mine.read("/hubs").await.as_deref(), Some(&b"newer"[..]));
            mine.remove("/hubs").await.unwrap();
            assert_eq!(mine.read("/hubs").await, None);
        });
        // Nothing is left behind under a temporary name.
        assert_eq!(fs::read_dir(folder.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_file_found_under_the_hash_of_another_question_is_not_its_answer() {
        let folder = tempfile::tempdir().unwrap();
        let cache = Cache::new(folder.path().to_owned(), "scope", 1 << 20);
        run(async {
            cache.write("/library/metadata/1", b"one".to_vec()).await.unwrap();
            // As if the two questions had the same hash.
            fs::copy(cache.path("/library/metadata/1"), cache.path("/library/metadata/2")).unwrap();
            assert_eq!(cache.read("/library/metadata/1").await.as_deref(), Some(&b"one"[..]));
            assert_eq!(cache.read("/library/metadata/2").await, None);

            // What an earlier version kept has no name in front of it.
            fs::write(cache.path("/hubs"), br#"{"MediaContainer":{}}"#).unwrap();
            assert_eq!(cache.read("/hubs").await, None);
            fs::write(cache.path("/hubs"), b"").unwrap();
            assert_eq!(cache.read("/hubs").await, None);
        });
    }

    #[test]
    fn a_full_folder_loses_what_was_not_read_lately() {
        let folder = tempfile::tempdir().unwrap();
        // Each file is its 300 bytes and, before them, 4 for the length
        // of its name and 7 for the name: 311.
        let cache = Cache::new(folder.path().to_owned(), "scope", 1040);
        let long_ago = |key: &str, seconds: u64| {
            let file = File::options().write(true).open(cache.path(key)).unwrap();
            file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)).unwrap();
        };
        run(async {
            for key in ["a", "b", "c"] {
                cache.write(key, vec![0; 300]).await.unwrap();
            }
            long_ago("a", 10);
            long_ago("b", 20);
            long_ago("c", 30);
            // Reading the oldest makes it the most recently used.
            assert!(cache.read("a").await.is_some());

            // 1244 bytes are over the limit: down to 780, oldest first.
            cache.write("d", vec![0; 300]).await.unwrap();
            assert!(cache.read("a").await.is_some());
            assert!(cache.read("b").await.is_none());
            assert!(cache.read("c").await.is_none());
            assert!(cache.read("d").await.is_some());
        });
    }
}
