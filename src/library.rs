mod discovery;
mod net;
mod preview;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Kind {
    Dataset,
    Story,
    Visualization,
    #[default]
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

const FIELDS: &str =
    "id,revision,url,COALESCE(NULLIF(title,''),fetched_title,''),notes,kind,files,found,image,notes_revision,images,visualizations,
    captured_at, COALESCE(strftime('%Y-%m-%d · %H:%M', captured_at / 1000, 'unixepoch', 'localtime'), '')";

fn decode(row: &Row<'_>) -> rusqlite::Result<Bookmark> {
    fn links<T: DeserializeOwned>(row: &Row<'_>, column: usize) -> rusqlite::Result<Vec<T>> {
        serde_json::from_str(&row.get::<_, String>(column)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })
    }
    let mut files = links(row, 6)?;
    for file in links(row, 7)? {
        if !files.contains(&file) {
            files.push(file);
        }
    }
    let kind: String = row.get(5)?;
    Ok(Bookmark {
        id: row.get(0)?,
        revision: row.get(1)?,
        url: row.get(2)?,
        title: row.get(3)?,
        notes: row.get(4)?,
        kind: Kind::ALL
            .into_iter()
            .find(|k| k.label() == kind)
            .unwrap_or_default(),
        files,
        image: row.get(8)?,
        notes_revision: row.get(9)?,
        images: links(row, 10)?,
        visualizations: links(row, 11)?,
        captured_at: row.get(12)?,
        capture_label: row.get(13)?,
    })
}

fn bookmark(conn: &Connection, id: i64) -> Result<Bookmark, String> {
    conn.query_row(
        &format!("SELECT {FIELDS} FROM bookmarks WHERE id=?1"),
        [id],
        decode,
    )
    .optional()
    .map_err(message)?
    .ok_or_else(|| "This bookmark was removed.".into())
}

