use super::*;
use rusqlite::params;
use std::sync::{Arc, Barrier};
use std::thread;

const V0: &str = "CREATE TABLE bookmarks (
    id INTEGER PRIMARY KEY AUTOINCREMENT, revision INTEGER NOT NULL DEFAULT 1,
    url TEXT UNIQUE NOT NULL, title TEXT NOT NULL, notes TEXT NOT NULL,
    kind TEXT NOT NULL, files TEXT NOT NULL, fetched_title TEXT,
    found TEXT NOT NULL DEFAULT '[]', image BLOB
);";

const V1: &str = "ALTER TABLE bookmarks ADD COLUMN notes_revision INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE bookmarks ADD COLUMN images TEXT NOT NULL DEFAULT '[]';
    ALTER TABLE bookmarks ADD COLUMN visualizations TEXT NOT NULL DEFAULT '[]';
    ALTER TABLE bookmarks ADD COLUMN captured_at INTEGER;
    CREATE INDEX bookmarks_capture ON bookmarks(captured_at DESC, id DESC);
    PRAGMA user_version = 1;";

const V2: &str = "ALTER TABLE bookmarks ADD COLUMN kind_manual INTEGER NOT NULL DEFAULT 1;
    PRAGMA user_version = 2;";

const NOTE: &str = "\r\n  My note 🌊\r\n\r\n---\r\n# A heading\n\n  ";
const ADDRESS: &str = "https://example.test/story?part=2#chart";

fn thumbnail() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn populate(path: &Path, version: u8) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(V0).unwrap();
    conn.execute(
        "INSERT INTO bookmarks(id,revision,url,title,notes,kind,files,fetched_title,found,image)
         VALUES (6,7,?1,'My title',?2,'Story','[\"/tmp/my data.xlsx\"]','Published title',
          '[\"https://example.test/data.csv?download=1\"]',?3)",
        params![ADDRESS, NOTE, thumbnail().as_slice()],
    )
    .unwrap();
    insert(&conn, 10);
    if version >= 1 {
        conn.execute_batch(V1).unwrap();
        conn.execute_batch(r#"UPDATE bookmarks SET notes_revision=9,captured_at=1770111000000,
         images='[{"title":"Figure A","url":"https://example.test/a.png"}]',
         visualizations='[{"title":"Interactive chart","url":"https://example.test/chart"}]' WHERE id=6;
         UPDATE bookmarks SET captured_at=1770110999000 WHERE id=10;"#).unwrap();
    }
    if version >= 2 {
        conn.execute_batch(V2).unwrap();
        conn.execute_batch("UPDATE bookmarks SET kind_manual=0 WHERE id=10")
            .unwrap();
    }
    conn
}

fn insert(conn: &Connection, id: i64) {
    conn.execute("INSERT INTO bookmarks(id,url,title,notes,kind,files) VALUES (?1,?2,'','Another note','Other','[]')",
        params![id,format!("https://example.test/{id}")]).unwrap();
}

#[test]
fn reads_every_shipped_schema_without_mutating_the_database() {
    for version in [0, 1, 2] {
        let dir = tempfile::tempdir().unwrap();
        let sqlite = dir.path().join("library.sqlite3");
        let marker = dir.path().join(".sqlite-imported");
        drop(populate(&sqlite, version));
        let original = fs::read(&sqlite).unwrap();
        let mut rows = Vec::new();
        run(&sqlite, &marker, |row| {
            rows.push(row);
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(&sqlite).unwrap(), original);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [6, 10]);
        let first = &rows[0];
        assert_eq!(first.revision, 7);
        assert_eq!(first.url, ADDRESS);
        assert_eq!(first.notes, NOTE);
        assert_eq!(first.title, "My title");
        assert_eq!(first.fetched_title.as_deref(), Some("Published title"));
        assert_eq!(first.kind, "Story");
        assert!(first.kind_manual);
        assert_eq!(rows[1].kind_manual, version < 2);
        assert_eq!(first.files, ["/tmp/my data.xlsx"]);
        assert_eq!(first.found, ["https://example.test/data.csv?download=1"]);
        assert_eq!(first.image.as_deref(), Some(thumbnail().as_slice()));
        if version == 0 {
            assert_eq!(first.notes_revision, 0);
            assert!(first.captured_at.is_none());
            assert!(first.images.is_empty() && first.visualizations.is_empty());
        } else {
            assert_eq!(first.captured_at, Some(1770111000000));
            assert_eq!(first.notes_revision, 9);
            assert_eq!(first.images[0].title, "Figure A");
            assert_eq!(first.visualizations[0].url, "https://example.test/chart");
        }
        run(&sqlite, &marker, |_| {
            panic!("re-imported completed database")
        })
        .unwrap();
    }
}

