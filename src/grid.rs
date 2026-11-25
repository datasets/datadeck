pub(crate) mod profile;
mod rail;
mod selection;

use crate::theme::Theme;
use gpui::{FontWeight, prelude::*, *};
use profile::{Profile, SAMPLE_LIMIT, numeric};
use selection::{Cell, Rectangle};
use std::{
    ops::{Index, Range},
    sync::Arc,
};

#[derive(Default)]
struct Rows {
    batches: Vec<(usize, Arc<Vec<Vec<String>>>)>,
    len: usize,
}

impl Rows {
    fn len(&self) -> usize {
        self.len
    }
    fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn append(&mut self, batch: Arc<Vec<Vec<String>>>) {
        if !batch.is_empty() {
            let start = self.len;
            self.len += batch.len();
            self.batches.push((start, batch));
        }
    }
    fn iter(&self) -> impl Iterator<Item = &Vec<String>> {
        self.batches.iter().flat_map(|(_, rows)| rows.iter())
    }
}

impl Index<usize> for Rows {
    type Output = Vec<String>;
    fn index(&self, i: usize) -> &Self::Output {
        let batch = self.batches.partition_point(|(start, _)| *start <= i) - 1;
        let (start, rows) = &self.batches[batch];
        &rows[i - start]
    }
}

const PAPER: u32 = 0xffffff;
const MUTED: u32 = 0x686868;
const RULE: u32 = 0xeeeeee;
const MONO: &str = "IBM Plex Mono";
pub const DEFAULT_FONT_SIZE: f32 = 11.;
pub type RowBatch = Arc<Vec<Vec<String>>>;
const WIDTH_SAMPLE: usize = 32;

/// View state for the same source across a background reload.
#[derive(Clone)]
pub struct Bookmark {
    column: Option<String>,
    row: Option<usize>,
    anchor: Option<(usize, String)>,
    inspector: bool,
    left: Pixels,
    top: Pixels,
}

pub struct Grid {
    columns: Vec<String>,
    font_size: f32,
    theme: Theme,
    rows: Rows,
    // Column boundaries include the row-number gutter.
    edges: Vec<f32>,
    numeric_columns: Vec<bool>,
    column: usize,
    row: Option<usize>,
    anchor: Option<Cell>,
    inspector: bool,
    interacted: bool,
    profile: Profile,
    hovered_bin: Option<usize>,
    focus: FocusHandle,
    drag: rail::Drag,
    horizontal: ScrollHandle,
    vertical: UniformListScrollHandle,
}

