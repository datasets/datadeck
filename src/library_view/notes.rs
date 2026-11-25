use super::*;
use crate::notes_editor::{NotesEditor, NotesEvent};
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

struct Committed {
    value: String,
    revision: i64,
}

struct Writer {
    database: Library,
    id: i64,
    generation: AtomicU64,
    saved: Mutex<Committed>,
}

impl Writer {
    fn save(&self, version: u64, value: &str) -> Result<Option<i64>, String> {
        let mut saved = self
            .saved
            .lock()
            .map_err(|_| "Could not save this note.".to_owned())?;
        if version != self.generation.load(Ordering::Acquire) {
            return Ok(None);
        }
        if saved.value != value {
            saved.revision = self.database.save_notes(self.id, &saved.value, value)?;
            saved.value = value.to_owned();
        }
        Ok(Some(saved.revision))
    }
}

pub(super) struct Note {
    pub editor: Entity<NotesEditor>,
    writer: Arc<Writer>,
    latest: String,
    revision: i64,
    dirty: bool,
    timer: Option<Task<()>>,
}

impl Note {
    fn flush(&mut self) -> Result<Option<i64>, String> {
        self.timer = None;
        if !self.dirty {
            return Ok(None);
        }
        let generation = self.writer.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let revision = self.writer.save(generation, &self.latest)?;
        if let Some(revision) = revision {
            self.dirty = false;
            self.revision = revision;
        }
        Ok(revision)
    }
}

impl LibraryView {
    pub(super) fn prepare_note(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.current() else { return };
        let id = entry.item.id;
        let revision = entry.item.notes_revision;
        if self
            .notes
            .get(&id)
            .is_some_and(|n| n.dirty || n.revision >= revision)
        {
            return;
        }
        let Some(database) = self.database.clone() else {
            return;
        };
        let value = entry.item.notes.clone();
        let editor = cx.new(|cx| NotesEditor::new(value.clone(), cx));
        cx.subscribe(&editor, move |this, editor, event, cx| match event {
            NotesEvent::Changed => this.note_changed(id, editor.read(cx).value().to_owned(), cx),
            NotesEvent::Capture(text) => this.capture_text(text, cx),
        })
        .detach();
        let writer = Arc::new(Writer {
            database,
            id,
            generation: AtomicU64::new(0),
            saved: Mutex::new(Committed {
                value: value.clone(),
                revision,
            }),
        });
        self.notes.insert(
            id,
            Note {
                editor,
                latest: value,
                revision,
                writer,
                dirty: false,
                timer: None,
            },
        );
        self.notes.retain(|key, n| *key == id || n.dirty);
    }

    pub fn append_note(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.selected else {
            self.focus_url(window, cx);
            self.capture_input
                .update(cx, |field, cx| field.paste_text(text, cx));
            return true;
        };
        self.prepare_note(cx);
        let Some(note) = self.notes.get(&id) else {
            return false;
        };
        let mut value = note.editor.read(cx).value().to_owned();
        if !value.is_empty() {
            value.push_str("\n\n");
        }
        value.push_str(text);
        let editor = note.editor.clone();
        editor.update(cx, |e, cx| e.set_text(value.clone(), cx));
        self.note_changed(id, value, cx);
        self.form = None;
        editor.focus_handle(cx).focus(window);
        cx.notify();
        true
    }

    fn note_changed(&mut self, id: i64, value: String, cx: &mut Context<Self>) {
        let Some(note) = self.notes.get_mut(&id) else {
            return;
        };
        if note.latest == value {
            return;
        }
        note.latest.clone_from(&value);
        note.dirty = true;
        let writer = note.writer.clone();
        let generation = writer.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let background = cx.background_executor().clone();
        note.timer = Some(cx.spawn(async move |this, cx| {
            background.timer(Duration::from_millis(300)).await;
            let result = background
                .spawn(async move { writer.save(generation, &value) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(note) = this.notes.get_mut(&id) else {
                    return;
                };
                if note.writer.generation.load(Ordering::Acquire) != generation {
                    return;
                }
                match result {
                    Ok(Some(revision)) => {
                        note.dirty = false;
                        note.revision = revision;
                        this.apply_note(id);
                    }
                    Err(error) => this.message = Some(error),
                    Ok(None) => {}
                }
                cx.notify();
            });
        }));
    }

    fn apply_note(&mut self, id: i64) {
        if let Some(note) = self.notes.get(&id)
            && let Some(entry) = self.entries.iter_mut().find(|e| e.item.id == id)
            && note.revision >= entry.item.notes_revision
        {
            entry.item.notes_revision = note.revision;
            entry.item.notes.clone_from(&note.latest);
        }
    }

    pub fn flush_notes(&mut self, cx: &mut Context<Self>) {
        self.flush_all(cx);
    }

    pub(super) fn flush_all(&mut self, cx: &App) {
        for note in self.notes.values_mut() {
            let visible = note.editor.read(cx).value();
            if visible != note.latest {
                note.latest = visible.to_owned();
                note.dirty = true;
            }
        }
        let dirty: Vec<_> = self
            .notes
            .iter()
            .filter(|(_, n)| n.dirty)
            .map(|(id, _)| *id)
            .collect();
        for id in dirty {
            match self
                .notes
                .get_mut(&id)
                .expect("pending note exists")
                .flush()
            {
                Ok(_) => self.apply_note(id),
                Err(error) => self.message = Some(error),
            }
        }
    }
}