#[test]
fn failed_import_resumes_without_restoring_acknowledged_rows() {
    let dir = tempfile::tempdir().unwrap();
    let sqlite = dir.path().join("library.sqlite3");
    let marker = dir.path().join(".sqlite-imported");
    run(&sqlite, &marker, |_| unreachable!()).unwrap();
    assert!(!sqlite.exists() && !marker.exists());
    drop(populate(&sqlite, 2));
    let mut seen = Vec::new();
    assert!(
        run(&sqlite, &marker, |r| {
            seen.push(r.id);
            if r.id == 10 {
                return Err("disk full".into());
            }
            Ok(())
        })
        .unwrap_err()
        .contains("disk full")
    );
    assert!(!marker.exists());
    run(&sqlite, &marker, |r| {
        seen.push(r.id);
        Ok(())
    })
    .unwrap();
    assert_eq!(seen, [6, 10, 10]);
    assert!(marker.exists());
    assert!(!progress_path(&marker).exists());
}

#[test]
fn reads_a_consistent_wal_snapshot_while_the_old_app_stays_open() {
    let dir = tempfile::tempdir().unwrap();
    let sqlite = dir.path().join("library.sqlite3");
    let marker = dir.path().join(".sqlite-imported");
    let writer = populate(&sqlite, 2);
    writer
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    insert(&writer, 12);
    let mut seen = Vec::new();
    run(&sqlite, &marker, |r| {
        seen.push(r.id);
        if r.id == 6 {
            insert(&writer, 14);
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(seen, [6, 10, 12]);
    assert_eq!(
        writer
            .query_row("SELECT count(*) FROM bookmarks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
    run(&sqlite, &marker, |_| {
        panic!("old app continued to write after cutover")
    })
    .unwrap();
}

#[test]
fn invalid_json_and_newer_schemas_do_not_create_a_completion_marker() {
    let dir = tempfile::tempdir().unwrap();
    let sqlite = dir.path().join("library.sqlite3");
    let marker = dir.path().join(".sqlite-imported");
    let db = populate(&sqlite, 2);
    db.execute_batch("UPDATE bookmarks SET images='broken json' WHERE id=6")
        .unwrap();
    assert!(
        run(&sqlite, &marker, |_| Ok(()))
            .unwrap_err()
            .contains("bookmark 6")
    );
    assert!(!marker.exists());
    db.execute_batch("PRAGMA user_version=99").unwrap();
    assert!(
        run(&sqlite, &marker, |_| Ok(()))
            .unwrap_err()
            .contains("version 99")
    );
    assert!(!marker.exists());
}

#[test]
fn simultaneous_markdown_library_opens_import_once_and_keep_existing_notes() {
    use crate::library::{BookmarkDraft, Library};
    let dir = tempfile::tempdir().unwrap();
    let sqlite = dir.path().join("library.sqlite3");
    let inbox = dir.path().join("inbox");
    let existing = Library::open_migrating(inbox.clone(), &sqlite).unwrap();
    let manual_note = "Existing Markdown notes";
    existing
        .capture(BookmarkDraft {
            url: "https://example.test/10".into(),
            notes: manual_note.into(),
            ..Default::default()
        })
        .unwrap();
    drop(populate(&sqlite, 2));
    let start = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let start = Arc::clone(&start);
            let sqlite = sqlite.clone();
            let inbox = inbox.clone();
            thread::spawn(move || {
                start.wait();
                Library::open_migrating(inbox, &sqlite)
                    .unwrap()
                    .list()
                    .unwrap()
            })
        })
        .collect();
    for worker in threads {
        let rows = worker.join().unwrap();
        assert_eq!(rows.len(), 2);
        let old = rows.iter().find(|r| r.url == ADDRESS).unwrap();
        assert_eq!(old.notes, NOTE);
        assert_eq!(old.title, "My title");
        assert_eq!(old.captured_at, Some(1770111000000));
        assert_eq!(old.image.as_deref(), Some(thumbnail().as_slice()));
        assert_eq!(old.images.len(), 1);
        assert_eq!(old.visualizations.len(), 1);
        assert_eq!(
            rows.iter().find(|r| r.url.ends_with("/10")).unwrap().notes,
            manual_note
        );
    }
    let db = Library::open_migrating(inbox.clone(), &sqlite).unwrap();
    let item = db
        .list()
        .unwrap()
        .into_iter()
        .find(|r| r.url == ADDRESS)
        .unwrap();
    db.remove(&item).unwrap();
    assert_eq!(
        Library::open_migrating(inbox, &sqlite)
            .unwrap()
            .list()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn an_existing_markdown_id_with_another_url_survives_the_import() {
    use crate::library::{BookmarkDraft, Library};
    let dir = tempfile::tempdir().unwrap();
    let sqlite = dir.path().join("library.sqlite3");
    let inbox = dir.path().join("inbox");
    let db = Library::open_migrating(inbox.clone(), &sqlite).unwrap();
    let (existing, _) = db
        .capture(BookmarkDraft {
            url: "https://local.test/keep".into(),
            notes: "Keep this file byte for byte".into(),
            ..Default::default()
        })
        .unwrap();
    let manual_file = fs::read_dir(&inbox)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "md"))
        .unwrap();
    let before = fs::read(&manual_file).unwrap();

    let writer = populate(&sqlite, 0);
    writer
        .execute("DELETE FROM bookmarks WHERE id=10", [])
        .unwrap();
    writer
        .execute("UPDATE bookmarks SET id=?1 WHERE id=6", [existing.id])
        .unwrap();
    drop(writer);
    let restored = Library::open_migrating(inbox, &sqlite)
        .unwrap()
        .list()
        .unwrap();
    assert_eq!(fs::read(&manual_file).unwrap(), before);
    assert_eq!(restored.len(), 2);
    assert!(
        restored
            .iter()
            .any(|r| r.id != existing.id && r.url == ADDRESS && r.notes == NOTE)
    );
}

#[test]
fn conflicting_legacy_notes_are_preserved_as_recoverable_markdown_once() {
    use crate::library::{BookmarkDraft, Library};
    let dir = tempfile::tempdir().unwrap();
    let sqlite = dir.path().join("old.sqlite3");
    let inbox = dir.path().join("inbox");
    let db = Library::open_migrating(inbox.clone(), &sqlite).unwrap();
    db.capture(BookmarkDraft {
        url: ADDRESS.into(),
        notes: "I already edited this in Markdown.".into(),
        ..Default::default()
    })
    .unwrap();
    let manual_file = fs::read_dir(&inbox)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "md"))
        .unwrap();
    let original = fs::read(&manual_file).unwrap();
    drop(populate(&sqlite, 2));
    let list = Library::open_migrating(inbox.clone(), &sqlite)
        .unwrap()
        .list()
        .unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(fs::read(&manual_file).unwrap(), original);
    let recovery = inbox.join("Recovered Notes");
    let recovered_docs = || {
        fs::read_dir(&recovery)
            .unwrap()
            .map(|p| p.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "md"))
            .collect::<Vec<_>>()
    };
    assert_eq!(recovered_docs().len(), 1);
    let restored = fs::read_to_string(&recovered_docs()[0]).unwrap();
    assert!(restored.ends_with(NOTE));
    assert!(restored.contains(ADDRESS));
    assert!(restored.contains("Published title") && restored.contains("My title"));
    assert!(restored.contains("https://example.test/chart"));
    assert_eq!(fs::read_dir(recovery.join("assets")).unwrap().count(), 1);
    // Simulate a process that had durably published the recovery before it could
    // acknowledge the row checkpoint. The existing copy must be reused.
    fs::remove_file(inbox.join(".sqlite-imported")).unwrap();
    Library::open_migrating(inbox, &sqlite).unwrap();
    assert_eq!(recovered_docs().len(), 1);
}

#[test]
fn an_unrelated_markdown_file_at_the_generated_name_survives() {
    use crate::library::Library;
    let dir = tempfile::tempdir().unwrap();
    let sqlite = dir.path().join("old.sqlite3");
    drop(populate(&sqlite, 2));
    let first = Library::open_migrating(dir.path().join("reference"), &sqlite).unwrap();
    let name = first
        .list()
        .unwrap()
        .into_iter()
        .find(|r| r.url == ADDRESS)
        .unwrap()
        .path
        .file_name()
        .unwrap()
        .to_owned();
    let inbox = dir.path().join("inbox");
    fs::create_dir(&inbox).unwrap();
    let unrelated = inbox.join(name);
    fs::write(&unrelated, "A personal Markdown note without a capture URL").unwrap();
    let list = Library::open_migrating(inbox, &sqlite)
        .unwrap()
        .list()
        .unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().any(|r| r.url == ADDRESS && r.path != unrelated));
    assert_eq!(
        fs::read_to_string(unrelated).unwrap(),
        "A personal Markdown note without a capture URL"
    );
}
