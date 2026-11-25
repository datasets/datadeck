use super::*;

impl Deck {
    pub(super) fn choose_folder(&mut self, _: &OpenFolder, _: &mut Window, cx: &mut Context<Self>) {
        let selection = source::pick_folder(cx);
        cx.spawn(async move |this, cx| {
            let selected = selection.await;
            this.update(cx, |s, cx| match selected {
                Ok(Some(path)) => s.load(path, None, cx),
                Err(e) => {
                    s.error = Some(e);
                    cx.notify()
                }
                Ok(None) => (),
            })
            .ok();
        })
        .detach();
    }
    pub(super) fn open_workspace(&mut self, path: String, cx: &mut Context<Self>) {
        let scan = cx
            .background_executor()
            .spawn(async move { workspace::discover(Path::new(&path)) });
        self.discovery = Some(cx.spawn(async move |this, cx| {
            let result = scan.await;
            this.update(cx, |s, cx| {
                match result {
                    Ok(workspace) => {
                        let Some(first) = workspace.resources.first() else {
                            s.error = Some("No supported datasets in this folder.".into());
                            cx.notify();
                            return;
                        };
                        let source = first.source.clone();
                        s.workspace = Some(workspace);
                        s.csv_options = Default::default();
                        s.start_watch(&source, cx);
                        s.read_source(source, None, false, cx);
                    }
                    Err(e) => s.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        }));
    }
    pub(super) fn start_watch(&mut self, source: &str, cx: &mut Context<Self>) {
        self.watcher = None;
        if source == EXAMPLE || source.starts_with("http://") || source.starts_with("https://") {
            return;
        }
        let path = Path::new(source).to_path_buf();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || watch::watch(path, tx));
        self.watcher = Some(cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                if this
                    .update(cx, |s, cx| {
                        s.read_source(s.source.clone(), s.sheet.clone(), true, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    pub(super) fn source_controls(&self, mut root: Div, cx: &mut Context<Self>) -> Div {
        if let Some(workspace) = &self.workspace {
            let current = workspace
                .resources
                .iter()
                .find(|r| r.source == self.source)
                .map(|r| r.name.as_str())
                .unwrap_or("");
            root = root.child(
                div()
                    .id("resources")
                    .cursor_pointer()
                    .bg(rgb(0xf8f8f8))
                    .px_4()
                    .py_1()
                    .border_b_1()
                    .border_color(rgb(LINE))
                    .text_size(px(12.))
                    .child(format!("{}   /   {}   ▾", workspace.name, current))
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.resources_open = !s.resources_open;
                        cx.notify()
                    })),
            );
            if self.resources_open {
                root =
                    root.child(
                        div()
                            .id("resource-list")
                            .flex_none()
                            .max_h(px(190.))
                            .overflow_y_scroll()
                            .children(workspace.resources.iter().enumerate().map(|(i, r)| {
                                let source = r.source.clone();
                                div()
                                    .id(("resource", i))
                                    .px_4()
                                    .py_1()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(0xf0f0f0)))
                                    .font_family(MONO)
                                    .text_size(px(11.))
                                    .child(r.name.clone())
                                    .on_click(cx.listener(move |s, _, _, cx| {
                                        s.load(source.clone(), None, cx)
                                    }))
                            })),
                    );
            }
        }
        if !self.sheets.is_empty() {
            root = root.child(
                div()
                    .id("sheet-tabs")
                    .flex_none()
                    .overflow_x_scroll()
                    .flex()
                    .gap_1()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(LINE))
                    .children(self.sheets.iter().enumerate().map(|(i, name)| {
                        let active = self.sheet.as_ref() == Some(name);
                        let name = name.clone();
                        div()
                            .id(("sheet-tab", i))
                            .cursor_pointer()
                            .flex_none()
                            .px_3()
                            .py_1()
                            .text_size(px(12.))
                            .border_b_2()
                            .border_color(rgb(if active { INK } else { PAPER }))
                            .child(name.clone())
                            .on_click(cx.listener(move |s, _, _, cx| {
                                if s.sheet.as_ref() != Some(&name) {
                                    s.load(s.source.clone(), Some(name.clone()), cx)
                                }
                            }))
                    })),
            );
        }
        if self.import_open
            && let Some((delimiter, header)) = self.csv_format
        {
            let mut format = div()
                .px_4()
                .py_2()
                .flex()
                .flex_wrap()
                .gap_2()
                .items_center()
                .bg(rgb(0xf7f7f7))
                .text_size(px(12.))
                .child("Delimiter");
            for (id, label, value) in [
                ("comma", "Comma", b','),
                ("tab", "Tab", b'\t'),
                ("semicolon", "Semicolon", b';'),
                ("pipe", "Pipe", b'|'),
            ] {
                format = format.child(button(id, label, delimiter == value).on_click(cx.listener(
                    move |s, _, _, cx| {
                        s.csv_options.delimiter = Some(value);
                        s.read_source(s.source.clone(), None, true, cx)
                    },
                )));
            }
            format = format
                .child(
                    button(
                        "csv-header",
                        if header {
                            "Header row on"
                        } else {
                            "Header row off"
                        },
                        false,
                    )
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.csv_options.header = Some(!header);
                        s.read_source(s.source.clone(), None, true, cx)
                    })),
                )
                .child(
                    button("csv-auto", "Detect again", false).on_click(cx.listener(
                        |s, _, _, cx| {
                            s.csv_options = Default::default();
                            s.read_source(s.source.clone(), None, true, cx)
                        },
                    )),
                );
            root = root.child(format);
        }
        root
    }
}

