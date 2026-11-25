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
    fn title(&self) -> &str {
        if self.item.title.is_empty() {
            &self.item.url
        } else {
            &self.item.title
        }
    }
}

struct Form {
    original: Option<Bookmark>,
    url: Entity<UrlInput>,
    title: Entity<UrlInput>,
    notes: Entity<UrlInput>,
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
    preview_generation: u64,
    preview_states: HashMap<i64, PreviewState>,
    message: Option<String>,
    ready: bool,
    filter: Option<Kind>,
    confirming_remove: Option<i64>,
}

impl EventEmitter<LibraryEvent> for LibraryView {}

impl LibraryView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            entries: vec![],
            selected: None,
            form: None,
            focus: cx.focus_handle().tab_stop(true),
            generation: 0,
            read_generation: 0,
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
        self.read_generation += 1;
        let generation = self.read_generation;
        let job = cx
            .background_executor()
            .spawn(async { Library::local()?.list() });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                if generation != this.read_generation {
                    return;
                }
                this.ready = true;
                match result {
                    Ok(items) => {
                        this.entries = items.into_iter().map(Entry::from).collect();
                        if this.current().is_none() {
                            this.selected = this.entries.first().map(|entry| entry.item.id);
                        }
                        if this.form.is_none() && this.entries.is_empty() {
                            this.start_form(None, cx);
                        }
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
        let notes = self.field(
            "What makes this worth keeping?",
            original
                .as_ref()
                .map(|b| b.notes.clone())
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
            notes,
            kind,
            files,
            generation: self.generation,
            saving: false,
        });
        self.message = None;
        cx.notify();
    }

    pub fn focus_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.form.is_none() {
            self.start_form(None, cx);
        }
        if let Some(form) = &self.form {
            form.url.read(cx).focus(window, cx)
        }
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.entries.is_empty() {
            self.form = None;
            self.message = None;
            cx.notify();
        }
    }

    pub fn add_url(&mut self, url: String, cx: &mut Context<Self>) {
        if !self.ready {
            self.refresh(cx);
        }
        let kind = if crate::web_links::web_url(&url)
            .as_ref()
            .is_some_and(crate::web_links::looks_like_table)
        {
            Kind::Dataset
        } else {
            Kind::Other
        };
        self.write(
            BookmarkDraft {
                url,
                kind,
                ..Default::default()
            },
            None,
            None,
            cx,
        );
    }

    fn save_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else { return };
        if form.saving {
            return;
        }
        let draft = BookmarkDraft {
            url: form.url.read(cx).value().to_owned(),
            title: form.title.read(cx).value().to_owned(),
            notes: form.notes.read(cx).value().to_owned(),
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
        let fetch = original
            .as_ref()
            .is_none_or(|item| item.url != draft.url.trim());
        let job = cx.background_executor().spawn(async move {
            let db = Library::local()?;
            if let Some(previous) = original {
                return db.update(&previous, draft).map(|item| (item, true));
            }
            let dropped_url = form_id
                .is_none()
                .then(|| crate::web_links::web_url(&draft.url))
                .flatten();
            match db.save(draft) {
                Ok(item) => Ok((item, true)),
                Err(error) => {
                    if let Some(url) = dropped_url
                        && let Some(saved) = db.list()?.into_iter().find(|b| b.url == url.as_str())
                    {
                        return Ok((saved, false));
                    }
                    Err(error)
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                let current = this.form.as_ref().map(|f| f.generation);
                let editing = this.form.as_ref().is_some_and(|f| {
                    f.original.is_some()
                        || !f.url.read(cx).value().is_empty()
                        || !f.title.read(cx).value().is_empty()
                        || !f.notes.read(cx).value().is_empty()
                        || f.kind != Kind::Other
                        || !f.files.is_empty()
                });
                let show = if form_id.is_some() {
                    form_id == current
                } else {
                    !editing
                };
                match result {
                    Ok((item, written)) => {
                        let id = item.id;
                        this.upsert(item);
                        if show {
                            this.form = None;
                            this.filter = None;
                            this.selected = Some(id);
                        }
                        if show || form_id.is_none() {
                            this.message = Some(
                                if written {
                                    "Saved locally"
                                } else {
                                    "Already in your library"
                                }
                                .into(),
                            );
                        }
                        if this.ready {
                            this.read_generation += 1;
                        } else {
                            this.refresh(cx);
                        }
                        if fetch && written {
                            this.enrich(id, cx);
                        }
                    }
                    Err(error) if show || form_id.is_none() => {
                        this.message = Some(error);
                        if form_id == current
                            && let Some(form) = &mut this.form
                        {
                            form.saving = false;
                        }
                    }
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn upsert(&mut self, item: Bookmark) {
        match self.entries.binary_search_by(|e| item.id.cmp(&e.item.id)) {
            Ok(index) if self.entries[index].item.revision <= item.revision => {
                self.entries[index] = item.into()
            }
            Ok(_) => {}
            Err(index) => self.entries.insert(index, item.into()),
        }
    }

    fn enrich(&mut self, id: i64, cx: &mut Context<Self>) {
        self.preview_generation += 1;
        let generation = self.preview_generation;
        self.preview_states
            .insert(id, PreviewState::Fetching(generation));
        let job = cx
            .background_executor()
            .spawn(async move { Library::local()?.enrich(id) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                if !matches!(this.preview_states.get(&id), Some(PreviewState::Fetching(n)) if *n == generation) {
                    return;
                }
                match result {
                    Ok(item) => {
                        this.preview_states.remove(&id);
                        this.upsert(item);
                        if this.ready {
                            this.read_generation += 1;
                        } else {
                            this.refresh(cx);
                        }
                    }
                    Err(_) => {
                        this.preview_states.insert(id, PreviewState::Unavailable);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
                        this.preview_states.remove(&id);
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
        self.selected = Some(id);
        self.confirming_remove = None;
        self.form = None;
        self.message = None;
        cx.notify();
    }

    fn choose_filter(&mut self, kind: Option<Kind>, cx: &mut Context<Self>) {
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
        }
        self.message = None;
        cx.notify();
    }
}

fn tag(kind: Kind) -> Div {
    div()
        .font_family(MONO)
        .text_size(px(10.))
        .text_color(rgb(MUTED))
        .child(kind.label())
}

fn label(text: &'static str) -> Div {
    div()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(INK))
        .child(text)
}

fn field_row(name: &'static str, field: Entity<UrlInput>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(label(name))
        .child(field)
}

fn option(id: ElementId, text: &'static str, chosen: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(8.))
        .py(px(4.))
        .rounded_sm()
        .bg(rgb(if chosen { 0xececea } else { PAPER }))
        .text_size(px(11.))
        .text_color(rgb(if chosen { INK } else { MUTED }))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(0xf1f1ef)))
        .child(text)
}

impl LibraryView {
    fn sidebar(&self, compact: bool, cx: &mut Context<Self>) -> Div {
        let visible: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.filter.is_none_or(|k| k == e.item.kind))
            .map(|(i, _)| i)
            .collect();
        let count = visible.len();
        div()
            .flex()
            .flex_col()
            .min_h_0()
            .flex_none()
            .w(px(if compact { 220. } else { 304. }))
            .h_full()
            .border_r_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .px_4()
                    .pt_5()
                    .pb_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Saved links"),
                    )
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(self.entries.len().to_string()),
                    ),
            )
            .child(
                div().px(px(9.)).pb_3().flex().flex_wrap().children(
                    std::iter::once(None)
                        .chain(Kind::ALL.into_iter().map(Some))
                        .enumerate()
                        .map(|(i, k)| {
                            option(
                                ("library-filter", i).into(),
                                k.map(Kind::label).unwrap_or("All"),
                                self.filter == k,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.choose_filter(k, cx)))
                        }),
                ),
            )
            .child(div().h(px(1.)).bg(rgb(LINE)))
            .when(count > 0, |s| {
                s.child(
                    uniform_list(
                        "saved-links",
                        count,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| {
                                    let entry = &this.entries[visible[index]];
                                    let id = entry.item.id;
                                    let active = this.selected == Some(id) && this.form.is_none();
                                    div()
                                        .id(("bookmark", id as usize))
                                        .w_full()
                                        .h(px(64.))
                                        .px_4()
                                        .py_2()
                                        .flex()
                                        .flex_col()
                                        .justify_center()
                                        .gap(px(3.))
                                        .border_l_2()
                                        .border_color(rgb(if active { INK } else { PAPER }))
                                        .bg(rgb(if active { 0xf2f2f0 } else { PAPER }))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(rgb(0xf5f5f3)))
                                        .child(
                                            div()
                                                .w_full()
                                                .truncate()
                                                .text_size(px(13.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(entry.title().to_owned()),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .overflow_hidden()
                                                .child(tag(entry.item.kind))
                                                .child(
                                                    div()
                                                        .font_family(MONO)
                                                        .text_size(px(10.))
                                                        .text_color(rgb(0x696969))
                                                        .truncate()
                                                        .child(entry.host.clone()),
                                                ),
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.show(id, cx);
                                            this.focus(window);
                                        }))
                                })
                                .collect()
                        }),
                    )
                    .flex_1()
                    .w_full()
                    .min_h_0(),
                )
            })
            .when(count == 0 && self.ready, |s| {
                s.child(div().p_4().text_size(px(12.)).text_color(rgb(MUTED)).child(
                    if self.filter.is_some() {
                        "No saved links in this category."
                    } else {
                        "Drop a web link here to save it."
                    },
                ))
            })
            .child(
                div()
                    .mt_auto()
                    .p_3()
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .child(
                        button("library-new", "Add link", false).on_click(cx.listener(
                            |this, _, window, cx| {
                                this.start_form(None, cx);
                                this.focus_url(window, cx);
                            },
                        )),
                    ),
            )
    }

    fn form_panel(&self, form: &Form, cx: &mut Context<Self>) -> Div {
        let editing = form.original.is_some();
        div()
            .max_w(px(620.))
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .pb_2()
                    .text_size(px(25.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if editing {
                        "Edit bookmark"
                    } else {
                        "Save a link"
                    }),
            )
            .child(field_row("Web address", form.url.clone()))
            .child(field_row("Title", form.title.clone()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(label("Category"))
                    .child(div().flex().flex_wrap().gap_1().children(
                        Kind::ALL.into_iter().enumerate().map(|(i, kind)| {
                            option(("library-kind", i).into(), kind.label(), form.kind == kind)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(f) = &mut this.form {
                                        f.kind = kind;
                                        cx.notify();
                                    }
                                }))
                        }),
                    )),
            )
            .child(field_row("Notes", form.notes.clone()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(label("Data files"))
                            .child(
                                option("add-file-url".into(), "＋ Add file", false).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        let field =
                                            this.field("File URL or local path", String::new(), cx);
                                        if let Some(f) = &mut this.form {
                                            f.files.push(field.clone());
                                        }
                                        field.read(cx).focus(window, cx);
                                        cx.notify();
                                    }),
                                ),
                            ),
                    )
                    .children(form.files.iter().enumerate().map(|(i, field)| {
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().flex_1().min_w_0().child(field.clone()))
                            .child(option(("remove-file", i).into(), "Remove", false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(f) = &mut this.form
                                        && i < f.files.len()
                                    {
                                        f.files.remove(i);
                                        cx.notify();
                                    }
                                }),
                            ))
                    })),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .pt_4()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        button(
                            "save-bookmark",
                            if form.saving {
                                "Saving…"
                            } else if editing {
                                "Save changes"
                            } else {
                                "Save bookmark"
                            },
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.save_form(cx))),
                    )
                    .when(!self.entries.is_empty(), |s| {
                        s.child(
                            button("cancel-bookmark", "Cancel", false)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                        )
                    }),
            )
    }

    fn detail(&self, entry: &Entry, cx: &mut Context<Self>) -> Div {
        let item = &entry.item;
        let url = item.url.clone();
        let preview_url = url.clone();
        let id = item.id;
        div()
            .max_w(px(660.))
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(tag(item.kind))
                    .child(button("edit-bookmark", "Edit", false).on_click(cx.listener(
                        |this, _, window, cx| {
                            if let Some(item) = this.current().map(|e| e.item.clone()) {
                                this.start_form(Some(item), cx);
                                this.focus_url(window, cx);
                            }
                        },
                    ))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(25.))
                            .line_height(px(32.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(entry.title().to_owned()),
                    )
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(item.url.clone()),
                    ),
            )
            .child(
                div().flex().gap_2().child(
                    button("visit-bookmark", "Open page ↗", false)
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                ),
            )
            .when_some(entry.preview.clone(), |s, preview| {
                s.child(
                    div()
                        .id("bookmark-preview")
                        .cursor_pointer()
                        .on_click(move |_, _, cx| cx.open_url(&preview_url))
                        .w_full()
                        .h(px(165.))
                        .flex()
                        .justify_center()
                        .bg(rgb(0xf6f6f5))
                        .overflow_hidden()
                        .child(img(preview).size_full().object_fit(ObjectFit::Contain)),
                )
            })
            .when(!item.notes.is_empty(), |s| {
                s.child(
                    div()
                        .border_t_1()
                        .border_color(rgb(LINE))
                        .pt_4()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(label("Notes"))
                        .child(
                            div()
                                .text_size(px(14.))
                                .line_height(px(20.))
                                .child(item.notes.clone()),
                        ),
                )
            })
            .when(!item.files.is_empty(), |s| {
                s.child(
                    div()
                        .flex()
                        .flex_col()
                        .border_t_1()
                        .border_color(rgb(LINE))
                        .pt_4()
                        .gap_2()
                        .child(label("Data files"))
                        .children(item.files.iter().enumerate().map(|(i, path)| {
                            let source = path.clone();
                            div()
                                .id(("open-bookmark-file", i))
                                .min_w_0()
                                .flex()
                                .gap_2()
                                .items_center()
                                .justify_between()
                                .py_2()
                                .border_b_1()
                                .border_color(rgb(LINE))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(0xf5f5f3)))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.))
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .truncate()
                                                .child(crate::caption(path)),
                                        )
                                        .child(
                                            div()
                                                .font_family(MONO)
                                                .text_size(px(10.))
                                                .text_color(rgb(MUTED))
                                                .truncate()
                                                .child(path.clone()),
                                        ),
                                )
                                .child(div().flex_none().text_size(px(11.)).child("View data →"))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(LibraryEvent::OpenData(source.clone()))
                                }))
                        })),
                )
            })
            .child(
                div()
                    .mt_2()
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .pt_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(self.confirming_remove != Some(id), |s| {
                        s.child(
                            option("remove-bookmark".into(), "Remove bookmark", false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.confirming_remove = Some(id);
                                    cx.notify();
                                }),
                            ),
                        )
                    })
                    .when(self.confirming_remove == Some(id), |s| {
                        s.child(div().text_size(px(12.)).child("Remove this bookmark?"))
                            .child(
                                button("confirm-remove", "Remove", true).on_click(
                                    cx.listener(move |this, _, _, cx| this.remove(id, cx)),
                                ),
                            )
                            .child(button("keep-bookmark", "Keep", false).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.confirming_remove = None;
                                    cx.notify();
                                },
                            )))
                    }),
            )
            .when_some(self.preview_states.get(&id), |s, state| match state {
                PreviewState::Fetching(_) => s.child(
                    div()
                        .font_family(MONO)
                        .text_size(px(10.))
                        .text_color(rgb(MUTED))
                        .child("Reading page details…"),
                ),
                PreviewState::Unavailable => s.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(MUTED))
                                .child("Page details unavailable"),
                        )
                        .child(
                            option("retry-preview".into(), "Retry", false)
                                .on_click(cx.listener(move |this, _, _, cx| this.enrich(id, cx))),
                        ),
                ),
            })
    }
}

