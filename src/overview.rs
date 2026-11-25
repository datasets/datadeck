use std::ops::Range;
use std::sync::Arc;

use gpui::{prelude::*, *};

use crate::grid::profile::{Profile, SAMPLE_LIMIT};
use crate::theme::Theme;
use crate::{LINE, MONO, MUTED, number};

pub enum OverviewEvent {
    Select(usize),
}

pub struct Overview {
    columns: Vec<String>,
    profiles: Vec<Profile>,
    sampled: usize,
    total: usize,
    size: f32,
    theme: Theme,
    worker: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    current: usize,
}

impl EventEmitter<OverviewEvent> for Overview {}

fn summarize(
    batches: &[Arc<Vec<Vec<String>>>],
    width: usize,
    total: usize,
    cancelled: impl Fn() -> bool,
) -> Option<(usize, Vec<Profile>)> {
    let rows: Vec<_> = batches
        .iter()
        .flat_map(|b| b.iter())
        .take(SAMPLE_LIMIT.min(total))
        .collect();
    let mut profiles = Vec::with_capacity(width);
    for index in 0..width {
        if cancelled() {
            return None;
        }
        profiles.push(Profile::read(rows.iter().copied(), index));
    }
    Some((rows.len(), profiles))
}

impl Overview {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            columns: vec![],
            profiles: vec![],
            sampled: 0,
            total: 0,
            size: crate::grid::DEFAULT_FONT_SIZE,
            theme: Theme::default(),
            worker: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            current: 0,
        }
    }

    pub fn set_appearance(&mut self, size: f32, theme: Theme, cx: &mut Context<Self>) {
        self.size = size.clamp(9., 20.);
        self.theme = theme;
        cx.notify();
    }

    pub fn show(
        &mut self,
        columns: Vec<String>,
        sample: Vec<Arc<Vec<Vec<String>>>>,
        total_rows: usize,
        size: f32,
        theme: Theme,
        cx: &mut Context<Self>,
    ) {
        self.worker = None;
        self.profiles.clear();
        self.columns = columns;
        self.total = total_rows;
        self.sampled = 0;
        self.scroll = UniformListScrollHandle::new();
        self.current = 0;
        self.set_appearance(size, theme, cx);
        let width = self.columns.len();
        if width == 0 {
            return;
        }
        let (tx, rx) = async_channel::bounded(1);
        cx.background_executor()
            .spawn(async move {
                if let Some(summary) = summarize(&sample, width, total_rows, || tx.is_closed()) {
                    let _ = tx.send(summary).await;
                }
            })
            .detach();
        // Replacing this task drops its receiver. The old analysis stops at the next column.
        self.worker = Some(cx.spawn(async move |this, cx| {
            if let Ok((sampled, profiles)) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.sampled = sampled;
                    this.profiles = profiles;
                    cx.notify();
                });
            }
        }));
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(last) = self.profiles.len().checked_sub(1) else {
            return;
        };
        let mods = event.keystroke.modifiers;
        if mods.control || mods.platform || mods.alt || mods.shift {
            return;
        }
        match event.keystroke.key.as_str() {
            "up" => self.current = self.current.saturating_sub(1),
            "down" => self.current = (self.current + 1).min(last),
            "home" => self.current = 0,
            "end" => self.current = last,
            "enter" => cx.emit(OverviewEvent::Select(self.current)),
            _ => return,
        }
        self.scroll
            .scroll_to_item(self.current, ScrollStrategy::Center);
        cx.stop_propagation();
        cx.notify();
    }

    fn field_cell(&self) -> Div {
        div().flex_1().min_w_0().pr_2().truncate()
    }

    fn type_cell(&self) -> Div {
        div().w(px(self.size * 5.2)).flex_none().truncate()
    }

    fn count_cell(&self) -> Div {
        div()
            .w(px(self.size * 4.8))
            .flex_none()
            .text_right()
            .truncate()
    }

    fn distribution_cell(&self) -> Div {
        div().flex_1().min_w_0().pl_3().truncate()
    }

    fn row(&self, index: usize, focused: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let profile = &self.profiles[index];
        let mut distribution = self.distribution_cell();
        let kind = if let Some(histogram) = &profile.distribution {
            let peak = histogram.bins.iter().copied().max().unwrap_or(1).max(1);
            let height = self.size * 1.4;
            let bars = div()
                .flex()
                .items_end()
                .h(px(height))
                .w_full()
                .gap(px(1.))
                .children(histogram.bins.iter().map(|count| {
                    div()
                        .flex_1()
                        .h(px(height * *count as f32 / peak as f32))
                        .bg(rgb(self.theme.chart(1)))
                }));
            distribution = distribution.child(bars).child(
                div()
                    .flex()
                    .justify_between()
                    .gap_2()
                    .font_family(MONO)
                    .text_size(px(self.size))
                    .text_color(rgb(MUTED))
                    .child(div().truncate().child(histogram.min.clone()))
                    .child(div().truncate().child(histogram.max.clone())),
            );
            "Number"
        } else if let Some((label, count)) = profile.common.first() {
            let fraction = *count as f32 / (profile.rows - profile.missing).max(1) as f32;
            distribution = distribution
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().min_w_0().flex_1().truncate().child(label.clone()))
                        .child(div().font_family(MONO).flex_none().child(number(*count))),
                )
                .child(
                    div().w_full().h(px(3.)).mt(px(3.)).bg(rgb(0xeeeeee)).child(
                        div()
                            .h_full()
                            .w(relative(fraction))
                            .bg(rgb(self.theme.chart(0))),
                    ),
                );
            "Text"
        } else {
            distribution = distribution.child("—").text_color(rgb(MUTED));
            "Empty"
        };
        div()
            .id(("overview-column", index))
            .w_full()
            .h(px(3.6 * self.size + 8.))
            .flex()
            .gap_2()
            .px_4()
            .items_center()
            .cursor_pointer()
            .border_b_1()
            .border_color(rgb(0xeeeeee))
            .when(focused && self.current == index, |style| {
                style
                    .bg(rgb(self.theme.selection_fill()))
                    .border_l_2()
                    .border_color(rgb(self.theme.selection_rule()))
            })
            .hover(|style| style.bg(rgb(self.theme.selection_fill())))
            .child(
                self.field_cell()
                    .font_family(MONO)
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.columns[index].clone()),
            )
            .child(self.type_cell().text_color(rgb(MUTED)).child(kind))
            .child(
                self.count_cell()
                    .font_family(MONO)
                    .child(number(profile.distinct)),
            )
            .child(
                self.count_cell()
                    .font_family(MONO)
                    .child(number(profile.missing)),
            )
            .child(distribution)
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(OverviewEvent::Select(index))))
    }
}

