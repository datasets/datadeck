mod files;
mod notes;
use crate::library::{Bookmark, BookmarkDraft, Kind, Library};
use crate::source::{UrlEvent, UrlInput};
use crate::{INK, LINE, MONO, MUTED, PAPER, button};
use gpui::{prelude::*, *};
use std::{collections::HashMap, sync::Arc};

actions!(datadeck_library, [NextField, PreviousField]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", NextField, Some("Library")),
        KeyBinding::new("shift-tab", PreviousField, Some("Library")),
    ]);
}

pub enum LibraryEvent {
    OpenData(String),
}

struct Entry {
    item: Bookmark,
    preview: Option<Arc<Image>>,
    host: String,
}

impl From<Bookmark> for Entry {
    fn from(mut item: Bookmark) -> Self {
        let preview = item
            .image
            .take()
            .map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Png, bytes)));
        let host = url::Url::parse(&item.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        Self {
            item,
            preview,
            host,
        }
    }
}

impl Entry {
    fn merge(old: Option<Self>, item: Bookmark) -> Self {
        let Some(previous) = old else {
            return item.into();
        };
        let (mut entry, other) = if previous.item.revision > item.revision {
            (previous, item)
        } else {
            (Self::from(item), previous.item)
        };
        if other.notes_revision > entry.item.notes_revision {
            entry.item.notes_revision = other.notes_revision;
            entry.item.notes = other.notes;
        }
        if other.captured_at > entry.item.captured_at {
            entry.item.captured_at = other.captured_at;
            entry.item.capture_label = other.capture_label;
        }
        entry
    }

    fn order(&self) -> (Option<i64>, i64) {
        (self.item.captured_at, self.item.id)
    }

    fn title(&self) -> &str {
        if self.item.title.is_empty() {
            &self.host
        } else {
            &self.item.title
        }
    }
}

struct Form {
    original: Option<Bookmark>,
    url: Entity<UrlInput>,
    title: Entity<UrlInput>,
    files: Vec<Entity<UrlInput>>,
    kind: Kind,
    generation: u64,
    saving: bool,
}

enum PreviewState {
    Fetching(u64),
    Unavailable,
}

pub struct LibraryView {
    entries: Vec<Entry>,
    selected: Option<i64>,
    form: Option<Form>,
    focus: FocusHandle,
    generation: u64,
    read_generation: u64,
    reading: bool,
    read_again: bool,
    file_watch: Option<Task<()>>,
    file_snapshot: Option<files::Snapshot>,
    file_warning: Option<String>,
    preview_generation: u64,
    preview_states: HashMap<i64, PreviewState>,
    message: Option<String>,
    ready: bool,
    filter: Option<Kind>,
    confirming_remove: Option<i64>,
    database: Option<Library>,
    capture_input: Entity<UrlInput>,
    notes: HashMap<i64, notes::Note>,
    focus_note: bool,
    last_capture: u64,
    expanded_links: bool,
}

impl EventEmitter<LibraryEvent> for LibraryView {}

