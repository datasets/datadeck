mod discovery;
mod files;
pub(crate) use files::markdown as is_inbox_entry;
mod migration;
mod net;
mod preview;

use chrono::Utc;
use files::{Document, Header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[serde(alias = "Dataset")]
    Dataset,
    #[serde(alias = "Story")]
    Story,
    #[serde(alias = "Visualization")]
    Visualization,
    #[default]
    #[serde(alias = "Other")]
    Other,
}

impl Kind {
    pub const ALL: [Self; 4] = [Self::Dataset, Self::Story, Self::Visualization, Self::Other];
    pub fn label(self) -> &'static str {
        match self {
            Self::Dataset => "Dataset",
            Self::Story => "Story",
            Self::Visualization => "Visualization",
            Self::Other => "Other",
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct BookmarkDraft {
    pub url: String,
    pub title: String,
    pub notes: String,
    pub kind: Kind,
    pub files: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FoundLink {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct Bookmark {
    pub path: PathBuf,
    pub id: i64,
    pub revision: i64,
    pub notes_revision: i64,
    pub captured_at: Option<i64>,
    pub capture_label: String,
    pub url: String,
    pub title: String,
    pub notes: String,
    pub kind: Kind,
    pub files: Vec<String>,
    pub image: Option<Vec<u8>>,
    pub images: Vec<FoundLink>,
    pub visualizations: Vec<FoundLink>,
}

#[derive(Clone)]
pub struct Library {
    path: PathBuf,
    recovery_session: String,
    paths: Arc<Mutex<HashMap<i64, PathBuf>>>,
}

fn message(err: impl std::fmt::Display) -> String {
    err.to_string()
}

fn http_url(input: &str) -> Result<Url, String> {
    let url = Url::parse(input.trim()).map_err(|_| "Enter a complete http:// or https:// URL.")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Use an HTTP or HTTPS URL without embedded credentials.".into());
    }
    Ok(url)
}

fn dataset_url(url: &Url) -> bool {
    dataset_name(url.path())
}

fn dataset_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|v| v.to_str())
        .is_some_and(|ext| {
            ["csv", "tsv", "xls", "xlsx", "parquet"]
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        })
}

fn file_url(mut url: Url) -> String {
    url.set_fragment(None);
    url.into()
}

fn normalized(draft: BookmarkDraft) -> Result<BookmarkDraft, String> {
    let url = http_url(&draft.url)?;
    let mut files = vec![];
    for file in &draft.files {
        let file = file.trim();
        if file.is_empty() {
            continue;
        }
        let source =
            if Path::new(file).is_absolute() {
                file.to_owned()
            } else {
                file_url(http_url(file).map_err(
                    |_| "A linked file must be an HTTP(S) URL or an absolute local path.",
                )?)
            };
        if !files.contains(&source) {
            files.push(source);
        }
    }
    if dataset_url(&url) {
        let source = file_url(url.clone());
        if !files.contains(&source) {
            files.insert(0, source);
        }
    }
    Ok(BookmarkDraft {
        url: url.into(),
        files,
        title: draft.title.trim().to_owned(),
        ..draft
    })
}

pub fn inbox_path() -> Result<PathBuf, String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .ok_or("Your configuration directory is unavailable.")?;
    Ok(config.join("datadeck/inbox"))
}

impl Library {
    pub fn local() -> Result<Self, String> {
        let inbox = inbox_path()?;
        if let Some(legacy) = dirs::data_local_dir().map(|p| p.join("DataDeck/library.sqlite3")) {
            Self::open_migrating(inbox, &legacy)
        } else {
            Self::open(inbox)
        }
    }

    fn open(path: PathBuf) -> Result<Self, String> {
        static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(&path).map_err(message)?;
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let recovery_session = format!(
            "{started}-{}-{}",
            std::process::id(),
            NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
        );
        Ok(Self {
            path,
            recovery_session,
            paths: Default::default(),
        })
    }

    pub fn inbox_path(&self) -> &Path {
        &self.path
    }

