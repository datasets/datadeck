use super::*;
use std::io::{BufRead, BufReader, Cursor, Write};
use std::net::TcpListener;
use std::sync::{Arc, Barrier};
use std::thread;

fn draft(url: &str) -> BookmarkDraft {
    BookmarkDraft {
        url: url.to_owned(),
        kind: Kind::Story,
        notes: "Follow up on units".into(),
        ..Default::default()
    }
}

#[test]
fn persists_offline_and_accepts_real_resource_paths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    let db = Library::open(path.clone()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let offline = format!("http://{}/story", listener.local_addr().unwrap());
    drop(listener);
    let mut entry = draft(&offline);
    entry.title = "My research".into();
    entry.files = vec![
        "https://data.test/file.CSV?dl=1#part".into(),
        "/tmp/a survey.xlsx".into(),
    ];
    let saved = db.save(entry.clone()).unwrap();
    assert!(db.enrich(saved.id).is_err());
    let rows = Library::open(path).unwrap().list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].files,
        ["https://data.test/file.CSV?dl=1", "/tmp/a survey.xlsx"]
    );
    assert_eq!(rows[0].notes, entry.notes);
    assert_eq!(rows[0].kind, Kind::Story);
    assert_eq!(rows[0].title, "My research");
    assert!(db.save(entry).unwrap_err().contains("already"));
    assert!(db.save(draft("javascript:alert(1)")).is_err());
    assert!(db.save(draft("https://secret@example.test")).is_err());
    let link = db
        .save(draft("https://example.test/source.Parquet?q=1#section"))
        .unwrap();
    assert_eq!(link.files, ["https://example.test/source.Parquet?q=1"]);
}

#[test]
fn concurrent_saves_survive_and_conflicting_edits_are_detected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bookmarks.db");
    let first = Library::open(path.clone()).unwrap();
    let second = Library::open(path).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let workers = [first.clone(), second.clone()]
        .into_iter()
        .enumerate()
        .map(|(i, db)| {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                db.save(draft(&format!("https://research.test/{i}")))
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let rows = workers
        .into_iter()
        .map(|w| w.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(first.list().unwrap().len(), 2);
    let mut new = draft(&rows[0].url);
    new.notes = "Newest idea".into();
    second.update(&rows[0], new).unwrap();
    assert!(
        first
            .update(&rows[0], draft(&rows[0].url))
            .unwrap_err()
            .contains("changed")
    );
    assert_eq!(
        bookmark(&first.connect().unwrap(), rows[0].id)
            .unwrap()
            .notes,
        "Newest idea"
    );
}

#[test]
fn fetches_title_and_thumbnail_and_discovers_downloads_without_fetching_data() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = BufReader::new(socket.try_clone().unwrap());
            let mut line = String::new();
            request.read_line(&mut line).unwrap();
            loop {
                let mut header = String::new();
                if request.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
                    break;
                }
            }
            let (mime, bytes) = if line.starts_with("GET /page ") {
                ("text/html; charset=windows-1252",br##"<html><head><title>Fallback</title><meta property="og:title" content="CafX &amp; Tea"><meta property="og:image" content="/thumb.png"><base href="/files/"></head><body><a href="a.csv?raw=1#row">CSV</a><a href="a.csv?raw=1#tables">CSV again</a><a href="fetch?id=5" download="b.xlsx">Excel</a><a href="javascript:thing.csv">No</a></body></html>"##.iter().map(|b|if *b==b'X'{0xe9}else{*b}).collect::<Vec<_>>())
            } else {
                assert!(line.starts_with("GET /thumb.png "), "{line}");
                let mut png = Cursor::new(Vec::new());
                image::RgbImage::new(640, 320)
                    .write_to(&mut png, image::ImageFormat::Png)
                    .unwrap();
                ("image/png", png.into_inner())
            };
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len()).unwrap();
            socket.write_all(&bytes).unwrap();
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let db = Library::open(dir.path().join("db")).unwrap();
    let original = db.save(draft(&format!("{host}/page"))).unwrap();
    let enriched = db.enrich(original.id).unwrap();
    server.join().unwrap();
    assert_eq!(enriched.title, "Café & Tea");
    assert_eq!(
        enriched.files,
        [
            format!("{host}/files/a.csv?raw=1"),
            format!("{host}/files/fetch?id=5")
        ]
    );
    let thumbnail = image::load_from_memory(enriched.image.as_deref().unwrap()).unwrap();
    assert_eq!((thumbnail.width(), thumbnail.height()), (320, 160));
    let mut edit = draft(&original.url);
    edit.notes = "Note edited while preview loaded".into();
    let updated = db.update(&original, edit).unwrap();
    assert_eq!(updated.files, enriched.files);
    assert_eq!(updated.title, enriched.title);
    assert!(updated.image.is_some());
    let moved = db
        .update(&updated, draft("https://example.test/another-source"))
        .unwrap();
    assert!(moved.image.is_none() && moved.title.is_empty() && moved.files.is_empty());
    assert!(db.remove(&original).is_err());
    db.remove(&moved).unwrap();
    assert!(db.list().unwrap().is_empty());
}