impl Grid {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            columns: Vec::new(),
            font_size: DEFAULT_FONT_SIZE,
            theme: Theme::default(),
            rows: Rows::default(),
            edges: vec![Self::gutter_width(DEFAULT_FONT_SIZE)],
            numeric_columns: vec![],
            column: 0,
            row: None,
            anchor: None,
            inspector: true,
            interacted: false,
            profile: Profile::default(),
            hovered_bin: None,
            focus: cx.focus_handle().tab_stop(true),
            drag: Default::default(),
            horizontal: ScrollHandle::new(),
            vertical: UniformListScrollHandle::new(),
        }
    }

    fn gutter_width(font_size: f32) -> f32 {
        (font_size * 4.8).ceil()
    }

    fn row_height(&self) -> f32 {
        (self.font_size * 2. + 2.).round()
    }

    fn header_height(&self) -> f32 {
        self.row_height() + 4.
    }

    pub fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        if !size.is_finite() {
            return;
        }
        let size = size.clamp(9., 20.);
        if self.font_size == size {
            return;
        }
        // Preserve the first visible row and approximately the same horizontal position.
        let old_height = self.row_height();
        let width_ratio = size / self.font_size;
        self.font_size = size;
        self.measure();
        let scroll = self.vertical.0.borrow().base_handle.clone();
        let old = scroll.offset();
        scroll.set_offset(point(old.x, old.y * (self.row_height() / old_height)));
        let old = self.horizontal.offset();
        self.horizontal
            .set_offset(point(old.x * width_ratio, old.y));
        cx.notify();
    }

    pub fn inspector_open(&self) -> bool {
        self.inspector
    }

    pub fn set_inspector(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.inspector != open {
            self.inspector = open;
            cx.notify();
        }
    }

    pub fn inspect_column(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.columns.len() {
            self.inspector = true;
            self.select(index, None, false, window, cx);
            self.reveal_column(index);
        }
    }

    /// The overview profiles at most SAMPLE_LIMIT rows. Share only batches intersecting
    /// that prefix; callers truncate the final batch at the sample boundary.
    pub fn overview_data(&self) -> (Vec<String>, Vec<RowBatch>, usize) {
        let batches = self
            .rows
            .batches
            .iter()
            .take_while(|(start, _)| *start < SAMPLE_LIMIT)
            .map(|(_, batch)| batch.clone())
            .collect();
        (self.columns.clone(), batches, self.rows.len())
    }

    pub fn bookmark(&self) -> Bookmark {
        Bookmark {
            column: self.columns.get(self.column).cloned(),
            row: self.row,
            anchor: self
                .anchor
                .map(|cell| (cell.row, self.columns[cell.column].clone())),
            inspector: self.inspector,
            left: -self.horizontal.offset().x,
            top: -self.vertical.0.borrow().base_handle.offset().y,
        }
    }

    pub fn restore(&mut self, book: &Bookmark, cx: &mut Context<Self>) {
        // Prefer any navigation the user performed while the new rows were streaming.
        if self.interacted {
            return;
        }
        let Some(column) = book
            .column
            .as_ref()
            .and_then(|name| self.columns.iter().position(|c| c == name))
        else {
            return;
        };
        self.column = column;
        self.row = book
            .row
            .and_then(|row| self.rows.len().checked_sub(1).map(|last| row.min(last)));
        self.anchor = book.anchor.as_ref().and_then(|(row, name)| {
            Some(Cell {
                column: self.columns.iter().position(|c| c == name)?,
                row: (*row).min(self.rows.len().checked_sub(1)?),
            })
        });
        self.inspector = book.inspector;
        self.profile_column();
        let vertical = self.vertical.0.borrow().base_handle.clone();
        let max_top =
            (px(self.row_height()) * self.rows.len() - vertical.bounds().size.height).max(px(0.));
        let max_left = (px(self.edges.last().copied().unwrap_or_default())
            - self.horizontal.bounds().size.width)
            .max(px(0.));
        // GPUI will apply the precise viewport bounds on its next layout if the view
        // has not been painted yet (a short reload can finish within a single frame).
        self.horizontal
            .set_offset(point(-book.left.clamp(px(0.), max_left), px(0.)));
        vertical.set_offset(point(px(0.), -book.top.clamp(px(0.), max_top)));
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        if self.theme != theme {
            self.theme = theme;
            cx.notify();
        }
    }

    pub fn reset(&mut self, columns: Vec<String>, cx: &mut Context<Self>) {
        self.columns = columns;
        self.rows = Rows::default();
        self.column = 0;
        self.row = None;
        self.anchor = None;
        self.interacted = false;
        self.profile = Profile::default();
        self.hovered_bin = None;
        self.horizontal = ScrollHandle::new();
        self.vertical = UniformListScrollHandle::new();
        self.measure();
        cx.notify();
    }

    /// A later CSV record may be wider than its header or the initial preview.
    pub fn widen(&mut self, columns: Vec<String>, cx: &mut Context<Self>) {
        if columns.len() <= self.columns.len() {
            return;
        }
        self.columns = columns;
        self.measure();
        cx.notify();
    }

    pub fn append(&mut self, rows: Arc<Vec<Vec<String>>>, cx: &mut Context<Self>) {
        let previous = self.rows.len();
        self.rows.append(rows);
        if previous < WIDTH_SAMPLE {
            self.measure();
            if !self.interacted {
                self.column = self
                    .numeric_columns
                    .iter()
                    .enumerate()
                    .find_map(|(index, numeric)| {
                        let name = self.columns[index].to_lowercase();
                        (*numeric && name != "id" && !name.ends_with("_id")).then_some(index)
                    })
                    .unwrap_or(0);
            }
        }
        if previous < SAMPLE_LIMIT {
            self.profile_column();
        }
        cx.notify();
    }

    fn measure(&mut self) {
        self.edges.clear();
        self.edges.push(Self::gutter_width(self.font_size));
        self.numeric_columns.clear();
        for (index, name) in self.columns.iter().enumerate() {
            let mut length = name.chars().take(33).count();
            let mut has_value = false;
            let mut all_numeric = true;
            for value in self
                .rows
                .iter()
                .take(WIDTH_SAMPLE)
                .filter_map(|r| r.get(index))
                .filter(|v| !v.is_empty())
            {
                length = length.max(value.chars().take(33).count());
                has_value = true;
                all_numeric &= numeric(value).is_some();
            }
            let width = (self.font_size * (2. + 0.6 * length as f32))
                .clamp(8. * self.font_size, 22. * self.font_size)
                .ceil();
            self.edges.push(
                self.edges
                    .last()
                    .copied()
                    .unwrap_or(Self::gutter_width(self.font_size))
                    + width,
            );
            self.numeric_columns.push(has_value && all_numeric);
        }
    }

    fn profile_column(&mut self) {
        self.hovered_bin = None;
        self.profile = Profile::read(self.rows.iter().take(SAMPLE_LIMIT), self.column);
    }

    fn rectangle(&self) -> Option<Rectangle> {
        let cursor = Cell {
            row: self.row?,
            column: self.column,
        };
        Some(Rectangle::between(self.anchor.unwrap_or(cursor), cursor))
    }

    fn select(
        &mut self,
        column: usize,
        row: Option<usize>,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.interacted = true;
        if row.is_some() && extend {
            self.anchor = self.anchor.or_else(|| {
                self.row.map(|prev| Cell {
                    row: prev,
                    column: self.column,
                })
            });
        } else {
            self.anchor = None;
        }
        if column != self.column {
            self.column = column;
            self.profile_column();
        }
        self.row = row;
        self.focus.focus(window);
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.columns.is_empty() {
            return;
        }
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        if (mods.platform || mods.control) && key == "c" {
            if let Some(rect) = self.rectangle() {
                cx.write_to_clipboard(ClipboardItem::new_string(rect.text(&self.rows)));
            }
            cx.stop_propagation();
            return;
        }
        let extend = mods.shift && matches!(key, "up" | "down" | "left" | "right");
        if mods.control || mods.platform || mods.alt || mods.function || (mods.shift && !extend) {
            return;
        }
        if matches!(key, "enter" | "i" | "escape") {
            self.inspector = key != "escape" && !self.inspector;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let mut col = self.column;
        let mut row = self.row.unwrap_or(0);
        let last = self.rows.len().saturating_sub(1);
        let page = (f32::from(self.vertical.0.borrow().base_handle.bounds().size.height)
            / self.row_height())
        .floor()
        .max(1.) as usize;
        match key {
            "left" => col = col.saturating_sub(1),
            "right" => col = (col + 1).min(self.columns.len() - 1),
            "up" => row = row.saturating_sub(1),
            "down" => row = self.row.map_or(0, |r| (r + 1).min(last)),
            "pageup" => row = row.saturating_sub(page),
            "pagedown" => row = (row + page).min(last),
            "home" => row = 0,
            "end" => row = last,
            _ => return,
        }
        self.select(
            col,
            (!self.rows.is_empty()).then_some(row),
            extend,
            window,
            cx,
        );
        self.vertical.scroll_to_item(row, ScrollStrategy::Center);
        self.reveal_column(col);
        cx.stop_propagation();
    }

    fn reveal_column(&self, col: usize) {
        let left = -f32::from(self.horizontal.offset().x);
        let width = f32::from(self.horizontal.bounds().size.width);
        let target = if width <= 0. || self.edges[col] < left {
            self.edges[col]
        } else {
            left.max(self.edges[col + 1] - width)
        };
        self.horizontal
            .set_offset(point(px(-target.max(0.)), px(0.)));
    }

    fn visible_columns(&self, viewport: f32) -> Range<usize> {
        let left = -f32::from(self.horizontal.offset().x);
        let start = self.edges.partition_point(|x| *x < left).saturating_sub(1);
        let end = self.edges.partition_point(|x| *x < left + viewport + 300.);
        start.min(self.columns.len())..end.min(self.columns.len())
    }

    fn gutter(&self, text: String) -> Div {
        div()
            .flex()
            .items_center()
            .justify_end()
            .pr(px(self.font_size))
            .w(px(Self::gutter_width(self.font_size)))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(rgb(RULE))
            .text_size(px((self.font_size - 1.).max(9.)))
            .text_color(rgb(MUTED))
            .child(text)
    }

    fn line(&self, row: Option<usize>, columns: Range<usize>, cx: &mut Context<Self>) -> Div {
        let header = row.is_none();
        let mut line = div()
            .flex()
            .items_center()
            .w_full()
            .flex_shrink_0()
            .h(px(if header {
                self.header_height()
            } else {
                self.row_height()
            }))
            .border_b_1()
            .border_color(rgb(RULE))
            .when(header, |el| el.bg(rgb(0xf8f8f8)));
        if columns.start == 0 {
            line = line.child(self.gutter(row.map_or_else(|| "#".into(), |i| (i + 1).to_string())));
        } else {
            line = line.child(div().flex_shrink_0().w(px(self.edges[columns.start])));
        }
        let selection = self.rectangle().filter(|rect| !rect.single());
        for col in columns {
            let highlighted = self.column == col && (self.inspector || self.row.is_some());
            let active = row.is_some() && highlighted && self.row == row;
            let value = match row {
                None => self.columns[col].as_str(),
                Some(i) => self.rows[i]
                    .get(col)
                    .map(String::as_str)
                    .unwrap_or_default(),
            };
            let empty = value.is_empty();
            let label = if empty && !header {
                "∅".to_owned()
            } else {
                value
                    .chars()
                    .take(120)
                    .collect::<String>()
                    .replace('\n', "↵")
                    .replace('\r', "")
            };
            let id = if header { ("head", col) } else { ("cell", col) };
            let cell = div()
                .id(id)
                .flex()
                .items_center()
                .overflow_hidden()
                .h_full()
                .flex_shrink_0()
                .w(px(self.edges[col + 1] - self.edges[col]))
                .px(px(self.font_size))
                .text_size(px(self.font_size))
                .when(header, |el| {
                    el.font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .hover(|el| el.bg(rgb(0xebebeb)))
                })
                .when(self.numeric_columns[col], |el| el.justify_end())
                .when(highlighted, |el| {
                    let fill = Hsla::from(rgb(self.theme.selection_fill()));
                    el.bg(if header || active {
                        fill
                    } else {
                        fill.opacity(0.65)
                    })
                })
                .when(highlighted && header, |el| {
                    el.border_b_1()
                        .border_color(rgb(self.theme.selection_rule()))
                })
                .when_some(
                    row.and_then(|r| {
                        selection
                            .filter(|rect| rect.contains(r, col))
                            .map(|rect| (r, rect))
                    }),
                    |el, (r, rect)| {
                        el.bg(Hsla::from(rgb(self.theme.selection_rule())).opacity(0.14))
                            .border_color(rgb(self.theme.selection_rule()))
                            .when(r == rect.top, |el| el.border_t_1())
                            .when(r == rect.bottom, |el| el.border_b_1())
                            .when(col == rect.left, |el| el.border_l_1())
                            .when(col == rect.right, |el| el.border_r_1())
                    },
                )
                .when(active, |el| {
                    el.border_1().border_color(rgb(self.theme.selection_rule()))
                })
                .when(empty, |el| el.text_color(rgb(MUTED)))
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if header {
                        this.inspector = true;
                    }
                    this.select(col, row, event.modifiers().shift, window, cx);
                }))
                .child(div().truncate().child(label));
            line = line.child(cell);
        }
        line
    }

    fn inspector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let profile = &self.profile;
        let title = |label: &'static str| {
            div()
                .text_color(rgb(MUTED))
                .text_size(px(self.font_size))
                .font_weight(FontWeight::MEDIUM)
                .child(label)
        };
        let metric = |label: &'static str, value: String| {
            div()
                .flex()
                .justify_between()
                .items_center()
                .py(px(6.))
                .border_b_1()
                .border_color(rgb(RULE))
                .child(label)
                .child(div().font_family(MONO).child(value))
        };
        let mut panel = div()
            .id("inspector")
            .overflow_y_scroll()
            .h_full()
            .flex_shrink_0()
            .w(px((self.font_size * 14. + 56.).round()))
            .bg(rgb(0xfcfcfc))
            .border_l_1()
            .border_color(rgb(0xe3e3e3))
            .px(px(15.))
            .py(px(12.))
            .text_size(px(self.font_size))
            .text_color(rgb(self.theme.ink()))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(title("Column inspector"))
                    .child(
                        div()
                            .id("hide-inspector")
                            .cursor_pointer()
                            .px(px(6.))
                            .text_size(px(self.font_size + 5.))
                            .text_color(rgb(MUTED))
                            .hover(|el| el.bg(rgb(RULE)))
                            .child("×")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.inspector = false;
                                this.focus.focus(window);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .mt(px(14.))
                    .font_family(MONO)
                    .text_size(px(self.font_size + 4.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.columns[self.column].clone()),
            )
            .child(
                div()
                    .mt(px(3.))
                    .text_color(rgb(MUTED))
                    .text_size(px(self.font_size))
                    .child({
                        let kind = if profile.rows == profile.missing {
                            "Empty"
                        } else if profile.distribution.is_some() {
                            "Numeric"
                        } else {
                            "Text"
                        };
                        if self.rows.len() > profile.rows {
                            format!("{kind} · First {} rows", profile.rows)
                        } else {
                            format!("{kind} · {} rows", profile.rows)
                        }
                    }),
            )
            .child(
                div()
                    .mt(px(14.))
                    .child(metric("Distinct values", profile.distinct.to_string()))
                    .child(metric(
                        "Empty cells",
                        if profile.rows == 0 {
                            "0".into()
                        } else {
                            format!(
                                "{} ({:.1}%)",
                                profile.missing,
                                100. * profile.missing as f32 / profile.rows as f32
                            )
                        },
                    )),
            );
        if let Some(distribution) = &profile.distribution {
            let peak = (*distribution.bins.iter().max().unwrap_or(&1)).max(1);
            panel = panel.child(
                div()
                    .mt(px(18.))
                    .child(title("Distribution"))
                    .child(
                        div()
                            .mt(px(8.))
                            .h(px(58.))
                            .flex()
                            .items_end()
                            .gap(px(2.))
                            .border_b_1()
                            .border_color(rgb(0xd9d9d9))
                            .children(distribution.bins.iter().enumerate().map(|(bin, &count)| {
                                div()
                                    .id(("bin", bin))
                                    .h_full()
                                    .flex_1()
                                    .flex()
                                    .items_end()
                                    .on_hover(cx.listener(move |this, entered, _, cx| {
                                        if *entered {
                                            this.hovered_bin = Some(bin);
                                        } else if this.hovered_bin == Some(bin) {
                                            this.hovered_bin = None;
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .w_full()
                                            .bg(rgb(self.theme.chart(0)))
                                            .when(
                                                self.hovered_bin.is_some_and(|i| i != bin),
                                                |bar| bar.opacity(0.55),
                                            )
                                            .h(px(if count == 0 {
                                                0.
                                            } else {
                                                (53. * count as f32 / peak as f32).max(2.)
                                            })),
                                    )
                            })),
                    )
                    .child(
                        div()
                            .mt(px(5.))
                            .flex()
                            .justify_between()
                            .gap_2()
                            .font_family(MONO)
                            .text_size(px((self.font_size - 1.).max(9.)))
                            .when_some(self.hovered_bin, |line, bin| {
                                line.child(div().truncate().child(distribution.labels[bin].clone()))
                            })
                            .when(self.hovered_bin.is_none(), |line| {
                                line.child(div().truncate().child(distribution.min.clone()))
                                    .child(div().truncate().child(distribution.max.clone()))
                            }),
                    ),
            );
        } else if !profile.common.is_empty() {
            let total = (profile.rows - profile.missing).max(1);
            panel = panel.child(div().mt(px(18.)).child(title("Most frequent")).children(
                profile.common.iter().enumerate().map(|(i, (name, count))| {
                    div()
                        .mt(px(13.))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .gap_2()
                                .text_size(px(self.font_size))
                                .font_family(MONO)
                                .child(div().truncate().child(name.clone()))
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_color(rgb(MUTED))
                                        .child(count.to_string()),
                                ),
                        )
                        .child(
                            div().mt(px(5.)).w_full().h(px(3.)).bg(rgb(0xebebeb)).child(
                                div()
                                    .h_full()
                                    .bg(rgb(self.theme.chart(i)))
                                    .w(relative(*count as f32 / total as f32)),
                            ),
                        )
                }),
            ));
        }
        if let Some(row) = self.row {
            let value = self.rows[row]
                .get(self.column)
                .map(String::as_str)
                .unwrap_or_default();
            // Guard the inspector layout for cells containing large free-text fields.
            let mut chars = value.chars();
            let mut preview: String = chars.by_ref().take(1024).collect();
            if chars.next().is_some() {
                preview.push('…');
            }
            if value.is_empty() {
                preview.push('∅');
            }
            panel = panel.child(
                div()
                    .mt(px(25.))
                    .pt(px(12.))
                    .border_t_1()
                    .border_color(rgb(RULE))
                    .child(
                        div()
                            .text_size(px(self.font_size))
                            .text_color(rgb(MUTED))
                            .child(format!("Row {}", row + 1)),
                    )
                    .child(
                        div()
                            .mt(px(6.))
                            .font_family(MONO)
                            .text_size(px(self.font_size))
                            .child(preview),
                    ),
            );
        }
        panel
    }
}