impl Focusable for Overview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Overview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let loading = !self.columns.is_empty() && self.profiles.is_empty();
        let scope = if self.columns.is_empty() {
            "No columns in this table".to_owned()
        } else if loading {
            "Profiling columns…".to_owned()
        } else if self.total == 0 {
            "No rows in this table".to_owned()
        } else if self.sampled < self.total {
            format!(
                "First {} of {} rows",
                number(self.sampled),
                number(self.total)
            )
        } else {
            format!(
                "All {} {}",
                number(self.sampled),
                if self.sampled == 1 { "row" } else { "rows" }
            )
        };
        let header = div()
            .flex()
            .items_center()
            .px_4()
            .py_2()
            .gap_2()
            .text_color(rgb(MUTED))
            .font_weight(FontWeight::MEDIUM)
            .border_b_1()
            .border_color(rgb(LINE))
            .child(self.field_cell().child("Column"))
            .child(self.type_cell().child("Type"))
            .child(self.count_cell().child("Distinct"))
            .child(self.count_cell().child("Missing"))
            .child(self.distribution_cell().child("Distribution"));
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .min_h_0()
            .flex()
            .flex_col()
            .text_size(px(self.size))
            .child(
                div()
                    .px_4()
                    .py_2()
                    .flex()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(self.size + 3.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Column overview"),
                    )
                    .child(div().text_color(rgb(MUTED)).child(scope)),
            )
            .when(!self.profiles.is_empty(), |root| {
                root.child(header).child(
                    uniform_list(
                        "overview-rows",
                        self.profiles.len(),
                        cx.processor(move |this, indices: Range<usize>, _, cx| {
                            indices.map(|index| this.row(index, focused, cx)).collect()
                        }),
                    )
                    .track_scroll(self.scroll.clone())
                    .flex_1()
                    .min_h_0()
                    .w_full(),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{SAMPLE_LIMIT, summarize};
    use std::sync::Arc;

    #[test]
    fn wide_ragged_data_uses_one_shared_sample_across_batches() {
        let mut first = vec![vec!["1".to_owned(); 2]; SAMPLE_LIMIT - 1];
        first[0].pop();
        let batches = vec![
            Arc::new(first),
            Arc::new(vec![vec!["2".into(), "x".into()], vec!["ignored".into()]]),
        ];
        let (n, profiles) = summarize(&batches, 3, SAMPLE_LIMIT + 1, || false).unwrap();
        assert_eq!(n, SAMPLE_LIMIT);
        assert_eq!(profiles[0].distinct, 2);
        assert!(profiles[0].distribution.is_some());
        assert_eq!(profiles[1].missing, 1);
        assert!(profiles[1].distribution.is_none());
        assert_eq!(profiles[2].missing, SAMPLE_LIMIT);
    }

    #[test]
    fn handles_empty_or_cancelled_overview() {
        let (n, profiles) = summarize(&[], 2, 0, || false).unwrap();
        assert_eq!(n, 0);
        assert_eq!(profiles[1].distinct, 0);
        assert!(summarize(&[], 1, 0, || true).is_none());
        assert!(summarize(&[], 0, 0, || false).unwrap().1.is_empty());
    }
}