fn write_error(err: rusqlite::Error) -> String {
    if err
        .sqlite_error()
        .is_some_and(|e| e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
    {
        "This URL is already in your library.".into()
    } else {
        message(err)
    }
}

impl Library {
    pub fn local() -> Result<Self, String> {
        let directory = dirs::data_local_dir()
            .ok_or("The local application data directory is unavailable.")?
            .join("DataDeck");
        std::fs::create_dir_all(&directory).map_err(message)?;
        Self::open(directory.join("library.sqlite3"))
    }

    fn open(path: PathBuf) -> Result<Self, String> {
        static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let recovery_session = format!(
            "{started}-{}-{}",
            std::process::id(),
            NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
        );
        let library = Self {
            path,
            recovery_session,
        };
        let mut conn = library.connect()?;
        let version = |conn: &Connection| {
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .map_err(message)
        };
        if version(&conn)? >= 2 {
            return Ok(library);
        }
        // Another process may have migrated this file while we waited for the write lock.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(message)?;
        if version(&tx)? == 0 {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS bookmarks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            revision INTEGER NOT NULL DEFAULT 1,
            url TEXT UNIQUE NOT NULL,
            title TEXT NOT NULL,
            notes TEXT NOT NULL,
            kind TEXT NOT NULL,
            files TEXT NOT NULL,
            fetched_title TEXT,
            found TEXT NOT NULL DEFAULT '[]',
            image BLOB
        );
        ALTER TABLE bookmarks ADD COLUMN notes_revision INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE bookmarks ADD COLUMN images TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE bookmarks ADD COLUMN visualizations TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE bookmarks ADD COLUMN captured_at INTEGER;
        CREATE INDEX bookmarks_capture ON bookmarks(captured_at DESC, id DESC);
        PRAGMA user_version = 1;",
            )
            .map_err(message)?;
        }
        if version(&tx)? < 2 {
            // Historical categories may have been chosen explicitly; keep them owned by the user.
            tx.execute_batch(
                "ALTER TABLE bookmarks ADD COLUMN kind_manual INTEGER NOT NULL DEFAULT 1;
                              PRAGMA user_version = 2;",
            )
            .map_err(message)?;
        }
        tx.commit().map_err(message)?;
        Ok(library)
    }

    fn connect(&self) -> Result<Connection, String> {
        let conn = Connection::open(&self.path).map_err(message)?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(message)?;
        Ok(conn)
    }

    pub fn list(&self) -> Result<Vec<Bookmark>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {FIELDS} FROM bookmarks ORDER BY captured_at DESC, id DESC"
            ))
            .map_err(message)?;
        stmt.query_map([], decode)
            .map_err(message)?
            .collect::<Result<_, _>>()
            .map_err(message)
    }

    #[cfg(test)]
    pub fn save(&self, draft: BookmarkDraft) -> Result<Bookmark, String> {
        self.store(draft, false).map(|(item, _)| item)
    }

    /// Re-capturing a URL moves the original bookmark to the top without replacing its contents.
    pub fn capture(&self, draft: BookmarkDraft) -> Result<(Bookmark, bool), String> {
        self.store(draft, true)
    }

    fn store(&self, draft: BookmarkDraft, recapture: bool) -> Result<(Bookmark, bool), String> {
        let mut draft = normalized(draft)?;
        if recapture && draft.kind == Kind::Other {
            draft.kind = discovery::url_kind(&http_url(&draft.url)?);
        }
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(message)?;
        // Serialize captures, including two instances saving during the same millisecond.
        let time: i64 = tx.query_row(
            "SELECT max(CAST(unixepoch('subsec') * 1000 AS INTEGER), COALESCE(max(captured_at), 0) + 1) FROM bookmarks",
            [], |row| row.get(0),
        ).map_err(message)?;
        let inserted = tx.execute(
            "INSERT INTO bookmarks(url,title,notes,kind,files,captured_at,kind_manual) VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(url) DO NOTHING",
            params![
                draft.url,
                draft.title,
                draft.notes,
                draft.kind.label(),
                serde_json::to_string(&draft.files).map_err(message)?,
                time,
                !recapture,
            ],
        ).map_err(write_error)? != 0;
        let id = if inserted {
            tx.last_insert_rowid()
        } else if recapture {
            let (id, mut notes): (i64, String) = tx
                .query_row(
                    "SELECT id, notes FROM bookmarks WHERE url=?1",
                    [&draft.url],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(message)?;
            let addition = draft.notes.trim();
            if !addition.is_empty()
                && notes.trim_end() != addition
                && !notes.trim_end().ends_with(&format!("\n\n{addition}"))
            {
                if !notes.is_empty() && !notes.ends_with("\n\n") {
                    if !notes.ends_with('\n') {
                        notes.push('\n');
                    }
                    notes.push('\n');
                }
                notes.push_str(addition);
            }
            tx.execute("UPDATE bookmarks SET captured_at=?1, notes_revision=notes_revision+(notes != ?2), notes=?2 WHERE id=?3",
                params![time, notes, id]).map_err(message)?;
            id
        } else {
            return Err("This URL is already in your library.".into());
        };
        let saved = bookmark(&tx, id)?;
        tx.commit().map_err(message)?;
        Ok((saved, inserted))
    }

    /// A successful return acknowledges a committed edit. Metadata has an independent revision.
    pub fn save_notes(&self, id: i64, previous: &str, notes: &str) -> Result<i64, String> {
        let mut conn = self.connect()?;
        // A foreground navigation flush must never inherit the general five-second lock wait.
        conn.busy_timeout(Duration::from_millis(40))
            .map_err(message)?;
        let tx = conn.transaction().map_err(message)?;
        let revision = tx
            .query_row(
                "UPDATE bookmarks SET notes_revision = notes_revision + (notes != ?2), notes = ?2
             WHERE id = ?1 AND (notes = ?3 OR notes = ?2) RETURNING notes_revision",
                params![id, notes, previous],
                |row| row.get(0),
            )
            .optional()
            .map_err(message)?;
        let Some(revision) = revision else {
            drop(tx);
            let message = "Notes changed in another window or the bookmark was removed.";
            return Err(match self.recover_notes(id, notes) {
                Ok(path) => format!("{message} Your draft was saved to {}", path.display()),
                Err(error) => format!(
                    "{message} Could not save a recovery copy ({error}). Copy your draft before reloading."
                ),
            });
        };
        tx.commit().map_err(message)?;
        Ok(revision)
    }

    fn recover_notes(&self, id: i64, notes: &str) -> std::io::Result<PathBuf> {
        let folder = self.path.with_file_name("Recovered Notes");
        std::fs::create_dir_all(&folder)?;
        let path = folder.join(format!("Bookmark {id} - {}.txt", self.recovery_session));
        // Repeated flushes of this editor replace its one recovery file atomically.
        let mut temporary = tempfile::NamedTempFile::new_in(&folder)?;
        temporary.write_all(notes.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(&path)?;
        Ok(path)
    }

    pub fn update(&self, previous: &Bookmark, draft: BookmarkDraft) -> Result<Bookmark, String> {
        let draft = normalized(draft)?;
        let mut conn = self.connect()?;
        let tx = conn.transaction().map_err(message)?;
        // Keep user-owned and fetched fields separate when the editor leaves them alone.
        // This also preserves metadata that arrived while the edit form was open.
        let changed = tx
            .execute(
                "UPDATE bookmarks SET
                revision = revision + 1,
                url = :url,
                notes_revision = notes_revision + (NOT :keep_notes AND notes != :notes),
                notes = CASE WHEN :keep_notes THEN notes ELSE :notes END,
                kind_manual = kind_manual OR :kind_changed,
                kind = CASE WHEN :kind_changed THEN :kind WHEN url = :url OR kind_manual THEN kind ELSE :url_kind END,
                title = CASE WHEN :keep_title THEN title ELSE :title END,
                files = CASE WHEN :keep_files THEN files ELSE :files END,
                found = CASE WHEN :keep_files THEN found ELSE '[]' END,
                fetched_title = CASE WHEN url = :url THEN fetched_title END,
                image = CASE WHEN url = :url THEN image END,
                images = CASE WHEN url = :url THEN images ELSE '[]' END,
                visualizations = CASE WHEN url = :url THEN visualizations ELSE '[]' END
             WHERE id = :id AND revision = :revision
               AND (:keep_notes OR notes = :previous_notes OR notes = :notes)",
                rusqlite::named_params! {
                    ":url": draft.url,
                    ":notes": draft.notes,
                    ":keep_notes": draft.notes == previous.notes,
                    ":previous_notes": previous.notes,
                    ":kind": draft.kind.label(),
                    ":kind_changed": draft.kind != previous.kind,
                    ":url_kind": discovery::url_kind(&http_url(&draft.url)?).label(),
                    ":title": draft.title,
                    ":files": serde_json::to_string(&draft.files).map_err(message)?,
                    ":keep_title": draft.url == previous.url && draft.title == previous.title,
                    ":keep_files": draft.url == previous.url && draft.files == previous.files,
                    ":id": previous.id,
                    ":revision": previous.revision,
                },
            )
            .map_err(write_error)?;
        if changed == 0 {
            return Err(
                "This bookmark changed in another window. Reload the library and try again.".into(),
            );
        }
        let saved = bookmark(&tx, previous.id)?;
        tx.commit().map_err(message)?;
        Ok(saved)
    }

    pub fn remove(&self, bookmark: &Bookmark) -> Result<(), String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "DELETE FROM bookmarks WHERE id=?1 AND revision=?2 AND notes_revision=?3",
                params![bookmark.id, bookmark.revision, bookmark.notes_revision],
            )
            .map_err(message)?;
        if changed == 0 {
            return Err(
                "This bookmark changed in another window. Reload the library and try again.".into(),
            );
        }
        Ok(())
    }

    #[cfg(test)]
    fn enrich(&self, id: i64) -> Result<Bookmark, String> {
        self.enrich_reporting(id, |_| {})
    }

    /// Runs on a worker. Commit discoveries before the optional thumbnail request begins.
    pub fn enrich_reporting(
        &self,
        id: i64,
        on_discovery: impl FnOnce(Bookmark),
    ) -> Result<Bookmark, String> {
        let before = bookmark(&self.connect()?, id)?;
        let source = http_url(&before.url)?;
        let (metadata, photo) = preview::fetch(&source)?;
        let images = serde_json::to_string(&metadata.images).map_err(message)?;
        let mut conn = self.connect()?;
        let tx = conn.transaction().map_err(message)?;
        tx.execute(
            "UPDATE bookmarks SET fetched_title = NULLIF(?1, ''), found = ?2, images = ?3, visualizations = ?4,
                 kind = CASE WHEN NOT kind_manual AND ?7 != 'Other' THEN ?7 ELSE kind END
             WHERE id = ?5 AND url = ?6",
            params![metadata.title, serde_json::to_string(&metadata.files).map_err(message)?, images,
                serde_json::to_string(&metadata.visualizations).map_err(message)?, id, before.url, metadata.kind.label()],
        ).map_err(message)?;
        let discovered = bookmark(&tx, id)?;
        tx.commit().map_err(message)?;
        let Some(photo) = photo.filter(|_| discovered.url == before.url) else {
            return Ok(discovered);
        };
        on_discovery(discovered);

        let image = preview::fetch_image(&net::agent(&source), &photo);
        let tx = conn.transaction().map_err(message)?;
        if let Some(image) = image {
            // A refreshed page can choose a different image while this network request is in flight.
            tx.execute(
                "UPDATE bookmarks SET image=?1 WHERE id=?2 AND url=?3 AND images=?4",
                params![image, id, before.url, images],
            )
            .map_err(message)?;
        }
        let latest = bookmark(&tx, id)?;
        tx.commit().map_err(message)?;
        Ok(latest)
    }
}

#[cfg(test)]
mod tests;