#[derive(Default)]
struct Snapshot {
    columns: Vec<String>,
    batches: Vec<Batch>,
    sheets: Vec<String>,
    sheet: Option<String>,
    format: Option<(u8, bool)>,
    extra_fields: usize,
    rows: usize,
}
impl Snapshot {
    fn push(&mut self, update: data::Update) {
        match update {
            data::Update::Schema {
                columns,
                sheets,
                sheet,
            } => {
                self.columns = columns;
                self.sheets = sheets;
                self.sheet = sheet;
            }
            data::Update::CsvFormat { delimiter, header } => {
                self.format = Some((delimiter, header))
            }
            data::Update::Widen(names) => {
                self.extra_fields += names.len();
                self.columns.extend(names);
            }
            data::Update::Rows(rows) => {
                self.rows += rows.len();
                self.batches.push(Arc::new(rows));
            }
        }
    }
}
impl Deck {
    /// Build a complete replacement off-thread. A failed or superseded read leaves the last good view intact.
    pub(super) fn refresh_source(
        &mut self,
        source: String,
        sheet: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let options = self.csv_options;
        let (tx, rx) = async_channel::bounded(1);
        self.status = "Refreshing…".into();
        std::thread::spawn(move || {
            let mut snapshot = Snapshot::default();
            let read = read_table(&source, sheet.as_deref(), options, |u| {
                snapshot.push(u);
                !tx.is_closed()
            });
            let _ = tx.send_blocking(read.map(|_| snapshot));
        });
        self.worker = Some(cx.spawn(async move |this, cx| {
            if let Ok(reply) = rx.recv().await {
                this.update(cx, |s, cx| match reply {
                    Err(e) => {
                        s.error = Some(e);
                        s.status = "Refresh failed".into();
                        cx.notify();
                    }
                    Ok(snapshot) => {
                        let replay = if s.showing_result {
                            s.sql.clone()
                        } else {
                            None
                        };
                        let bookmark = s.grid.read(cx).bookmark();
                        let old = std::mem::replace(&mut s.batches, snapshot.batches);
                        s.columns = query::column_names(&snapshot.columns);
                        s.rows = snapshot.rows;
                        s.sheet = snapshot.sheet;
                        s.sheets = snapshot.sheets;
                        s.csv_format = snapshot.format;
                        s.extra_fields = snapshot.extra_fields;
                        s.grid.update(cx, |g, cx| {
                            g.reset(s.columns.clone(), cx);
                            for batch in &s.batches {
                                g.append(batch.clone(), cx);
                            }
                            g.restore(&bookmark, cx);
                        });
                        std::thread::spawn(move || drop(old));
                        s.db = Default::default();
                        s.query_task = None;
                        s.querying = false;
                        s.ready = !s.columns.is_empty();
                        s.error = None;
                        s.status = "Refreshed".into();
                        if let Some(sql) = replay {
                            s.execute_sql(sql, true, cx);
                        } else {
                            s.sql = None;
                            s.showing_result = false;
                            s.result.update(cx, |g, cx| g.reset(vec![], cx));
                        }
                        cx.notify();
                    }
                })
                .ok();
            }
        }));
        cx.notify();
    }
}
