use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use async_channel::{Sender, TrySendError};

const POLL_INTERVAL: Duration = Duration::from_millis(120);
const SETTLE_TIME: Duration = Duration::from_millis(360);

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64),
    #[cfg(unix)]
    changed: (i64, i64),
}

impl FileStamp {
    fn read(path: &Path) -> Result<Self, ErrorKind> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        let metadata = fs::metadata(path).map_err(|e| e.kind())?;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            #[cfg(unix)]
            identity: (metadata.dev(), metadata.ino()),
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

type Observation = Result<FileStamp, ErrorKind>;

struct Changes {
    reported: Observation,
    observed: Observation,
    since: Instant,
}

impl Changes {
    fn new(initial: Observation, now: Instant) -> Self {
        Self {
            reported: initial.clone(),
            observed: initial,
            since: now,
        }
    }

    fn observe(&mut self, value: Observation, now: Instant) -> bool {
        if self.observed != value {
            self.observed = value;
            self.since = now;
        }
        if self.observed != self.reported && now.duration_since(self.since) >= SETTLE_TIME {
            self.reported = self.observed.clone();
            return true;
        }
        false
    }
}

/// Run on a dedicated worker. Poll the pathname so editor saves that replace its inode
/// and sources that disappear and return are both visible. Closing the receiver cancels it.
pub fn watch(path: PathBuf, tx: Sender<()>) {
    let mut changes = Changes::new(FileStamp::read(&path), Instant::now());
    while !tx.is_closed() {
        thread::sleep(POLL_INTERVAL);
        if changes.observe(FileStamp::read(&path), Instant::now())
            && let Err(TrySendError::Closed(())) = tx.try_send(())
        {
            return;
        }
        // A full channel already contains a request to reread the current path.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_stable_edits_replacements_removal_and_recreation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.csv");
        fs::write(&path, "a\n1\n").unwrap();
        let mut now = Instant::now();
        let mut changes = Changes::new(FileStamp::read(&path), now);
        let mut settled = |path: &Path| {
            now += POLL_INTERVAL;
            let stamp = FileStamp::read(path);
            assert!(!changes.observe(stamp.clone(), now));
            now += SETTLE_TIME;
            assert!(changes.observe(stamp.clone(), now));
            assert!(!changes.observe(stamp, now));
        };
        fs::write(&path, "a\n11\n").unwrap();
        settled(&path);
        let replacement = dir.path().join("replacement.csv");
        fs::write(&replacement, "a\n22\n").unwrap();
        // Preserve mtime and length across replacement, just as some writers do.
        let modified = path.metadata().unwrap().modified().unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        fs::rename(&replacement, &path).unwrap();
        settled(&path);
        fs::remove_file(&path).unwrap();
        settled(&path);
        fs::write(&path, "a\n3\n").unwrap();
        settled(&path);
    }

    #[test]
    fn a_second_write_restarts_the_settling_period() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path();
        let now = Instant::now();
        let mut changes = Changes::new(FileStamp::read(path), now);
        fs::write(path, "incomplete").unwrap();
        assert!(!changes.observe(FileStamp::read(path), now));
        fs::write(path, "complete").unwrap();
        let later = now + SETTLE_TIME;
        assert!(!changes.observe(FileStamp::read(path), later));
        assert!(!changes.observe(FileStamp::read(path), later + POLL_INTERVAL));
        assert!(changes.observe(FileStamp::read(path), later + SETTLE_TIME));
    }
}