impl Render for LibraryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let compact = window.viewport_size().width < px(850.);
        let panel = if let Some(form) = &self.form {
            self.form_panel(form, cx)
        } else if let Some(entry) = self.current() {
            self.detail(entry, cx)
        } else {
            div()
                .text_size(px(13.))
                .text_color(rgb(MUTED))
                .child(if self.ready {
                    "Select a saved link or add a new one."
                } else {
                    "Loading library…"
                })
        };
        div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .text_color(rgb(INK))
            .bg(rgb(PAPER))
            .key_context("Library")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &NextField, w, _| w.focus_next()))
            .on_action(cx.listener(|_, _: &PreviousField, w, _| w.focus_prev()))
            .child(self.sidebar(compact, cx))
            .child(
                div()
                    .id("library-details")
                    .min_w_0()
                    .min_h_0()
                    .flex_1()
                    .h_full()
                    .overflow_y_scroll()
                    .px(px(if compact { 20. } else { 34. }))
                    .py(px(24.))
                    .child(panel)
                    .when_some(self.message.clone(), |s, message| {
                        s.child(
                            div()
                                .pt_3()
                                .text_color(rgb(MUTED))
                                .font_family(MONO)
                                .text_size(px(11.))
                                .child(message),
                        )
                    }),
            )
    }
}