#[test]
fn redirected_image_cannot_load_another_private_origin() {
    let page = TcpListener::bind("127.0.0.1:0").unwrap();
    let blocked = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/page", page.local_addr().unwrap());
    let redirect = format!("http://{}/hidden.png", blocked.local_addr().unwrap());
    let server = thread::spawn(move || {
        for response in [
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<title>Still useful</title><meta property='og:image' content='/redirect'>".to_owned(),
            format!("HTTP/1.1 302 Found\r\nLocation: {redirect}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        ] {
            let (mut stream,_) = page.accept().unwrap();
            let mut req=BufReader::new(stream.try_clone().unwrap());
            loop {let mut line=String::new(); if req.read_line(&mut line).unwrap()==0 || line=="\r\n" {break;}}
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    let page = Url::parse(&url).unwrap();
    let (meta, photo) = preview::fetch(&page).unwrap();
    assert!(preview::fetch_image(&net::agent(&page), &photo.unwrap()).is_none());
    server.join().unwrap();
    assert_eq!(meta.title, "Still useful");
    blocked.set_nonblocking(true).unwrap();
    assert_eq!(
        blocked.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn note_saves_survive_navigation_and_cannot_overwrite_another_windows_edit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    let db = Library::open(path.clone()).unwrap();
    let other = Library::open(path.clone()).unwrap();
    let a = db.save(draft("https://notes.test/a")).unwrap();
    let b = db.save(draft("https://notes.test/b")).unwrap();

    let version = db.save_notes(a.id, &a.notes, "new").unwrap();
    assert!(version > a.notes_revision);
    assert!(!dir.path().join("Recovered Notes").exists());
    let first_conflict = other.save_notes(a.id, &a.notes, "stale note").unwrap_err();
    let final_conflict = other
        .save_notes(a.id, &a.notes, "complete losing draft")
        .unwrap_err();
    assert_eq!(first_conflict, final_conflict);
    let recovered: Vec<_> = std::fs::read_dir(dir.path().join("Recovered Notes"))
        .unwrap()
        .collect();
    assert_eq!(recovered.len(), 1);
    let recovered = recovered.into_iter().next().unwrap().unwrap().path();
    assert!(final_conflict.contains(recovered.to_str().unwrap()));
    assert_eq!(
        std::fs::read_to_string(recovered).unwrap(),
        "complete losing draft"
    );
    assert!(other.remove(&a).is_err());

    let mut edit = draft(&a.url);
    edit.title = "Changed title in a second window".into();
    let updated = other.update(&a, edit.clone()).unwrap();
    assert_eq!(updated.notes, "new");
    assert_eq!(updated.notes_revision, version);
    edit.notes = "Edited in form".into();
    db.save_notes(a.id, "new", "newer").unwrap();
    assert!(other.update(&updated, edit).is_err());
    db.save_notes(b.id, &b.notes, "second bookmark").unwrap();
    let latest = db.save_notes(a.id, "newer", "final\n日本語").unwrap();
    assert!(latest > version);
    assert_eq!(
        db.save_notes(a.id, "newer", "final\n日本語").unwrap(),
        latest
    );

    drop((db, other));
    let reopened = Library::open(path).unwrap().list().unwrap();
    assert_eq!(reopened[0].notes, "second bookmark");
    assert_eq!(reopened[1].notes, "final\n日本語");
    assert_eq!(reopened[1].title, "Changed title in a second window");
}

#[test]
fn delayed_preview_reads_notes_committed_while_request_was_in_flight() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/page", listener.local_addr().unwrap());
    let (started, fetching) = std::sync::mpsc::sync_channel(0);
    let (resume, go) = std::sync::mpsc::sync_channel(0);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        while request.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
            line.clear();
        }
        started.send(()).unwrap();
        go.recv_timeout(Duration::from_secs(5)).unwrap();
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<title>Fetched later</title>").unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let db = Library::open(dir.path().join("library.db")).unwrap();
    let item = db.save(draft(&url)).unwrap();
    let id = item.id;
    let network = db.clone();
    let enrichment = thread::spawn(move || network.enrich(id));
    fetching.recv_timeout(Duration::from_secs(5)).unwrap();
    db.save_notes(id, &item.notes, "first pass").unwrap();
    let version = db.save_notes(id, "first pass", "Second pass.").unwrap();
    resume.send(()).unwrap();
    server.join().unwrap();
    let result = enrichment.join().unwrap().unwrap();
    assert_eq!(result.title, "Fetched later");
    assert_eq!(result.notes, "Second pass.");
    assert_eq!(result.notes_revision, version);
}

#[test]
fn discovers_and_persists_images_charts_and_datasets_together() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());
    let document = r#"<head><base href="/reports/"></head><body>
      <figure><img data-src="map.svg"><figcaption>Changes in land use</figcaption></figure>
      <a href="map.svg#photo">Download map</a><img src="pixel.gif" width="1">
      <a href="counts.csv">Counts workbook</a><img src="javascript:alert(1)">
      <figure data-grapher-src="https://ourworldindata.org/grapher/wheat"><figcaption>Wheat yields</figcaption></figure>
      <iframe title="Median rent" src="https://datawrapper.dwcdn.net/Rent1/2/"></iframe>
      <div class="flourish-embed" data-src="visualisation/123" aria-label="Population"></div>
      <tableau-viz src="https://public.tableau.com/views/Map/County" title="County map"></tableau-viz>
      <a href="https://observablehq.com/@team/plot">Compare distributions</a>
      <a href="https://ourworldindata.org/grapher/wheat">Same chart</a>
      <iframe src="https://ourworldindata.org.other.test/grapher/fake"></iframe>
    </body>"#;
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        for line in BufReader::new(stream.try_clone().unwrap())
            .lines()
            .map_while(Result::ok)
        {
            if line.is_empty() {
                break;
            }
        }
        write!(stream, "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{document}", document.len()).unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    let db = Library::open(path.clone()).unwrap();
    let source = db.save(draft(&format!("{host}/page"))).unwrap();
    db.enrich(source.id).unwrap();
    server.join().unwrap();
    let item = Library::open(path).unwrap().list().unwrap().pop().unwrap();
    assert_eq!(item.files, [format!("{host}/reports/counts.csv")]);
    assert_eq!(
        item.images,
        [FoundLink {
            title: "Changes in land use".into(),
            url: format!("{host}/reports/map.svg")
        }]
    );
    let labels = item
        .visualizations
        .iter()
        .map(|v| v.title.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        [
            "Wheat yields",
            "Median rent",
            "Population",
            "County map",
            "Compare distributions"
        ]
    );
    assert_eq!(
        item.visualizations[2].url,
        "https://public.flourish.studio/visualisation/123/"
    );
    assert_eq!(item.captured_at, source.captured_at);
    let moved = db
        .update(&item, draft("https://elsewhere.test/new"))
        .unwrap();
    assert!(moved.images.is_empty() && moved.visualizations.is_empty());
}

#[test]
fn legacy_database_migrates_once_and_recaptures_move_only_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    Connection::open(&path).unwrap().execute_batch("CREATE TABLE bookmarks (
      id INTEGER PRIMARY KEY AUTOINCREMENT, revision INTEGER NOT NULL DEFAULT 1, url TEXT UNIQUE NOT NULL,
      title TEXT NOT NULL, notes TEXT NOT NULL, kind TEXT NOT NULL, files TEXT NOT NULL,
      fetched_title TEXT, found TEXT NOT NULL DEFAULT '[]', image BLOB);
      INSERT INTO bookmarks (url,title,notes,kind,files) VALUES ('https://old.test/a','Existing title','Existing note','Story','[]'), ('https://old.test/b','Older','Note','Story','[]');").unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                Library::open(path).unwrap()
            })
        })
        .collect();
    let databases: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    let db = &databases[0];
    let original = db.list().unwrap();
    assert_eq!((original[0].id, original[1].id), (2, 1));
    assert!(
        original
            .iter()
            .all(|item| item.captured_at.is_none() && item.capture_label.is_empty())
    );
    let (new, inserted) = db.capture(draft("https://new.test/entry")).unwrap();
    assert!(inserted && new.captured_at.is_some() && !new.capture_label.is_empty());
    let mut fresh = draft(&original[1].url);
    fresh.notes = "Extra context".into();
    let (seen, inserted) = databases[1].capture(fresh.clone()).unwrap();
    assert!(!inserted);
    assert_eq!(seen.id, original[1].id);
    assert!(seen.captured_at > new.captured_at);
    assert_eq!(seen.title, "Existing title");
    assert_eq!(seen.notes, "Existing note\n\nExtra context");
    let (again, _) = db.capture(fresh).unwrap();
    assert_eq!(
        (again.notes, again.notes_revision),
        (seen.notes, seen.notes_revision)
    );
    let ordered = Library::open(path).unwrap().list().unwrap();
    assert_eq!(ordered.iter().map(|b| b.id).collect::<Vec<_>>(), [1, 3, 2]);
}

