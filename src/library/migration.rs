//! One-time import from DataDeck's SQLite library. The caller holds the same
//! inbox lock used by normal Markdown writers for the duration of this operation.

use super::FoundLink;
use rusqlite::{Connection, OpenFlags, Row, types::FromSql};
use serde::de::DeserializeOwned;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) struct LegacyRecord {
    pub id: i64,
    pub revision: i64,
    pub notes_revision: i64,
    pub captured_at: Option<i64>,
    pub url: String,
    pub title: String,
    pub fetched_title: Option<String>,
    pub notes: String,
    pub kind: String,
    pub kind_manual: bool,
    pub files: Vec<String>,
    pub found: Vec<String>,
    pub image: Option<Vec<u8>>,
    pub images: Vec<FoundLink>,
    pub visualizations: Vec<FoundLink>,
}

fn optional<T: FromSql>(row: &Row<'_>, name: &str) -> rusqlite::Result<Option<T>> {
    match row.get(name) {
        Err(rusqlite::Error::InvalidColumnName(_)) => Ok(None),
        result => result,
    }
}

fn links<T: DeserializeOwned>(row: &Row<'_>, name: &str) -> rusqlite::Result<Vec<T>> {
    match optional::<String>(row, name)? {
        None => Ok(Vec::new()),
        Some(raw) => serde_json::from_str(&raw).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                row.as_ref().column_index(name).unwrap_or(0),
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        }),
    }
}

fn decode(row: &Row<'_>) -> rusqlite::Result<LegacyRecord> {
    Ok(LegacyRecord {
        id: row.get("id")?,
        url: row.get("url")?,
        title: row.get("title")?,
        notes: row.get("notes")?,
        kind: row.get("kind")?,
        revision: optional(row, "revision")?.unwrap_or(1),
        notes_revision: optional(row, "notes_revision")?.unwrap_or(0),
        captured_at: optional(row, "captured_at")?,
        fetched_title: optional(row, "fetched_title")?,
        // The old UI allowed people to choose their own category before it
        // distinguished manual and automatically inferred categories.
        kind_manual: optional(row, "kind_manual")?.unwrap_or(true),
        files: links(row, "files")?,
        found: links(row, "found")?,
        image: optional(row, "image")?,
        images: links(row, "images")?,
        visualizations: links(row, "visualizations")?,
    })
}

fn present(path: &Path) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(io::Error::other(format!(
            "Expected a file at {}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn write_checkpoint(path: &Path, contents: &str) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing inbox directory"))?;
    let mut temp = tempfile::NamedTempFile::new_in(directory)?;
    temp.write_all(contents.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    #[cfg(unix)]
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

fn progress_path(marker: &Path) -> PathBuf {
    marker.with_extension("partial")
}

/// Keep a conflicting legacy note available as plain Markdown while retaining
/// the user's existing canonical file for this URL.
pub(super) fn recover_note(inbox: &Path, row: &LegacyRecord) -> Result<(), String> {
    use serde::Serialize;
    use sha2::{Digest, Sha256};

    #[derive(Serialize)]
    struct RecoveryHeader<'a> {
        url: &'a str,
        title: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        fetched_title: Option<&'a str>,
        kind: &'a str,
        captured_at: Option<i64>,
        files: &'a [String],
        found_files: &'a [String],
        images: &'a [FoundLink],
        visualizations: &'a [FoundLink],
        thumbnail: Option<String>,
    }

    let directory = inbox.join("Recovered Notes");
    fs::create_dir_all(&directory).map_err(super::message)?;
    if !fs::symlink_metadata(&directory)
        .map_err(super::message)?
        .is_dir()
    {
        return Err("Recovered Notes must be a directory inside the inbox.".into());
    }
    let thumbnail = row
        .image
        .as_deref()
        .map(|image| super::files::asset(&directory, row.id, image))
        .transpose()?;
    let yaml = serde_yaml_ng::to_string(&RecoveryHeader {
        url: &row.url,
        title: if row.title.is_empty() {
            row.fetched_title.as_deref().unwrap_or("")
        } else {
            &row.title
        },
        fetched_title: row
            .fetched_title
            .as_deref()
            .filter(|s| !row.title.is_empty() && *s != row.title),
        kind: &row.kind,
        captured_at: row.captured_at,
        files: &row.files,
        found_files: &row.found,
        images: &row.images,
        visualizations: &row.visualizations,
        thumbnail,
    })
    .map_err(super::message)?;
    let content = format!("---\n{yaml}---\n\n{}", row.notes);
    let hash = format!("{:x}", Sha256::digest(content.as_bytes()));
    let stem = format!("SQLite bookmark {}-{}", row.id, &hash[..12]);
    // The same input has a deterministic name. Keep independently created or
    // edited recovery files intact, including during a retried import.
    for n in 0u64.. {
        let name = if n == 0 {
            format!("{stem}.md")
        } else {
            format!("{stem}-{n}.md")
        };
        let path = directory.join(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                if meta.is_file()
                    && meta.len() == content.len() as u64
                    && fs::read(&path).map_err(super::message)? == content.as_bytes()
                {
                    return Ok(());
                }
                continue;
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.to_string()),
        }
        let mut temp = tempfile::NamedTempFile::new_in(&directory).map_err(super::message)?;
        temp.write_all(content.as_bytes()).map_err(super::message)?;
        temp.as_file().sync_all().map_err(super::message)?;
        match temp.persist_noclobber(&path) {
            Ok(_) => {
                #[cfg(unix)]
                fs::File::open(&directory)
                    .and_then(|d| d.sync_all())
                    .map_err(super::message)?;
                return Ok(());
            }
            Err(err) if err.error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.to_string()),
        }
    }
    Err("Could not choose a unique recovery filename.".into())
}

