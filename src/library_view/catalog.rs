use super::*;

const ACCENT: u32 = 0x83314e;

fn category_label(kind: Option<Kind>) -> &'static str {
    match kind {
        None => "All",
        Some(Kind::Dataset) => "Datasets",
        Some(Kind::Story) => "Stories",
        Some(Kind::Visualization) => "Visualizations",
        Some(Kind::Other) => "Other",
    }
}

fn metadata() -> Div {
    div()
        .font_family(MONO)
        .text_size(px(11.))
        .text_color(rgb(MUTED))
}

fn note_excerpt(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect()
}

impl LibraryView {
    pub(super) fn catalog(
        &self,
        compact: bool,
        message: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let visible: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.filter.is_none_or(|kind| kind == entry.item.kind))
            .map(|(i, _)| i)
            .collect();
        let count = visible.len();
        let heading = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .pb(px(if compact { 20. } else { 28. }))
            .child(
                div()
                    .text_size(px(if compact { 29. } else { 34. }))
                    .line_height(px(42.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Saved"),
            )
            .child(
                div()
                    .w(px(if compact { 260. } else { 340. }))
                    .child(self.capture_line(cx)),
            );

        let mut catalog = div()
            .id("saved-catalog")
            .tab_group()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .px(px(if compact { 24. } else { 48. }))
            .pt(px(if compact { 24. } else { 34. }))
            .pb_5()
            .child(heading)
            .when_some(message, |row, text| {
                row.child(metadata().pb_2().child(text))
            });
        if !self.entries.is_empty() {
            catalog = catalog.child(self.catalog_filters(count, compact, cx));
        }
        if count == 0 {
            return catalog.when(self.ready, |row| {
                row.child(empty_catalog(self.entries.is_empty()))
            });
        }

        let list = uniform_list(
            "saved-links",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| this.catalog_row(&this.entries[visible[index]], compact, cx))
                    .collect()
            }),
        )
        .track_scroll(self.catalog_scroll.clone())
        .flex_1()
        .min_h_0()
        .w_full();
        catalog.child(list)
    }

    fn catalog_filters(&self, count: usize, compact: bool, cx: &mut Context<Self>) -> Div {
        let kinds = std::iter::once(None).chain(Kind::ALL.into_iter().map(Some));
        let filters = kinds.enumerate().map(|(index, kind)| {
            let selected = self.filter == kind;
            div()
                .id(("library-filter", index))
                .tab_index(0)
                .cursor_pointer()
                .pt_1()
                .pb_2()
                .border_b_2()
                .border_color(rgb(if selected { ACCENT } else { PAPER }))
                .text_size(px(12.))
                .text_color(rgb(if selected { ACCENT } else { MUTED }))
                .when(selected, |tab| tab.font_weight(FontWeight::SEMIBOLD))
                .hover(|tab| tab.text_color(rgb(INK)))
                .focus(|tab| tab.bg(rgb(0xf5f5f3)))
                .child(category_label(kind))
                .on_click(cx.listener(move |this, _, _, cx| this.choose_filter(kind, cx)))
        });
        div()
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(if compact { 12. } else { 20. }))
                    .children(filters),
            )
            .child(metadata().child(crate::count_label(count, "link")))
    }

    fn catalog_row(&self, entry: &Entry, compact: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let item = &entry.item;
        let id = item.id;
        let excerpt = note_excerpt(&item.notes);
        let (date, time) = item
            .capture_label
            .split_once(" · ")
            .unwrap_or((&item.capture_label, ""));
        div()
            .id(("bookmark", id as usize))
            .tab_index(0)
            .focus(|s| s.bg(rgb(0xf3f3f0)))
            .w_full()
            .h(px(if compact { 66. } else { 76. }))
            .flex()
            .items_center()
            .gap(px(if compact { 16. } else { 28. }))
            .border_b_1()
            .border_color(rgb(0xe9e9e6))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0xf8f8f6)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(if compact { 15. } else { 16. }))
                            .line_height(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(entry.title().to_owned()),
                    )
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(metadata().truncate().child(entry.host.clone()))
                            .when(compact && !excerpt.is_empty(), |s| {
                                s.child(div().text_color(rgb(0xaaa9a5)).child("·")).child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(12.))
                                        .text_color(rgb(MUTED))
                                        .child(excerpt.clone()),
                                )
                            }),
                    ),
            )
            .when(!compact, |s| {
                s.child(
                    div()
                        .w(relative(0.31))
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .text_color(rgb(MUTED))
                        .child(excerpt),
                )
            })
            .child(
                div()
                    .flex_none()
                    .w(px(88.))
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(4.))
                    .child(metadata().child(date.to_owned()))
                    .when(!time.is_empty(), |s| {
                        s.child(metadata().text_color(rgb(0x767676)).child(time.to_owned()))
                    }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.show(id, cx);
                this.focus(window)
            }))
    }
}

fn empty_catalog(no_entries: bool) -> Div {
    let title = if no_entries {
        "No saved links yet"
    } else {
        "No links in this category"
    };
    div()
        .mt(px(if no_entries { 8. } else { 24. }))
        .when(no_entries, |row| {
            row.border_t_1().border_color(rgb(LINE)).pt(px(28.))
        })
        .child(
            div()
                .text_size(px(17.))
                .font_weight(FontWeight::MEDIUM)
                .child(title),
        )
        .when(no_entries, |row| {
            row.child(div().mt_2().text_size(px(13.)).text_color(rgb(MUTED))
            .child("Paste a web address to get started. A note can go before or after the URL."))
        })
}
