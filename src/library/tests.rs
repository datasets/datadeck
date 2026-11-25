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
    let meta = preview::fetch(&Url::parse(&url).unwrap()).unwrap();
    server.join().unwrap();
    assert_eq!(meta.title, "Still useful");
    assert!(meta.image.is_none());
    blocked.set_nonblocking(true).unwrap();
    assert_eq!(
        blocked.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
