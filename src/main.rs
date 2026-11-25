mod capture;
mod cli;
mod data;
mod grid;
mod library;
mod library_navigation;
mod library_view;
mod navigation;
mod notes_editor;
mod overview;
mod query;
mod source;
mod theme;
mod views;
mod watch;
mod web_links;
mod workspace;
use theme::Theme;

use gpui::{prelude::*, *};
use grid::Grid;
use query::Batch;
use source::{UrlEvent, UrlInput};
use std::{
    borrow::Cow,
    path::Path,
    sync::{Arc, Mutex},
};

actions!(
    datadeck,
    [
        OpenFile,
        OpenFolder,
        OpenUrl,
        PasteData,
        ShowQuery,
        LargerType,
        SmallerType,
        DefaultType,
        EditorialTheme,
        MonochromeTheme,
        Quit,
        CloseWindow,
        Quickstart,
        ShowLibrary
    ]
);
const PAPER: u32 = 0xffffff;
const INK: u32 = 0x151515;
const MUTED: u32 = 0x595959;
const LINE: u32 = 0xdedede;

const MONO: &str = "IBM Plex Mono";
const EXAMPLE: &str = "example:field-notes.csv";
const CLIPBOARD: &str = "clipboard:";

fn preference_path(key: &str) -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|dir| dir.join("DataDeck").join(key))
}
fn read_preference(key: &str) -> Option<String> {
    std::fs::read_to_string(preference_path(key)?).ok()
}
fn save_preference(key: &str, value: &str) -> std::io::Result<()> {
    if let Some(path) = preference_path(key)
        && let Some(parent) = path.parent()
    {
        std::fs::create_dir_all(parent)?;
        std::fs::write(path, value)?;
    }
    Ok(())
}