/// The importer must durably publish a record without overwriting existing
/// Markdown. An interruption after publication but before the checkpoint can
/// repeat the last callback. Earlier acknowledged records are skipped even
/// if the user has since independently deleted, renamed, or edited them.
pub(super) fn run(
    legacy: &Path,
    marker: &Path,
    mut import: impl FnMut(LegacyRecord) -> Result<(), String>,
) -> Result<(), String> {
    let fail = |detail: &dyn std::fmt::Display| {
        format!(
            "Could not import the old library at {}: {detail}",
            legacy.display()
        )
    };
    if present(marker).map_err(|e| fail(&e))? || !present(legacy).map_err(|e| fail(&e))? {
        return Ok(());
    }
    let progress = progress_path(marker);
    let last: Option<i64> = match fs::read_to_string(&progress) {
        Ok(value) => Some(value.trim().parse().map_err(|e| fail(&e))?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(fail(&error)),
    };
    // Immutable mode would ignore the old application's WAL. A plain
    // read-only connection and a single read transaction see committed frames.
    let mut connection = Connection::open_with_flags(legacy, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| fail(&e))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|e| fail(&e))?;
    let tx = connection.transaction().map_err(|e| fail(&e))?;
    let version: i64 = tx
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| fail(&e))?;
    if version > 2 {
        return Err(fail(&format_args!(
            "unsupported SQLite library version {version}"
        )));
    }
    {
        let mut stmt = tx
            .prepare("SELECT * FROM bookmarks WHERE ?1 IS NULL OR id > ?1 ORDER BY id")
            .map_err(|e| fail(&e))?;
        let mut rows = stmt.query([last]).map_err(|e| fail(&e))?;
        while let Some(row) = rows.next().map_err(|e| fail(&e))? {
            let id: i64 = row.get("id").map_err(|e| fail(&e))?;
            let record = decode(row).map_err(|e| fail(&format_args!("bookmark {id}: {e}")))?;
            import(record).map_err(|e| fail(&format_args!("bookmark {id}: {e}")))?;
            write_checkpoint(&progress, &id.to_string()).map_err(|e| fail(&e))?;
        }
    }
    tx.commit().map_err(|e| fail(&e))?;
    write_checkpoint(
        marker,
        "SQLite bookmarks imported. The original database is retained.\n",
    )
    .map_err(|e| fail(&e))?;
    // A leftover progress file is harmless after this durable completion marker.
    let _ = fs::remove_file(&progress);
    Ok(())
}

#[cfg(test)]
mod tests;