impl Focusable for Grid {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Grid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = self.visible_columns(f32::from(window.viewport_size().width));
        let width = self
            .edges
            .last()
            .copied()
            .unwrap_or(Self::gutter_width(self.font_size));
        let header = self.line(None, columns.clone(), cx);
        let body = uniform_list(
            "dataset-rows",
            self.rows.len(),
            cx.processor(move |this, range: Range<usize>, _, cx| {
                range
                    .map(|i| {
                        div()
                            .id(("row", i))
                            .child(this.line(Some(i), columns.clone(), cx))
                    })
                    .collect()
            }),
        )
        .track_scroll(self.vertical.clone())
        .flex_1()
        .min_h_0()
        .w_full();
        let mut table = div()
            .id("dataset-horizontal")
            .min_w_0()
            .flex_1()
            .h_full()
            .track_scroll(&self.horizontal)
            .overflow_x_scroll()
            .font_family(MONO)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_full()
                    .w(px(width))
                    .h_full()
                    .child(header)
                    .child(body),
            );
        if self.rows.is_empty() && !self.columns.is_empty() {
            table = table.child(
                div()
                    .absolute()
                    .top(px(60.))
                    .left(px(20.))
                    .text_color(rgb(MUTED))
                    .child("No rows to display"),
            );
        }
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .relative()
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .overflow_hidden()
            .bg(rgb(PAPER))
            .text_color(rgb(self.theme.ink()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .h_full()
                    .child(
                        div().flex().min_h_0().flex_1().child(table).child(
                            rail::scrollbar(
                                Axis::Vertical,
                                self.vertical.0.borrow().base_handle.clone(),
                                self.drag.clone(),
                                cx.entity_id(),
                            )
                            .mt(px(self.header_height())),
                        ),
                    )
                    .child(rail::scrollbar(
                        Axis::Horizontal,
                        self.horizontal.clone(),
                        self.drag.clone(),
                        cx.entity_id(),
                    )),
            )
            .when(self.inspector && !self.columns.is_empty(), |el| {
                el.child(self.inspector(cx))
            })
    }
}