fn caption(source: &str) -> String {
    if source == EXAMPLE {
        return "field-notes.csv".into();
    }
    if source == CLIPBOARD {
        return "Clipboard".into();
    }
    source
        .split(['?', '#'])
        .next()
        .unwrap_or(source)
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(source)
        .into()
}
fn number(value: usize) -> String {
    let digits = value.to_string();
    let mut text = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            text.push(',');
        }
        text.push(c);
    }
    text
}
fn count_label(value: usize, unit: &str) -> String {
    let suffix = if value == 1 { "" } else { "s" };
    format!("{} {unit}{suffix}", number(value))
}
fn button(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px_3()
        .py_1()
        .rounded_sm()
        .border_1()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .bg(rgb(if primary { INK } else { PAPER }))
        .border_color(rgb(if primary { INK } else { LINE }))
        .text_color(rgb(if primary { 0xffffff } else { INK }))
        .hover(move |s| s.bg(rgb(if primary { 0x363636 } else { 0xf2f2f2 })))
        .child(label)
}
fn mark() -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .justify_center()
        .children([22., 16., 22.].map(|width| div().h(px(3.)).w(px(width)).bg(rgb(INK))))
}
enum LoadMessage {
    Update(data::Update),
    Finish(Result<(), String>),
}
fn read_table(
    source: &str,
    clipboard: Option<Arc<String>>,
    sheet: Option<&str>,
    options: data::CsvOptions,
    emit: impl FnMut(data::Update) -> bool,
) -> Result<(), String> {
    if let Some(text) = clipboard {
        data::load_csv_with_options(text.as_bytes(), options, emit)
    } else if source == EXAMPLE {
        data::load_csv_with_options(
            &include_bytes!("../examples/field-notes.csv")[..],
            options,
            emit,
        )
    } else {
        data::load_with_options(source, sheet, options, emit)
    }
}
type CachedDb = Arc<Mutex<Option<rusqlite::Connection>>>;
struct Deck {
    focus: FocusHandle,
    font_size: u8,
    theme: Theme,
    grid: Entity<Grid>,
    result: Entity<Grid>,
    overview: Entity<overview::Overview>,
    overview_open: bool,
    library_open: bool,
    focus_library: bool,
    library: Entity<library_view::LibraryView>,
    link_probe: Option<Task<()>>,
    last_inspector_open: bool,
    url: Entity<UrlInput>,
    editor: Entity<UrlInput>,
    source: String,
    clipboard: Option<Arc<String>>,
    columns: Vec<String>,
    sheets: Vec<String>,
    sheet: Option<String>,
    resources_open: bool,
    workspace: Option<workspace::Workspace>,
    discovery: Option<Task<()>>,
    watcher: Option<Task<()>>,
    csv_options: data::CsvOptions,
    csv_format: Option<(u8, bool)>,
    extra_fields: usize,
    import_open: bool,
    batches: Vec<Batch>,
    rows: usize,
    ready: bool,
    focus_grid: bool,
    querying: bool,
    result_count: usize,
    showing_result: bool,
    query_open: bool,
    url_open: bool,
    sql: Option<String>,
    status: String,
    error: Option<String>,
    db: CachedDb,
    worker: Option<Task<()>>,
    query_task: Option<Task<()>>,
}
impl Deck {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url = cx.new(UrlInput::new);
        let editor = cx.new(UrlInput::query);
        cx.subscribe_in(&url, window, |this, _, event, window, cx| {
            this.url_open = false;
            this.focus.focus(window);
            if let UrlEvent::Open(value) = event {
                this.open_location(value.clone(), cx);
            }
            cx.notify();
        })
        .detach();
        cx.subscribe_in(&editor, window, |this, _, event, window, cx| match event {
            UrlEvent::Open(value) => this.run_query(value, cx),
            UrlEvent::Cancel => {
                this.query_open = false;
                this.focus.focus(window);
                cx.notify();
            }
        })
        .detach();
        let focus = cx.focus_handle();
        focus.focus(window);
        let theme = if read_preference("theme").as_deref() == Some("Monochrome") {
            Theme::Monochrome
        } else {
            Theme::Editorial
        };
        let font_size = read_preference("table-font-size")
            .and_then(|s| s.trim().parse::<u8>().ok())
            .unwrap_or(11)
            .clamp(9, 20);
        let new_grid = |cx: &mut Context<Grid>| {
            let mut grid = Grid::new(cx);
            grid.set_font_size(font_size.into(), cx);
            grid.set_theme(theme, cx);
            grid
        };
        let grid = cx.new(new_grid);
        let result = cx.new(new_grid);
        let overview = cx.new(overview::Overview::new);
        cx.subscribe_in(&overview, window, |s, _, event, w, cx| {
            let overview::OverviewEvent::Select(index) = event;
            s.overview_open = false;
            s.active_grid()
                .update(cx, |g, cx| g.inspect_column(*index, w, cx));
            cx.notify();
        })
        .detach();
        for entity in [&grid, &result] {
            cx.observe(entity, |s, _, cx| {
                let open = s.active_grid().read(cx).inspector_open();
                if open != s.last_inspector_open {
                    s.last_inspector_open = open;
                    cx.notify();
                }
            })
            .detach();
        }
        let library = cx.new(library_view::LibraryView::new);
        cx.subscribe(&library, |s, _, event, cx| {
            let library_view::LibraryEvent::OpenData(source) = event;
            s.load(source.clone(), None, cx);
        })
        .detach();
        #[cfg(target_os = "macos")]
        if let Some(drops) = web_links::browser_drops(window) {
            cx.spawn(async move |this, cx| {
                while let Ok(link) = drops.recv().await {
                    if this.update(cx, |s, cx| s.bookmark(link, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        Self {
            library,
            library_open: false,
            focus_library: false,
            link_probe: None,
            theme,
            font_size,
            focus,
            grid,
            result,
            overview,
            overview_open: false,
            last_inspector_open: true,
            url,
            editor,
            source: String::new(),
            clipboard: None,
            columns: vec![],
            sheets: vec![],
            sheet: None,
            resources_open: false,
            workspace: None,
            discovery: None,
            watcher: None,
            csv_options: Default::default(),
            csv_format: None,
            extra_fields: 0,
            import_open: false,
            batches: vec![],
            rows: 0,
            ready: false,
            focus_grid: false,
            querying: false,
            result_count: 0,
            showing_result: false,
            query_open: false,
            url_open: false,
            sql: None,
            status: "".into(),
            error: None,
            db: Default::default(),
            worker: None,
            query_task: None,
        }
    }
    fn resize_type(&mut self, value: u8, cx: &mut Context<Self>) {
        self.font_size = value.clamp(9, 20);
        self.overview.update(cx, |v, cx| {
            v.set_appearance(self.font_size.into(), self.theme, cx)
        });
        for grid in [&self.grid, &self.result] {
            grid.update(cx, |grid, cx| grid.set_font_size(self.font_size.into(), cx));
        }
        if let Err(error) = save_preference("table-font-size", &self.font_size.to_string()) {
            self.error = Some(format!("Could not remember text size: {error}"));
        }
        cx.notify();
    }
    fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        self.overview.update(cx, |v, cx| {
            v.set_appearance(self.font_size.into(), self.theme, cx)
        });
        for grid in [&self.grid, &self.result] {
            grid.update(cx, |grid, cx| grid.set_theme(theme, cx));
        }
        if let Err(error) = save_preference("theme", theme.label()) {
            self.error = Some(format!("Could not remember theme: {error}"));
        }
        cx.notify();
    }
    fn choose(&mut self, _: &OpenFile, _: &mut Window, cx: &mut Context<Self>) {
        let path = source::pick_file(cx);
        cx.spawn(async move |this, cx| {
            let result = path.await;
            this.update(cx, |this, cx| match result {
                Ok(Some(path)) => this.open_location(path, cx),
                Err(e) => {
                    this.error = Some(e);
                    cx.notify()
                }
                _ => (),
            })
            .ok();
        })
        .detach()
    }
    fn paste_data(&mut self, _: &PasteData, window: &mut Window, cx: &mut Context<Self>) {
        let text = cx.read_from_clipboard().and_then(|item| item.text());
        let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
            self.error = Some("Copy spreadsheet cells or delimited text, then paste again.".into());
            cx.notify();
            return;
        };

        if !capture::clipboard_table(&text) {
            if let Some(item) = web_links::capture_text(&text) {
                self.capture_bookmark(item, cx);
                return;
            }
            if self.library_open
                && self
                    .library
                    .update(cx, |v, cx| v.append_note(&text, window, cx))
            {
                return;
            }
        }
        self.link_probe = None;
        self.leave_library(cx);
        self.discovery = None;
        self.watcher = None;
        self.workspace = None;
        self.import_open = false;
        self.csv_options = Default::default();
        self.clipboard = Some(Arc::new(text));
        self.focus.focus(window);
        self.read_source(CLIPBOARD.into(), None, false, cx);
    }
    fn url(&mut self, _: &OpenUrl, w: &mut Window, cx: &mut Context<Self>) {
        self.link_probe = None;
        if self.library_open {
            self.library.update(cx, |view, cx| view.focus_url(w, cx));
            return;
        }
        self.query_open = false;
        self.url_open = true;
        self.resources_open = false;
        self.url.read(cx).focus(w, cx);
        cx.notify()
    }
    fn query(&mut self, _: &ShowQuery, w: &mut Window, cx: &mut Context<Self>) {
        self.link_probe = None;
        if self.ready {
            self.leave_library(cx);
            self.url_open = false;
            self.query_open = true;
            self.editor.read(cx).focus(w, cx);
            cx.notify();
        }
    }
    fn load(&mut self, source: String, sheet: Option<String>, cx: &mut Context<Self>) {
        self.link_probe = None;
        self.leave_library(cx);
        self.discovery = None;
        let path = Path::new(&source);
        if !source.starts_with("http://")
            && !source.starts_with("https://")
            && (path.is_dir() || workspace::is_manifest(path))
        {
            self.open_workspace(source, cx);
            return;
        }
        if self.source != source {
            self.csv_options = Default::default();
            self.import_open = false;
            if !self
                .workspace
                .as_ref()
                .is_some_and(|w| w.resources.iter().any(|r| r.source == source))
            {
                self.workspace = None;
            }
        }
        self.start_watch(&source, cx);
        self.read_source(source, sheet, false, cx);
    }
    fn read_source(
        &mut self,
        source: String,
        sheet: Option<String>,
        refresh: bool,
        cx: &mut Context<Self>,
    ) {
        if refresh {
            self.refresh_source(source, sheet, cx);
            return;
        }
        if source != CLIPBOARD {
            self.clipboard = None;
        }
        let clipboard = self.clipboard.clone();
        let options = self.csv_options;
        self.worker = None;
        self.query_task = None;
        self.querying = false;
        self.overview_open = false;
        self.focus_grid = true;
        self.ready = false;
        self.query_open = false;
        self.showing_result = false;
        self.resources_open = false;
        self.url_open = false;
        self.sql = None;
        self.status = "Opening source…".into();
        self.error = None;
        self.source = source.clone();
        let previous_batches = std::mem::take(&mut self.batches);
        self.rows = 0;
        self.columns.clear();
        self.sheets.clear();
        self.csv_format = None;
        self.extra_fields = 0;
        self.sheet = sheet.clone();
        self.grid.update(cx, |grid, cx| grid.reset(vec![], cx));
        self.result.update(cx, |grid, cx| grid.reset(vec![], cx));
        self.editor
            .update(cx, |editor, cx| editor.set_text(String::new(), cx));
        self.db = Default::default();
        let (tx, rx) = async_channel::bounded(4);
        std::thread::spawn(move || {
            drop(previous_batches);
            let emit = |u| tx.send_blocking(LoadMessage::Update(u)).is_ok();
            let result = read_table(&source, clipboard, sheet.as_deref(), options, emit);
            let _ = tx.send_blocking(LoadMessage::Finish(result));
        });
        self.worker = Some(cx.spawn(async move |this, cx| {
            let mut frame = std::time::Instant::now();
            while let Ok(message) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        match message {
                            LoadMessage::Update(data::Update::CsvFormat { delimiter, header }) => {
                                this.csv_format = Some((delimiter, header))
                            }
                            LoadMessage::Update(data::Update::Widen(extra)) => {
                                this.extra_fields += extra.len();
                                this.columns.extend(extra);
                                this.columns = query::column_names(&this.columns);
                                this.grid
                                    .update(cx, |g, cx| g.widen(this.columns.clone(), cx));
                            }
                            LoadMessage::Update(data::Update::Schema {
                                columns,
                                sheets,
                                sheet,
                            }) => {
                                this.columns = query::column_names(&columns);
                                this.sheet = sheet;
                                this.sheets = sheets;
                                this.grid
                                    .update(cx, |grid, cx| grid.reset(this.columns.clone(), cx));
                                this.status = "Loading rows…".into();
                            }
                            LoadMessage::Update(data::Update::Rows(rows)) => {
                                this.rows += rows.len();
                                let batch = Arc::new(rows);
                                this.batches.push(batch.clone());
                                this.grid.update(cx, |grid, cx| grid.append(batch, cx));
                            }
                            LoadMessage::Finish(result) => {
                                this.ready = result.is_ok() && !this.columns.is_empty();
                                this.error = result.err();
                                this.status = if this.error.is_some() {
                                    "Load failed"
                                } else if this.columns.is_empty() {
                                    "Empty source"
                                } else {
                                    "Ready"
                                }
                                .into();
                            }
                        }
                        cx.notify()
                    })
                    .is_err()
                {
                    break;
                }
                if frame.elapsed().as_millis() >= 8 {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1))
                        .await;
                    frame = std::time::Instant::now();
                }
            }
        }));
        cx.notify()
    }
    fn run_query(&mut self, input: &str, cx: &mut Context<Self>) {
        if !self.ready {
            return;
        }
        let sql = match query::translate(input, &self.columns) {
            Ok(sql) => sql,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        self.editor.update(cx, |e, cx| e.set_text(sql.clone(), cx));
        self.execute_sql(sql, false, cx);
    }
    fn execute_sql(&mut self, sql: String, refreshing: bool, cx: &mut Context<Self>) {
        self.query_task = None;
        self.querying = true;
        self.error = None;
        self.status = "Running query…".into();
        let batches = self.batches.clone();
        let columns = self.columns.clone();
        let db = self.db.clone();
        let (tx, rx) = async_channel::bounded(1);
        let query = sql.clone();
        std::thread::spawn(move || {
            let run = || -> Result<_, String> {
                let mut cache = db.lock().map_err(|e| e.to_string())?;
                if cache.is_none() {
                    let mut conn = query::database(&columns)?;
                    for rows in batches {
                        if tx.is_closed() {
                            return Err("Cancelled".into());
                        }
                        query::insert(&mut conn, columns.len(), &rows)?;
                    }
                    *cache = Some(conn);
                }
                let conn = cache.as_ref().unwrap();
                let cancel = tx.clone();
                conn.progress_handler(5000, Some(move || cancel.is_closed()));
                let mut names = vec![];
                let mut preview = Vec::new();
                let mut count = 0;
                let result = query::select(
                    conn,
                    &query,
                    |v| {
                        names = v;
                        Ok(())
                    },
                    |row| {
                        count += 1;
                        if preview.len() < 10000 {
                            preview.push(row)
                        }
                        Ok(())
                    },
                );
                conn.progress_handler(0, None::<fn() -> bool>);
                result?;
                Ok((names, preview, count))
            };
            let _ = tx.send_blocking(run());
        });
        self.query_task = Some(cx.spawn(async move |this, cx| {
            if let Ok(response) = rx.recv().await {
                this.update(cx, |this, cx| {
                    this.querying = false;
                    match response {
                        Ok((columns, rows, count)) => {
                            this.result_count = count;
                            this.result.update(cx, |grid, cx| {
                                grid.reset(columns, cx);
                                grid.append(Arc::new(rows), cx);
                            });
                            this.sql = Some(sql);
                            this.showing_result = true;
                            this.status = "Query complete".into();
                        }
                        Err(e) => {
                            this.error = Some(e);
                            if refreshing {
                                this.showing_result = false;
                                this.sql = None;
                                this.status = "Source refreshed · query failed".into();
                            } else {
                                this.status = "Query failed".into();
                            }
                        }
                    }
                    this.update_overview(cx);
                    cx.notify();
                })
                .ok();
            }
        }));
        cx.notify();
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(sql) = self.sql.clone() else { return };
        let dialect = self
            .csv_format
            .map(|(d, h)| data::CsvOptions {
                delimiter: Some(d),
                header: Some(h),
            })
            .unwrap_or_default();
        let sql = cli::freeze(&sql, dialect, self.sheet.as_deref());
        let path = cx.prompt_for_new_path(Path::new("."), Some("recipe.sql"));
        cx.spawn(async move |this, cx| {
            let result = async {
                let file = path
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
                if let Some(file) = file {
                    std::fs::write(&file, format!("{sql}\n")).map_err(|e| e.to_string())?;
                    Ok(Some(file))
                } else {
                    Ok(None)
                }
            }
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(Some(p)) => this.status = format!("Saved {}", p.display()),
                    Err(e) => this.error = Some(e),
                    _ => (),
                }
                cx.notify()
            })
            .ok();
        })
        .detach();
    }
}
impl Render for Deck {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_library) && self.library_open {
            self.library.read(cx).focus(window);
        }
        if !self.library_open && self.focus_grid && !self.columns.is_empty() {
            self.focus_grid = false;
            if !self.url_open && !self.query_open {
                self.grid.focus_handle(cx).focus(window);
            }
        }
        let opened = !self.source.is_empty();
        let mut toolbar = div()
            .id("titlebar")
            .flex_none()
            .h(px(45.))
            .flex()
            .items_center()
            .gap_2()
            .pl(px(if cfg!(target_os = "macos") { 80. } else { 16. }))
            .pr_3()
            .border_b_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .id("drag-region")
                    .window_control_area(WindowControlArea::Drag)
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(mark())
                    .child(
                        div()
                            .truncate()
                            .font_family(MONO)
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(if self.library_open {
                                "DataDeck / Library".into()
                            } else if opened {
                                caption(&self.source)
                            } else {
                                "DataDeck".into()
                            }),
                    ),
            );
        toolbar = toolbar.child(
            button(
                "library-toggle",
                if self.library_open {
                    if opened {
                        "Back to viewer"
                    } else {
                        "Dataset viewer"
                    }
                } else {
                    "Library"
                },
                false,
            )
            .on_click(cx.listener(|s, _, window, cx| s.toggle_library(&ShowLibrary, window, cx))),
        );
        if self.library_open && !opened {
            toolbar = toolbar.child(
                button("library-open-file", "Open file", false)
                    .on_click(cx.listener(|s, _, w, cx| s.choose(&OpenFile, w, cx))),
            );
        }
        if opened && !self.library_open {
            toolbar = toolbar
                .child(
                    button("file", "Open file", false)
                        .on_click(cx.listener(|s, _, w, c| s.choose(&OpenFile, w, c))),
                )
                .child(
                    button("folder", "Open folder", false)
                        .on_click(cx.listener(|s, _, w, c| s.choose_folder(&OpenFolder, w, c))),
                )
                .child(
                    button("url", "Open URL", false)
                        .on_click(cx.listener(|s, _, w, c| s.url(&OpenUrl, w, c))),
                )
                .child(
                    button("paste", "Paste data", false)
                        .on_click(cx.listener(|s, _, w, c| s.paste_data(&PasteData, w, c))),
                );
        }
        if self.ready && !self.library_open {
            toolbar = toolbar.child(
                button("query", "Query", true)
                    .on_click(cx.listener(|s, _, w, c| s.query(&ShowQuery, w, c))),
            );
        }
        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(PAPER))
            .text_color(rgb(INK))
            .key_context("DataDeck")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::paste_data))
            .on_action(cx.listener(Self::toggle_library))
            .on_action(cx.listener(Self::choose))
            .on_action(cx.listener(Self::choose_folder))
            .on_action(cx.listener(Self::url))
            .on_action(cx.listener(Self::query))
            .on_action(
                cx.listener(|s, _: &EditorialTheme, _, cx| s.set_theme(Theme::Editorial, cx)),
            )
            .on_action(
                cx.listener(|s, _: &MonochromeTheme, _, cx| s.set_theme(Theme::Monochrome, cx)),
            )
            .on_action(cx.listener(|s, _: &LargerType, _, cx| s.resize_type(s.font_size + 1, cx)))
            .on_action(cx.listener(|s, _: &SmallerType, _, cx| s.resize_type(s.font_size - 1, cx)))
            .on_action(cx.listener(|s, _: &DefaultType, _, cx| s.resize_type(11, cx)))
            .on_action(|_: &CloseWindow, w, _| w.remove_window())
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                if let Some(path) = paths.paths().first() {
                    this.open_location(path.to_string_lossy().into_owned(), cx);
                }
            }))
            .child(toolbar);
        if self.url_open {
            root = root.child(self.url.clone());
        }
        if let Some(error) = &self.error {
            root = root.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .bg(rgb(0xf0f0f0))
                    .text_color(rgb(0x222222))
                    .text_size(px(12.))
                    .child(div().flex_1().child(error.clone()))
                    .child(button("dismiss", "Dismiss", false).on_click(cx.listener(
                        |s, _, _, cx| {
                            s.error = None;
                            cx.notify()
                        },
                    ))),
            );
        }
        if self.link_probe.is_some() {
            root = root.child(
                div()
                    .px_4()
                    .py_2()
                    .text_size(px(12.))
                    .child("Opening link…"),
            );
        }
        if self.library_open {
            return root.child(self.library.clone());
        }
        if !opened {
            return root.child(
                div().flex_1().flex().items_center().justify_center().child(
                    div()
                        .w(px(456.))
                        .max_w_full()
                        .px_6()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(
                            div()
                                .font_family(MONO)
                                .text_size(px(29.))
                                .font_weight(FontWeight::MEDIUM)
                                .child("Open a dataset"),
                        )
                        .child(
                            div()
                                .text_size(px(14.))
                                .text_color(rgb(MUTED))
                                .child("Drop a CSV, Parquet file, or Excel workbook here."),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    button("start-file", "Choose file…", true).on_click(
                                        cx.listener(|s, _, w, c| s.choose(&OpenFile, w, c)),
                                    ),
                                )
                                .child(button("start-folder", "Open folder…", false).on_click(
                                    cx.listener(|s, _, w, c| s.choose_folder(&OpenFolder, w, c)),
                                ))
                                .child(
                                    button("start-url", "Open URL…", false)
                                        .on_click(cx.listener(|s, _, w, c| s.url(&OpenUrl, w, c))),
                                )
                                .child(button("start-paste", "Paste data", false).on_click(
                                    cx.listener(|s, _, w, c| s.paste_data(&PasteData, w, c)),
                                )),
                        )
                        .child(
                            div()
                                .mt_3()
                                .pt_3()
                                .border_t_1()
                                .border_color(rgb(LINE))
                                .child(
                                    div()
                                        .id("example")
                                        .cursor_pointer()
                                        .text_size(px(12.))
                                        .text_color(rgb(INK))
                                        .hover(|s| s.underline())
                                        .child("Explore the field notes example →")
                                        .on_click(cx.listener(|s, _, _, cx| {
                                            s.load(EXAMPLE.into(), None, cx)
                                        })),
                                ),
                        ),
                ),
            );
        }
        root = self.source_controls(root, cx);
        if self.query_open && !self.url_open {
            root=root.child(self.editor.clone()).child(div().px_5().pb_2().text_size(px(12.)).text_color(rgb(MUTED)).child("Query table data with SQL, or ask “count rows” or “top 5 values in COLUMN”."));
        }
        if self.querying {
            root = root.child(div().px_5().py_1().child(
                button("cancel-query", "Cancel query", false).on_click(cx.listener(
                    |s, _, _, cx| {
                        s.query_task = None;
                        s.querying = false;
                        s.status = "Query cancelled".into();
                        cx.notify();
                    },
                )),
            ));
        }
        if self.sql.is_some() {
            root = root.child(
                div()
                    .px_5()
                    .py_1()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        button("source-tab", "Source", !self.showing_result).on_click(cx.listener(
                            |s, _, _, cx| {
                                s.showing_result = false;
                                s.update_overview(cx);
                                cx.notify()
                            },
                        )),
                    )
                    .child(
                        button("result-tab", "Result", self.showing_result).on_click(cx.listener(
                            |s, _, _, cx| {
                                s.showing_result = true;
                                s.update_overview(cx);
                                cx.notify()
                            },
                        )),
                    )
                    .child(div().flex_1())
                    .child(
                        button("save-sql", "Save last query…", false)
                            .on_click(cx.listener(|s, _, _, cx| s.save(cx))),
                    ),
            );
        }
        if self.ready {
            root = root.child(self.view_switcher(cx));
        }
        root.child(
            div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(if self.overview_open {
                    self.overview.clone().into_any_element()
                } else {
                    self.active_grid().into_any_element()
                }),
        )
        .child(
            div()
                .flex_none()
                .h(px(31.))
                .px_4()
                .border_t_1()
                .border_color(rgb(LINE))
                .bg(rgb(0xffffff))
                .flex()
                .gap_3()
                .items_center()
                .font_family(MONO)
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .child(if self.showing_result && self.result_count > 10000 {
                    format!("First 10,000 of {} rows", number(self.result_count))
                } else {
                    let count = if self.showing_result {
                        self.result_count
                    } else {
                        self.rows
                    };
                    count_label(count, "row")
                })
                .when(!self.showing_result, |s| {
                    s.child(count_label(self.columns.len(), "column"))
                })
                .child(div().flex_1())
                .when(self.csv_format.is_some(), |s| {
                    s.child(
                        div()
                            .id("format")
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child(match self.extra_fields {
                                0 => "CSV settings".into(),
                                1 => "CSV settings · 1 extra field".into(),
                                n => format!("CSV settings · {n} extra fields"),
                            })
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.import_open = !s.import_open;
                                cx.notify()
                            })),
                    )
                })
                .when(self.watcher.is_some(), |s| s.child("Live"))
                .child(self.status.clone())
                .child(
                    div()
                        .id("switch-theme")
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap_1()
                        .ml_2()
                        .pl_3()
                        .border_l_1()
                        .border_color(rgb(LINE))
                        .hover(|s| s.text_color(rgb(INK)))
                        .child(div().size(px(7.)).bg(rgb(self.theme.chart(0))))
                        .child(self.theme.label())
                        .on_click(cx.listener(|s, _, _, cx| s.set_theme(s.theme.other(), cx))),
                )
                .child(
                    div()
                        .ml_2()
                        .pl_3()
                        .border_l_1()
                        .border_color(rgb(LINE))
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(button("smaller", "−", false).on_click(
                            cx.listener(|s, _, _, cx| s.resize_type(s.font_size - 1, cx)),
                        ))
                        .child(format!("{} px", self.font_size))
                        .child(button("larger", "+", false).on_click(
                            cx.listener(|s, _, _, cx| s.resize_type(s.font_size + 1, cx)),
                        )),
                ),
        )
    }
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.as_slice() == ["--inbox-path"] {
        match library::inbox_path() {
            Ok(path) => println!("{}", path.display()),
            Err(error) => {
                eprintln!("DataDeck: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.first().is_some_and(|s| s == "run") {
        if let Err(e) = cli::replay(&args[1..]) {
            eprintln!("DataDeck: {e}");
            std::process::exit(1)
        }
        return;
    }
    if args
        .first()
        .is_some_and(|s| matches!(s.as_str(), "-h" | "--help"))
    {
        println!(
            "DataDeck\n\nOpen the desktop app:\n  datadeck               Open your recent capture log\n  datadeck FILE | FOLDER | DATAPACKAGE | URL\n  datadeck --library     Open your bookmark library\n  datadeck --inbox-path  Print the Markdown inbox directory\n\nRun a saved SQL recipe and write CSV to stdout:\n  datadeck run RECIPE.sql FILE_OR_URL [--sheet NAME] [--delimiter comma|tab|semicolon|pipe] [--header yes|no]\n\nExamples:\n  datadeck examples\n  datadeck examples/field-survey.csv\n  datadeck run examples/clean-field-survey.sql examples/field-survey.csv > cleaned.csv"
        );
        return;
    }
    let initial = args.into_iter().next();
    let app = Application::new();
    let (tx, rx) = async_channel::unbounded();
    app.on_open_urls(move |urls| {
        for url in urls {
            if let Some(path) = url::Url::parse(&url)
                .ok()
                .and_then(|u| u.to_file_path().ok())
            {
                let _ = tx.try_send(path.to_string_lossy().into_owned());
            }
        }
    });
    app.run(move |cx| {
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")),
                Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf")),
            ])
            .expect("load bundled fonts");
        source::bind_keys(cx);
        notes_editor::bind_keys(cx);
        library_view::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("secondary-=", LargerType, None),
            KeyBinding::new("secondary-+", LargerType, None),
            KeyBinding::new("secondary--", SmallerType, None),
            KeyBinding::new("secondary-0", DefaultType, None),
            KeyBinding::new("secondary-shift-o", OpenFolder, None),
            KeyBinding::new("secondary-shift-b", ShowLibrary, None),
            KeyBinding::new("secondary-o", OpenFile, None),
            KeyBinding::new("secondary-l", OpenUrl, None),
            KeyBinding::new("secondary-v", PasteData, Some("DataDeck && !DatasetInput")),
            KeyBinding::new("secondary-k", ShowQuery, None),
            KeyBinding::new("secondary-w", CloseWindow, None),
            KeyBinding::new("secondary-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &Quickstart, cx| {
            cx.open_url("https://github.com/datasets/datadeck/blob/main/docs/quickstart.md")
        });
        cx.set_menus(vec![
            Menu {
                name: "DataDeck".into(),
                items: vec![MenuItem::action("Quit DataDeck", Quit)],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("Open File…", OpenFile),
                    MenuItem::action("Open Folder…", OpenFolder),
                    MenuItem::action("Open URL…", OpenUrl),
                    MenuItem::action("Paste Data", PasteData),
                    MenuItem::action("Query", ShowQuery),
                    MenuItem::action("Close Window", CloseWindow),
                ],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Bookmark Library", ShowLibrary),
                    MenuItem::separator(),
                    MenuItem::action("Increase Table Text Size", LargerType),
                    MenuItem::action("Decrease Table Text Size", SmallerType),
                    MenuItem::action("Reset Table Text Size", DefaultType),
                    MenuItem::separator(),
                    MenuItem::action("Editorial Theme", EditorialTheme),
                    MenuItem::action("Monochrome Theme", MonochromeTheme),
                ],
            },
            Menu {
                name: "Help".into(),
                items: vec![MenuItem::action("Cleaning tutorial and videos", Quickstart)],
            },
        ]);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit()
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1140.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("DataDeck".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(14.), px(14.))),
                }),
                window_min_size: Some(size(px(640.), px(400.))),
                ..Default::default()
            },
            move |w, cx| {
                cx.new(|cx| {
                    let mut deck = Deck::new(w, cx);
                    match initial {
                        Some(flag) if flag == "--library" => {
                            deck.toggle_library(&ShowLibrary, w, cx)
                        }
                        Some(path) => deck.open_location(path, cx),
                        None => deck.toggle_library(&ShowLibrary, w, cx),
                    }
                    cx.spawn(async move |this, cx| {
                        while let Ok(path) = rx.recv().await {
                            if this.update(cx, |s, cx| s.open_location(path, cx)).is_err() {
                                break;
                            }
                        }
                    })
                    .detach();
                    deck
                })
            },
        )
        .expect("open DataDeck window");
        cx.activate(true);
    })
}

#[cfg(test)]
mod clipboard_tests {
    use super::{CLIPBOARD, data, read_table};
    use std::sync::Arc;

    #[test]
    fn clipboard_snapshot_can_be_reparsed_without_reading_a_file() {
        let source = Arc::new(String::from("site\tvisits\nFen\t2\nLake\t3\n"));
        for (header, expected_count) in [(true, 2), (false, 3)] {
            let mut rows = 0;
            let mut format = None;
            read_table(
                CLIPBOARD,
                Some(source.clone()),
                None,
                data::CsvOptions {
                    header: Some(header),
                    delimiter: None,
                },
                |event| {
                    match event {
                        data::Update::CsvFormat { delimiter, header } => {
                            format = Some((delimiter, header))
                        }
                        data::Update::Rows(batch) => rows += batch.len(),
                        _ => (),
                    }
                    true
                },
            )
            .unwrap();
            assert_eq!(rows, expected_count);
            assert_eq!(format, Some((b'\t', header)));
        }
    }
}
