use super::*;
use crate::watch::FileStamp;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::Duration;

pub(super) type Snapshot = HashMap<OsString, FileStamp>;

// Read file identity and times off the foreground thread. Markdown bodies and images
// are only opened by the repository when refreshing a changed inbox.
pub(super) fn snapshot(path: &Path) -> std::io::Result<Snapshot> {
    let mut files = HashMap::new();
    for item in fs::read_dir(path)? {
        let item = item?;
        let name = item.file_name();
        if !crate::library::is_inbox_entry(Path::new(&name)) {
            continue;
        }
        // symlink_metadata also handles editors replacing the file during this scan.
        match fs::symlink_metadata(item.path()) {
            Ok(m) if m.is_file() => {
                files.insert(name, FileStamp::from_metadata(&m));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(files)
}

impl LibraryView {
    pub(super) fn observe_files(&mut self, cx: &mut Context<Self>) {
        if self.file_watch.is_some() {
            return;
        }
        let Some(database) = &self.database else {
            return;
        };
        let root = database.inbox_path().to_owned();
        let mut known = self.file_snapshot.take();
        let (changes, receiver) = async_channel::bounded(1);
        let executor = cx.background_executor().clone();
        cx.background_executor()
            .spawn(async move {
                let mut candidate = None;
                loop {
                    executor.timer(Duration::from_millis(750)).await;
                    if changes.is_closed() {
                        break;
                    }
                    let Ok(next) = snapshot(&root) else { continue };
                    if known.as_ref() == Some(&next) {
                        candidate = None;
                        continue;
                    }
                    if candidate.as_ref() != Some(&next) {
                        candidate = Some(next);
                        continue;
                    }
                    known = Some(next);
                    candidate = None;
                    // A full channel already requests the latest version of the folder.
                    let _ = changes.try_send(());
                }
            })
            .detach();
        self.file_watch = Some(cx.spawn(async move |this, cx| {
            while receiver.recv().await.is_ok() {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    pub fn leave(&mut self, cx: &mut Context<Self>) {
        self.file_watch = None;
        self.all_saved(cx);
    }

    pub fn open_inbox(&mut self, cx: &mut Context<Self>) {
        self.flush_notes(cx);
        let path = crate::library::inbox_path();
        let job = cx.background_executor().spawn(async move {
            let path = path?;
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            Ok::<_, String>(path)
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(path) => cx.open_with_system(&path),
                Err(message) => {
                    this.message = Some(message);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn open_markdown(&mut self, id: i64, cx: &mut Context<Self>) {
        self.flush_notes(cx);
        if let Some(entry) = self.entries.iter().find(|e| e.item.id == id) {
            cx.open_with_system(&entry.item.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::snapshot;
    use std::fs;
    #[test]
    fn detects_changed_markdown_and_ignores_hidden_files_assets_and_links() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let initial = snapshot(root).unwrap();
        fs::write(root.join(".swap.md"), "ignored").unwrap();
        fs::write(root.join("note.md~"), "ignored").unwrap();
        fs::create_dir(root.join("assets")).unwrap();
        fs::write(root.join("assets/image.md"), "ignored").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join(".swap.md"), root.join("link.md")).unwrap();
        assert_eq!(snapshot(root).unwrap(), initial);
        fs::write(root.join("Note.MARKDOWN"), "first").unwrap();
        let saved = snapshot(root).unwrap();
        assert_eq!(saved.len(), 1);
        fs::write(root.join("Note.MARKDOWN"), "changed").unwrap();
        assert_ne!(snapshot(root).unwrap(), saved);
        fs::remove_file(root.join("Note.MARKDOWN")).unwrap();
        assert_eq!(snapshot(root).unwrap(), initial);
    }
}
