mod net;
mod preview;

use rusqlite::{Connection, OptionalExtension, Row, params};
use std::path::{Path, PathBuf};
use std::time::Duration;
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

#[derive(Clone, Debug)]
pub struct Bookmark {
    pub id: i64,
    pub revision: i64,
    pub url: String,
    pub title: String,
    pub notes: String,
    pub kind: Kind,
    pub files: Vec<String>,
    pub image: Option<Vec<u8>>,
}

#[derive(Clone)]
pub struct Library {
    path: PathBuf,
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
    "id,revision,url,COALESCE(NULLIF(title,''),fetched_title,''),notes,kind,files,found,image";

fn decode(row: &Row<'_>) -> rusqlite::Result<Bookmark> {
    fn links(row: &Row<'_>, column: usize) -> rusqlite::Result<Vec<String>> {
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
        let library = Self { path };
        library
            .connect()?
            .execute_batch(
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
        )",
            )
            .map_err(message)?;
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
            .prepare(&format!("SELECT {FIELDS} FROM bookmarks ORDER BY id DESC"))
            .map_err(message)?;
        stmt.query_map([], decode)
            .map_err(message)?
            .collect::<Result<_, _>>()
            .map_err(message)
    }

    pub fn save(&self, draft: BookmarkDraft) -> Result<Bookmark, String> {
        let draft = normalized(draft)?;
        let mut conn = self.connect()?;
        let tx = conn.transaction().map_err(message)?;
        tx.execute(
            "INSERT INTO bookmarks(url,title,notes,kind,files) VALUES (?1,?2,?3,?4,?5)",
            params![
                draft.url,
                draft.title,
                draft.notes,
                draft.kind.label(),
                serde_json::to_string(&draft.files).map_err(message)?
            ],
        )
        .map_err(write_error)?;
        let saved = bookmark(&tx, tx.last_insert_rowid())?;
        tx.commit().map_err(message)?;
        Ok(saved)
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
                notes = :notes,
                kind = :kind,
                title = CASE WHEN :keep_title THEN title ELSE :title END,
                files = CASE WHEN :keep_files THEN files ELSE :files END,
                found = CASE WHEN :keep_files THEN found ELSE '[]' END,
                fetched_title = CASE WHEN url = :url THEN fetched_title END,
                image = CASE WHEN url = :url THEN image END
             WHERE id = :id AND revision = :revision",
                rusqlite::named_params! {
                    ":url": draft.url,
                    ":notes": draft.notes,
                    ":kind": draft.kind.label(),
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
                "DELETE FROM bookmarks WHERE id=?1 AND revision=?2",
                params![bookmark.id, bookmark.revision],
            )
            .map_err(message)?;
        if changed == 0 {
            return Err(
                "This bookmark changed in another window. Reload the library and try again.".into(),
            );
        }
        Ok(())
    }

    /// Run off the UI thread. Network I/O always happens outside the write transaction.
    pub fn enrich(&self, id: i64) -> Result<Bookmark, String> {
        let before = bookmark(&self.connect()?, id)?;
        let metadata = preview::fetch(&http_url(&before.url)?)?;
        let mut conn = self.connect()?;
        let tx = conn.transaction().map_err(message)?;
        let files = serde_json::to_string(&metadata.files).map_err(message)?;
        tx.execute(
            "UPDATE bookmarks SET fetched_title = NULLIF(?1, ''), found = ?2, image = COALESCE(?3, image)
             WHERE id = ?4 AND url = ?5",
            params![metadata.title, files, metadata.image, id, before.url],
        ).map_err(message)?;
        let saved = bookmark(&tx, id)?;
        tx.commit().map_err(message)?;
        Ok(saved)
    }
}

#[cfg(test)]
mod tests;