impl LibraryView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let capture_input = cx.new(UrlInput::capture);
        cx.subscribe(&capture_input, |this, _, event, cx| {
            if let UrlEvent::Open(text) = event {
                this.capture_text(text, cx);
            }
        })
        .detach();
        cx.on_release(|this, cx| this.flush_all(cx)).detach();
        cx.on_app_quit(|this, cx| {
            this.flush_notes(cx);
            async {}
        })
        .detach();
        Self {
            database: None,
            capture_input,
            notes: HashMap::new(),
            focus_note: false,
            last_capture: 0,
            expanded_links: false,
            entries: vec![],
            selected: None,
            form: None,
            focus: cx.focus_handle().tab_stop(true),
            generation: 0,
            read_generation: 0,
            reading: false,
            read_again: false,
            file_watch: None,
            file_snapshot: None,
            file_warning: None,
            preview_generation: 0,
            preview_states: HashMap::new(),
            message: None,
            ready: false,
            filter: None,
            confirming_remove: None,
        }
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.reading {
            self.read_again = true;
            return;
        }
        self.reading = true;
        let generation = self.read_generation;
        let cached = self.database.clone();
        let start_watching = self.file_watch.is_none();
        let job = cx.background_executor().spawn(async move {
            let database = match cached {
                Some(store) => store,
                None => Library::local()?,
            };
            let stamp = if start_watching {
                files::snapshot(database.inbox_path()).ok()
            } else {
                None
            };
            let (rows, warnings) = database.list_with_warnings()?;
            Ok::<_, String>((database, rows, stamp, warnings))
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                this.reading = false;
                if std::mem::take(&mut this.read_again) || generation != this.read_generation {
                    this.refresh(cx);
                    return;
                }
                this.ready = true;
                match result {
                    Ok((database, items, stamp, warnings)) => {
                        this.file_warning = warnings.first().map(|first| {
                            if warnings.len() == 1 {
                                first.clone()
                            } else {
                                format!("{} inbox files need attention. {}", warnings.len(), first)
                            }
                        });
                        this.database = Some(database);
                        if this.file_watch.is_none() {
                            this.file_snapshot = stamp;
                        }
                        // The settled files are authoritative, even when a hand edit did
                        // not change a revision field or moved a capture date backwards.
                        let mut refreshed: Vec<_> = items.into_iter().map(Entry::from).collect();
                        if let Some(id) = this.selected
                            && this.notes.get(&id).is_some_and(|n| n.has_draft(cx))
                            && !refreshed.iter().any(|e| e.item.id == id)
                            && let Some(index) = this.entries.iter().position(|e| e.item.id == id)
                        {
                            refreshed.push(this.entries.remove(index));
                            this.message = Some(
                                "The selected file changed on disk. Your note is still open."
                                    .into(),
                            );
                        }
                        this.entries = refreshed;
                        this.entries
                            .sort_unstable_by_key(|e| std::cmp::Reverse(e.order()));
                        if this
                            .current()
                            .is_none_or(|e| this.filter.is_some_and(|k| k != e.item.kind))
                        {
                            this.selected = this
                                .entries
                                .iter()
                                .find(|e| this.filter.is_none_or(|k| e.item.kind == k))
                                .map(|e| e.item.id);
                        }
                        if this
                            .form
                            .as_ref()
                            .and_then(|f| f.original.as_ref())
                            .is_some_and(|b| !this.entries.iter().any(|e| e.item.id == b.id))
                        {
                            this.message = Some("This bookmark was removed from the inbox.".into());
                        }
                        this.prepare_note(cx);
                    }
                    Err(error) => this.message = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn current(&self) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| Some(e.item.id) == self.selected)
    }

    fn field(
        &mut self,
        hint: &'static str,
        initial: String,
        cx: &mut Context<Self>,
    ) -> Entity<UrlInput> {
        let input = cx.new(|cx| {
            let mut field = UrlInput::field(hint, cx);
            field.set_text(initial, cx);
            field
        });
        cx.subscribe(&input, |this, _, event, cx| match event {
            UrlEvent::Open(_) => this.save_form(cx),
            UrlEvent::Cancel => this.cancel(cx),
        })
        .detach();
        input
    }

    fn start_form(&mut self, original: Option<Bookmark>, cx: &mut Context<Self>) {
        self.flush_notes(cx);
        self.generation += 1;
        self.confirming_remove = None;
        let url = self.field(
            "https://…",
            original.as_ref().map(|b| b.url.clone()).unwrap_or_default(),
            cx,
        );
        let title = self.field(
            "Use the page title",
            original
                .as_ref()
                .map(|b| b.title.clone())
                .unwrap_or_default(),
            cx,
        );
        let kind = original.as_ref().map(|b| b.kind).unwrap_or_default();
        let files = original
            .iter()
            .flat_map(|b| b.files.iter())
            .map(|path| self.field("File URL or local path", path.clone(), cx))
            .collect();
        self.form = Some(Form {
            original,
            url,
            title,
            kind,
            files,
            generation: self.generation,
            saving: false,
        });
        self.message = None;
        cx.notify();
    }

    pub fn focus_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.capture_input.read(cx).focus(window, cx);
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.entries.is_empty() {
            self.form = None;
            self.message = None;
            cx.notify();
        }
    }

    fn capture_text(&mut self, raw: &str, cx: &mut Context<Self>) {
        if let Some(capture) = crate::web_links::capture_text(raw) {
            self.capture_input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            self.capture(capture.url, capture.note, cx);
        } else {
            self.message = Some("Paste one web address, with an optional note.".into());
            cx.notify();
        }
    }

    pub fn capture(&mut self, url: String, mut note: String, cx: &mut Context<Self>) {
        // Dropping a URL also completes a comment already waiting in the capture field.
        let draft = self.capture_input.read(cx).value().trim();
        if !draft.is_empty() && crate::web_links::capture_text(draft).is_none() {
            note = if note.is_empty() {
                draft.to_owned()
            } else {
                format!("{draft}\n\n{note}")
            };
            self.capture_input
                .update(cx, |field, cx| field.set_text(String::new(), cx));
        }
        self.flush_notes(cx);
        self.last_capture += 1;
        let sequence = self.last_capture;
        let kind = if crate::web_links::web_url(&url)
            .as_ref()
            .is_some_and(crate::web_links::looks_like_table)
        {
            Kind::Dataset
        } else {
            Kind::Other
        };
        self.message = None;
        let job = cx.background_executor().spawn(async move {
            let database = Library::local()?;
            let (item, _) = database.capture(BookmarkDraft {
                url,
                notes: note,
                kind,
                ..Default::default()
            })?;
            Ok::<_, String>((database, item))
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((database, item)) => {
                        let id = item.id;
                        this.database = Some(database);
                        this.upsert(item);
                        if sequence == this.last_capture && this.form.is_none() {
                            this.filter = None;
                            this.show(id, cx);
                            this.focus_note = true;
                        }
                        if this.ready {
                            this.read_generation += 1;
                        } else {
                            this.refresh(cx);
                        }
                        this.enrich(id, cx);
                    }
                    Err(err) => this.message = Some(err),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn save_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else { return };
        if form.saving {
            return;
        }
        let draft = BookmarkDraft {
            url: form.url.read(cx).value().to_owned(),
            title: form.title.read(cx).value().to_owned(),
            notes: form
                .original
                .as_ref()
                .map(|b| b.notes.clone())
                .unwrap_or_default(),
            kind: form.kind,
            files: form
                .files
                .iter()
                .map(|v| v.read(cx).value().to_owned())
                .collect(),
        };
        let original = form.original.clone();
        let generation = form.generation;
        form.saving = true;
        self.write(draft, original, Some(generation), cx);
    }

    fn write(
        &mut self,
        draft: BookmarkDraft,
        original: Option<Bookmark>,
        form_id: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        self.message = None;
        let Some(previous) = original else { return };
        let fetch = previous.url != draft.url.trim();
        let job = cx
            .background_executor()
            .spawn(async move { Library::local()?.update(&previous, draft) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(item) => {
                        let id = item.id;
                        this.upsert(item);
                        if this.form.as_ref().map(|f| f.generation) == form_id {
                            this.show(id, cx);
                        }
                        this.read_generation += 1;
                        if fetch {
                            this.enrich(id, cx);
                        }
                    }
                    Err(error) => {
                        this.message = Some(error);
                        if let Some(f) = &mut this.form {
                            f.saving = false;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn upsert(&mut self, item: Bookmark) {
        let old = self
            .entries
            .iter()
            .position(|e| e.item.id == item.id)
            .map(|i| self.entries.remove(i));
        let entry = Entry::merge(old, item);
        let index = self.entries.partition_point(|e| e.order() > entry.order());
        self.entries.insert(index, entry);
    }

    fn enrich(&mut self, id: i64, cx: &mut Context<Self>) {
        self.preview_generation += 1;
        let generation = self.preview_generation;
        self.preview_states
            .insert(id, PreviewState::Fetching(generation));
        let (tx, rx) = async_channel::unbounded();
        cx.background_executor()
            .spawn(async move {
                let result = Library::local().and_then(|db| {
                    db.enrich_reporting(id, |item| {
                        let _ = tx.try_send((false, Ok(item)));
                    })
                });
                let _ = tx.try_send((true, result));
            })
            .detach();
        cx.spawn(async move |this,cx| {
            while let Ok((complete,result))=rx.recv().await {
                let keep = this.update(cx,|this,cx| {
                    if !matches!(this.preview_states.get(&id),Some(PreviewState::Fetching(n)) if *n==generation) {return false}
                    match result {
                        Ok(item) => {
                            if complete {this.preview_states.remove(&id);}
                            this.upsert(item);
                            this.prepare_note(cx);
                            if this.ready {this.read_generation+=1} else {this.refresh(cx)}
                        }
                        Err(_) => {this.preview_states.insert(id,PreviewState::Unavailable);}
                    }
                    cx.notify();
                    !complete
                });
                if !matches!(keep,Ok(true)) { break; }
            }
        }).detach();
        cx.notify();
    }

    fn remove(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(previous) = self
            .entries
            .iter()
            .find(|e| e.item.id == id)
            .map(|e| e.item.clone())
        else {
            return;
        };
        self.confirming_remove = None;
        let job = cx
            .background_executor()
            .spawn(async move { Library::local()?.remove(&previous) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.entries.retain(|e| e.item.id != id);
                        this.notes.remove(&id);
                        this.preview_states.remove(&id);
                        if this
                            .form
                            .as_ref()
                            .and_then(|f| f.original.as_ref())
                            .is_some_and(|b| b.id == id)
                        {
                            this.form = None;
                        }
                        if this.selected == Some(id) {
                            this.selected = this
                                .entries
                                .iter()
                                .find(|e| this.filter.is_none_or(|k| k == e.item.kind))
                                .map(|e| e.item.id);
                        }
                        this.refresh(cx);
                        this.message = Some("Bookmark removed".into());
                    }
                    Err(error) => this.message = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn show(&mut self, id: i64, cx: &mut Context<Self>) {
        self.flush_notes(cx);
        self.expanded_links = false;
        self.selected = Some(id);
        self.confirming_remove = None;
        self.form = None;
        self.message = None;
        self.prepare_note(cx);
        cx.notify();
    }

    fn choose_filter(&mut self, kind: Option<Kind>, cx: &mut Context<Self>) {
        self.flush_notes(cx);
        self.filter = kind;
        if self
            .current()
            .is_none_or(|e| kind.is_some_and(|k| k != e.item.kind))
        {
            self.selected = self
                .entries
                .iter()
                .find(|e| kind.is_none_or(|k| k == e.item.kind))
                .map(|e| e.item.id);
            self.form = None;
            self.expanded_links = false;
            self.prepare_note(cx);
        }
        self.message = None;
        cx.notify();
    }
}

mod panel;