    fn open_migrating(path: PathBuf, legacy: &Path) -> Result<Self, String> {
        let library = Self::open(path)?;
        let marker = library.path.join(".sqlite-imported");
        if legacy.is_file() && !marker.exists() {
            let _lock = library.lock(Duration::from_secs(10))?;
            let (mut docs, _) = files::scan(&library.path)?;
            migration::run(legacy, &marker, |row| library.import(&mut docs, row))?;
        }
        Ok(library)
    }

    fn lock(&self, timeout: Duration) -> Result<File, String> {
        let path = self.path.join(".datadeck.lock");
        if fs::symlink_metadata(&path).is_ok_and(|m| !m.is_file()) {
            return Err("Inbox lock must be a regular file.".into());
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(message)?;
        let start = std::time::Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(TryLockError::WouldBlock) if start.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(3))
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(
                        "Another DataDeck window is writing to the inbox. Try again.".into(),
                    );
                }
                Err(TryLockError::Error(error)) => return Err(message(error)),
            }
        }
    }

    fn remember(&self, doc: &Document) {
        self.paths
            .lock()
            .expect("inbox path cache")
            .insert(doc.header.datadeck.id, doc.path.clone());
    }

    fn document(&self, id: i64) -> Result<Document, String> {
        let cached = self
            .paths
            .lock()
            .expect("inbox path cache")
            .get(&id)
            .cloned();
        if let Some(path) = cached
            && let Ok(Some(doc)) = Document::read(path)
            && doc.header.datadeck.id == id
        {
            return Ok(doc);
        }
        let doc = files::scan(&self.path)?
            .0
            .into_iter()
            .find(|doc| doc.header.datadeck.id == id)
            .ok_or("This bookmark was removed or its file is unreadable.")?;
        self.remember(&doc);
        Ok(doc)
    }

    pub fn list_with_warnings(&self) -> Result<(Vec<Bookmark>, Vec<String>), String> {
        let (docs, warnings) = files::scan(&self.path)?;
        let mut bookmarks = Vec::with_capacity(docs.len());
        for doc in docs {
            self.remember(&doc);
            bookmarks.push(doc.bookmark());
        }
        bookmarks.sort_by_key(|b| std::cmp::Reverse((b.captured_at, b.id)));
        Ok((bookmarks, warnings))
    }

    #[cfg(test)]
    pub fn list(&self) -> Result<Vec<Bookmark>, String> {
        self.list_with_warnings().map(|(bookmarks, _)| bookmarks)
    }

    fn next_id(docs: &[Document]) -> i64 {
        let mut id = Utc::now().timestamp_micros();
        while docs.iter().any(|d| d.header.datadeck.id == id) {
            id += 1;
        }
        id
    }

    fn import(&self, docs: &mut Vec<Document>, row: migration::LegacyRecord) -> Result<(), String> {
        let url = http_url(&row.url)?.to_string();
        if let Some(existing) = docs.iter().find(|d| d.header.url == url) {
            if !row.notes.is_empty() && row.notes != existing.notes {
                migration::recover_note(&self.path, &row)?;
            }
            return Ok(());
        }
        let mut header = Header::default();
        header.url = url;
        header.datadeck.id = if row.id != 0 && docs.iter().all(|d| d.header.datadeck.id != row.id) {
            row.id
        } else {
            Self::next_id(docs)
        };
        header.datadeck.revision = row.revision;
        header.datadeck.notes_revision = row.notes_revision;
        header.title = row.title;
        if let Some(title) = row.fetched_title {
            header.set_page_title(title);
        }
        header.kind = Kind::ALL
            .into_iter()
            .find(|kind| kind.label().eq_ignore_ascii_case(&row.kind));
        if !row.kind_manual {
            header.datadeck.auto_kind = header.kind;
        }
        header.set_files(row.files, row.found)?;
        header.set_time(row.captured_at);
        header.images = row.images;
        header.visualizations = row.visualizations;
        if let Some(image) = row.image {
            header.thumbnail = Some(files::asset(&self.path, header.datadeck.id, &image)?);
        }
        let mut doc = Document::new(&self.path, header, row.notes);
        doc.write()?;
        docs.push(doc);
        Ok(())
    }

    #[cfg(test)]
    fn save(&self, draft: BookmarkDraft) -> Result<Bookmark, String> {
        self.store(draft, false).map(|(item, _)| item)
    }

    pub fn capture(&self, draft: BookmarkDraft) -> Result<(Bookmark, bool), String> {
        self.store(draft, true)
    }

    fn store(&self, draft: BookmarkDraft, recapture: bool) -> Result<(Bookmark, bool), String> {
        let draft = normalized(draft)?;
        let _lock = self.lock(Duration::from_secs(5))?;
        let (mut docs, _) = files::scan(&self.path)?;
        let time = docs
            .iter()
            .filter_map(|d| d.header.time())
            .max()
            .unwrap_or_default()
            .saturating_add(1)
            .max(Utc::now().timestamp_millis());
        let (mut doc, inserted) =
            if let Some(index) = docs.iter().position(|d| d.header.url == draft.url) {
                if !recapture {
                    return Err("This URL is already in your library.".into());
                }
                let mut doc = docs.swap_remove(index);
                let addition = draft.notes.trim();
                if !addition.is_empty()
                    && doc.notes.trim_end() != addition
                    && !doc.notes.trim_end().ends_with(&format!("\n\n{addition}"))
                {
                    if !doc.notes.is_empty() && !doc.notes.ends_with("\n\n") {
                        if !doc.notes.ends_with('\n') {
                            doc.notes.push('\n');
                        }
                        doc.notes.push('\n');
                    }
                    doc.notes.push_str(addition);
                    doc.header.datadeck.notes_revision =
                        doc.header.datadeck.notes_revision.saturating_add(1);
                }
                (doc, false)
            } else {
                let mut h = Header::default();
                h.url = draft.url;
                h.datadeck.id = Self::next_id(&docs);
                h.datadeck.revision = 1;
                h.title = draft.title;
                let kind = if recapture && draft.kind == Kind::Other {
                    discovery::url_kind(&http_url(&h.url)?)
                } else {
                    draft.kind
                };
                if recapture {
                    h.set_auto_kind(kind);
                } else {
                    h.kind = Some(kind);
                }
                h.files = draft.files;
                h.set_time(Some(time));
                (Document::new(&self.path, h, draft.notes), true)
            };
        doc.header.set_time(Some(time));
        doc.write()?;
        self.remember(&doc);
        Ok((doc.bookmark(), inserted))
    }

    pub fn save_notes(&self, id: i64, previous: &str, notes: &str) -> Result<i64, String> {
        let _lock = self.lock(Duration::from_millis(40))?;
        let mut doc = match self.document(id) {
            Ok(doc) => doc,
            Err(error) => return Err(self.note_conflict(id, notes, &error)),
        };
        if doc.notes == notes {
            return Ok(doc.header.datadeck.notes_revision);
        }
        if doc.notes != previous {
            return Err(self.note_conflict(
                id,
                notes,
                "Notes changed in another window or editor.",
            ));
        }
        doc.notes = notes.to_owned();
        doc.header.datadeck.notes_revision = doc.header.datadeck.notes_revision.saturating_add(1);
        doc.write()
            .map_err(|reason| self.note_conflict(id, notes, &reason))?;
        Ok(doc.header.datadeck.notes_revision)
    }

    fn note_conflict(&self, id: i64, notes: &str, message: &str) -> String {
        match self.recover_notes(id, notes) {
            Ok(path) => format!("{message} Your draft was saved to {}", path.display()),
            Err(error) => format!(
                "{message} Could not save a recovery copy ({error}). Copy your draft before reloading."
            ),
        }
    }

    fn recover_notes(&self, id: i64, notes: &str) -> std::io::Result<PathBuf> {
        let folder = self.path.join("Recovered Notes");
        fs::create_dir_all(&folder)?;
        if !fs::symlink_metadata(&folder)?.is_dir() {
            return Err(std::io::Error::other("Invalid recovery folder"));
        }
        let path = folder.join(format!("Bookmark {id} - {}.txt", self.recovery_session));
        let mut file = tempfile::NamedTempFile::new_in(&folder)?;
        file.write_all(notes.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(&path)?;
        Ok(path)
    }

    pub fn update(&self, previous: &Bookmark, draft: BookmarkDraft) -> Result<Bookmark, String> {
        let draft = normalized(draft)?;
        let _lock = self.lock(Duration::from_secs(5))?;
        let mut doc = self.document(previous.id)?;
        let moved = draft.url != previous.url;
        if doc.header.datadeck.revision != previous.revision
            || (draft.notes != previous.notes
                && doc.notes != previous.notes
                && doc.notes != draft.notes)
        {
            return Err(
                "This bookmark changed in another window. Reload the library and try again.".into(),
            );
        }
        if moved
            && files::scan(&self.path)?
                .0
                .iter()
                .any(|d| d.header.url == draft.url && d.header.datadeck.id != previous.id)
        {
            return Err("This URL is already in your library.".into());
        }
        let h = &mut doc.header;
        if moved {
            h.title = draft.title;
            h.datadeck.auto_title = None;
            h.datadeck.page_title = None;
        } else if draft.title != previous.title {
            h.set_manual_title(draft.title);
        }
        if moved || draft.files != previous.files {
            h.set_files(draft.files, vec![])?;
        }
        if draft.kind != previous.kind {
            h.kind = Some(draft.kind);
            h.datadeck.auto_kind = None;
        } else if moved && h.automatic_kind() {
            h.set_auto_kind(discovery::url_kind(&http_url(&draft.url)?));
        }
        if moved {
            h.url = draft.url;
            h.thumbnail = None;
            h.images.clear();
            h.visualizations.clear();
        }
        if draft.notes != previous.notes && draft.notes != doc.notes {
            doc.notes = draft.notes;
            h.datadeck.notes_revision = h.datadeck.notes_revision.saturating_add(1);
        }
        h.datadeck.revision = h.datadeck.revision.saturating_add(1);
        doc.write()?;
        Ok(doc.bookmark())
    }

    pub fn remove(&self, previous: &Bookmark) -> Result<(), String> {
        let _lock = self.lock(Duration::from_secs(5))?;
        let doc = self.document(previous.id)?;
        if doc.header.datadeck.revision != previous.revision
            || doc.header.datadeck.notes_revision != previous.notes_revision
            || doc.notes != previous.notes
        {
            return Err(
                "This bookmark changed in another window. Reload the library and try again.".into(),
            );
        }
        doc.unchanged()?;
        fs::remove_file(doc.path).map_err(message)
    }

    #[cfg(test)]
    fn enrich(&self, id: i64) -> Result<Bookmark, String> {
        self.enrich_reporting(id, |_| {})
    }

    pub fn enrich_reporting(
        &self,
        id: i64,
        on_discovery: impl FnOnce(Bookmark),
    ) -> Result<Bookmark, String> {
        let before = self.document(id)?;
        let source = http_url(&before.header.url)?;
        let (metadata, photo) = preview::fetch(&source)?;
        let images = metadata.images;
        let discovered = {
            let _lock = self.lock(Duration::from_secs(5))?;
            let mut doc = self.document(id)?;
            if doc.header.url != before.header.url {
                return Ok(doc.bookmark());
            }
            let h = &mut doc.header;
            h.set_page_title(metadata.title);
            h.set_files(h.user_files().to_vec(), metadata.files)?;
            h.images.clone_from(&images);
            h.visualizations = metadata.visualizations;
            if h.automatic_kind() && metadata.kind != Kind::Other {
                h.set_auto_kind(metadata.kind);
            }
            doc.write()?;
            doc.bookmark()
        };
        let Some(photo) = photo else {
            return Ok(discovered);
        };
        on_discovery(discovered);
        let image = preview::fetch_image(&net::agent(&source), &photo);
        let _lock = self.lock(Duration::from_secs(5))?;
        let mut doc = self.document(id)?;
        if let Some(image) = image
            && doc.header.url == before.header.url
            && doc.header.images == images
        {
            doc.header.thumbnail = Some(files::asset(&self.path, id, &image)?);
            doc.write()?;
        }
        Ok(doc.bookmark())
    }
}

#[cfg(test)]
mod tests;
