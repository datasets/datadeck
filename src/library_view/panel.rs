use super::*;
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
    fn capture_line(&self, cx: &mut Context<Self>) -> Div {
        div()
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .child(div().min_w_0().flex_1().child(self.capture_input.clone()))
            .child(
                button("paste-bookmark", "Paste", true).on_click(cx.listener(
                    |this, _, window, cx| {
                        if let Some(text) = cx.read_from_clipboard().and_then(|clip| clip.text()) {
                            this.capture_input
                                .update(cx, |field, cx| field.paste_text(&text, cx));
                            this.focus_url(window, cx);
                        } else {
                            this.focus_url(window, cx)
                        }
                    },
                )),
            )
    }

    fn inbox_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        option("open-inbox-folder".into(), "Open inbox folder ↗", false)
            .on_click(cx.listener(|this, _, _, cx| this.open_inbox(cx)))
    }

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
                            .child("Capture log"),
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
                                                        .child(entry.item.capture_label.clone()),
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
                    .child(self.inbox_button(cx)),
            )
    }

    fn form_panel(&self, form: &Form, cx: &mut Context<Self>) -> Div {
        let id = form.original.as_ref().expect("editing saved details").id;
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
                    .child("Bookmark details"),
            )
            .child(
                div().flex().items_center().gap_2().child(
                    option("open-markdown".into(), "Open Markdown file ↗", false)
                        .on_click(cx.listener(move |this, _, _, cx| this.open_markdown(id, cx))),
                ),
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
                            } else {
                                "Save changes"
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
    }

    fn resource_links(&self, item: &Bookmark, cx: &mut Context<Self>) -> Div {
        let total = item.files.len() + item.visualizations.len() + item.images.len();
        let limit = if self.expanded_links { usize::MAX } else { 1 };
        let mut links = Vec::new();
        for path in item.files.iter().take(limit) {
            links.push(("Data file", crate::caption(path), path.clone(), true));
        }
        for (kind, items) in [
            ("Visualization", &item.visualizations),
            ("Image", &item.images),
        ] {
            for link in items.iter().take(limit) {
                let title = if link.title.is_empty() {
                    crate::caption(&link.url)
                } else {
                    link.title.clone()
                };
                links.push((kind, title, link.url.clone(), false));
            }
        }
        let displayed = links.len();
        div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgb(LINE))
            .pt_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pb_1()
                    .child(label("On this page"))
                    .when(total > displayed || self.expanded_links, |s| {
                        s.child(
                            div()
                                .id("expand-page-links")
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(MUTED))
                                .child(if self.expanded_links {
                                    "Show less".to_owned()
                                } else {
                                    format!("Show all {total}")
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.expanded_links = !this.expanded_links;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .children(
                links
                    .into_iter()
                    .enumerate()
                    .map(|(i, (kind, title, url, is_file))| {
                        div()
                            .id(("page-link", i))
                            .flex()
                            .items_center()
                            .gap_3()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(LINE))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0xf5f5f3)))
                            .child(
                                div()
                                    .w(px(88.))
                                    .font_family(MONO)
                                    .text_size(px(10.))
                                    .text_color(rgb(MUTED))
                                    .child(kind),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(MUTED))
                                    .child(if is_file { "View data →" } else { "Open ↗" }),
                            )
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if is_file {
                                    cx.emit(LibraryEvent::OpenData(url.clone()))
                                } else {
                                    cx.open_url(&url)
                                }
                            }))
                    }),
            )
    }

    fn detail(&self, entry: &Entry, compact: bool, cx: &mut Context<Self>) -> Div {
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
                    .child(
                        option("edit-bookmark".into(), "Details", false).on_click(cx.listener(
                            |this, _, window, cx| {
                                if let Some(item) = this.current().map(|e| e.item.clone()) {
                                    this.start_form(Some(item), cx);
                                    if let Some(form) = &this.form {
                                        form.title.read(cx).focus(window, cx);
                                    }
                                }
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(25.))
                            .line_height(px(31.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(entry.title().to_owned()),
                    )
                    .child(
                        div()
                            .id("visit-bookmark")
                            .cursor_pointer()
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .hover(|s| s.underline())
                            .child(format!("{} ↗", item.url))
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    ),
            )
            .when_some(self.notes.get(&id), |s, n| {
                s.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(label("Notes"))
                                .when(
                                    matches!(
                                        self.preview_states.get(&id),
                                        Some(PreviewState::Fetching(_))
                                    ),
                                    |s| s.child(fetching()),
                                ),
                        )
                        .child(n.editor.clone()),
                )
            })
            .when(
                entry.preview.is_some()
                    || !item.files.is_empty()
                    || !item.visualizations.is_empty()
                    || !item.images.is_empty(),
                |s| {
                    s.child(
                        div()
                            .flex()
                            .when(compact, |s| s.flex_col())
                            .gap_4()
                            .items_start()
                            .w_full()
                            .when_some(entry.preview.clone(), |s, preview| {
                                s.child(
                                    div()
                                        .id("bookmark-preview")
                                        .w(px(185.))
                                        .h(px(125.))
                                        .flex_none()
                                        .overflow_hidden()
                                        .bg(rgb(0xf7f7f5))
                                        .cursor_pointer()
                                        .on_click(move |_, _, cx| cx.open_url(&preview_url))
                                        .child(
                                            img(preview).size_full().object_fit(ObjectFit::Contain),
                                        ),
                                )
                            })
                            .when(
                                !item.files.is_empty()
                                    || !item.visualizations.is_empty()
                                    || !item.images.is_empty(),
                                |s| {
                                    s.child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .when(compact, |s| s.w_full())
                                            .child(self.resource_links(item, cx)),
                                    )
                                },
                            ),
                    )
                },
            )
            .when_some(self.preview_states.get(&id), |s, state| match state {
                PreviewState::Fetching(_) => s,
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
        self.observe_files(cx);
        let compact = window.viewport_size().width < px(850.);
        if std::mem::take(&mut self.focus_note)
            && let Some(note) = self.selected.and_then(|id| self.notes.get(&id))
        {
            note.editor.focus_handle(cx).focus(window);
        }
        let empty = self.entries.is_empty();
        let panel = if let Some(form) = &self.form {
            self.form_panel(form, cx)
        } else if let Some(entry) = self.current() {
            self.detail(entry, compact, cx)
        } else if empty {
            div()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .child(
                    div()
                        .max_w(px(480.))
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(
                            div()
                                .text_size(px(32.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Paste a link"),
                        )
                        .child(div().text_size(px(14.)).text_color(rgb(MUTED)).child(
                            "Include a note above or below the URL, or add one as the page loads.",
                        ))
                        .child(self.capture_line(cx))
                        .when(self.database.is_some(), |s| {
                            s.child(div().flex().justify_start().child(self.inbox_button(cx)))
                        })
                        .when_some(
                            self.message.clone().or_else(|| self.file_warning.clone()),
                            |s, message| {
                                s.child(
                                    div()
                                        .font_family(MONO)
                                        .text_size(px(11.))
                                        .text_color(rgb(MUTED))
                                        .child(message),
                                )
                            },
                        ),
                )
        } else {
            div()
                .text_color(rgb(MUTED))
                .child("No captures in this category.")
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
            .when(!empty, |s| s.child(self.sidebar(compact, cx)))
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
                    .when(!empty, |s| {
                        s.child(
                            div()
                                .max_w(px(660.))
                                .w_full()
                                .pb_4()
                                .mb_4()
                                .border_b_1()
                                .border_color(rgb(LINE))
                                .child(self.capture_line(cx)),
                        )
                    })
                    .child(panel)
                    .when_some(
                        self.message
                            .clone()
                            .or_else(|| self.file_warning.clone())
                            .filter(|_| !empty),
                        |s, message| {
                            s.child(
                                div()
                                    .pt_3()
                                    .text_color(rgb(MUTED))
                                    .font_family(MONO)
                                    .text_size(px(11.))
                                    .child(message),
                            )
                        },
                    ),
            )
    }
}

fn fetching() -> Div {
    // The eight spokes turn one step per frame of the indicator.
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(11.))
        .text_color(rgb(MUTED))
        .child(div().w(px(12.)).font_family(MONO).with_animation(
            "preview-spinner",
            Animation::new(std::time::Duration::from_millis(850)).repeat(),
            |s, time| s.child(["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠇"][(time * 8.) as usize % 8]),
        ))
        .child("Fetching preview")
}