#[test]
fn html_discoveries_and_notes_are_available_before_thumbnail_finishes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());
    let (release, resume) = std::sync::mpsc::sync_channel(0);
    let server = thread::spawn(move || {
        for image in [false, true] {
            let (mut stream, _) = listener.accept().unwrap();
            let request = BufReader::new(stream.try_clone().unwrap());
            for line in request.lines() {
                if line.unwrap().is_empty() {
                    break;
                }
            }
            if image {
                resume.recv_timeout(Duration::from_secs(5)).unwrap();
                stream
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                    )
                    .unwrap();
            } else {
                let page = r#"<title>Weather report</title><meta property="og:image" content="/slow.png"><a href="raw.csv">Data</a>"#;
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{page}", page.len()).unwrap();
            }
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let db = Library::open(dir.path().join("library.db")).unwrap();
    let item = db.save(draft(&format!("{host}/report"))).unwrap();
    let (sender, ready) = std::sync::mpsc::sync_channel(0);
    let worker = db.clone();
    let id = item.id;
    let fetch = thread::spawn(move || worker.enrich_reporting(id, |b| sender.send(b).unwrap()));
    let partial = ready.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(partial.title, "Weather report");
    assert_eq!(partial.files, [format!("{host}/raw.csv")]);
    assert_eq!(partial.images[0].url, format!("{host}/slow.png"));
    assert_eq!(db.list().unwrap()[0].title, "Weather report");
    db.save_notes(id, &item.notes, "Saved while loading")
        .unwrap();
    release.send(()).unwrap();
    server.join().unwrap();
    assert_eq!(fetch.join().unwrap().unwrap().notes, "Saved while loading");
}

#[test]
fn classifies_only_recognized_sources_and_honors_manually_selected_other() {
    let dir = tempfile::tempdir().unwrap();
    let db = Library::open(dir.path().join("library.db")).unwrap();
    let automatic = |url: &str| BookmarkDraft {
        url: url.into(),
        ..Default::default()
    };
    let (chart, _) = db
        .capture(automatic(
            "https://ourworldindata.org/grapher/life-expectancy",
        ))
        .unwrap();
    assert_eq!(chart.kind, Kind::Visualization);
    let (unknown, _) = db
        .capture(automatic("https://example.test/visualizations/index"))
        .unwrap();
    assert_eq!(unknown.kind, Kind::Other);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        for html in [
            r#"<meta property="og:type" content="article"><title>Research story</title>"#,
            r#"<meta property="og:type" content="article"><title>Revised story</title>"#,
            r#"<script type="application/ld+json">{"@context":"https://schema.org","@graph":[{"@type":["Thing","Dataset"],"distribution":[{"contentUrl":"/download1","encodingFormat":"text/csv"},{"contentUrl":"ignored.pdf"}]}]}</script>"#,
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            for line in BufReader::new(stream.try_clone().unwrap()).lines() {
                if line.unwrap().is_empty() {
                    break;
                }
            }
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{html}", html.len()).unwrap();
        }
    });
    let (article, _) = db.capture(automatic(&format!("{host}/story"))).unwrap();
    let article = db.enrich(article.id).unwrap();
    assert_eq!(article.kind, Kind::Story);
    let edited = db.update(&article, automatic(&article.url)).unwrap();
    assert_eq!(edited.kind, Kind::Other);
    assert_eq!(db.enrich(article.id).unwrap().kind, Kind::Other);
    let (dataset, _) = db.capture(automatic(&format!("{host}/data"))).unwrap();
    let dataset = db.enrich(dataset.id).unwrap();
    assert_eq!(dataset.kind, Kind::Dataset);
    assert_eq!(dataset.files, [format!("{host}/download1")]);
    server.join().unwrap();
}
