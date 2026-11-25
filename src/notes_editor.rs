use crate::source::{self, byte_range};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementInputHandler, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, KeyBinding, MouseButton, Pixels, Point, TextAlign,
    TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, canvas, div, fill,
    point, prelude::*, px, rgb, size,
};
use std::collections::VecDeque;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

actions!(datadeck_notes, [Up, Down, ExtendUp, ExtendDown, Undo, Redo]);

pub fn bind_keys(cx: &mut App) {
    let context = Some("NotesEditor");
    cx.bind_keys([
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("shift-up", ExtendUp, context),
        KeyBinding::new("shift-down", ExtendDown, context),
        KeyBinding::new("secondary-z", Undo, context),
        KeyBinding::new("secondary-shift-z", Redo, context),
        KeyBinding::new("shift-enter", source::Submit, context),
    ]);
}

pub enum NotesEvent {
    Changed,
    Capture(String),
}

#[derive(Default)]
struct Buffer {
    text: String,
    anchor: usize,
    caret: usize,
    marked: Option<Range<usize>>,
    // Original contents during a macOS input-method composition. Only the final
    // transcript is a user edit, even when the IME revises its text several times.
    before_composition: Option<String>,
    undo: VecDeque<Edit>,
    redo: Vec<Edit>,
}

struct Edit {
    start: usize,
    before: String,
    after: String,
}

impl Buffer {
    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    fn select(&mut self, target: usize, extend: bool) {
        self.caret = target;
        if !extend {
            self.anchor = target;
        }
    }

    fn adjacent(&self, forward: bool) -> usize {
        if forward {
            self.text[self.caret..]
                .graphemes(true)
                .next()
                .map_or(self.caret, |g| self.caret + g.len())
        } else {
            self.text[..self.caret]
                .grapheme_indices(true)
                .next_back()
                .map_or(0, |(p, _)| p)
        }
    }

    fn utf16_range(&self, bytes: Range<usize>) -> Range<usize> {
        self.text[..bytes.start].encode_utf16().count()
            ..self.text[..bytes.end].encode_utf16().count()
    }

    fn remember(&mut self, edit: Edit) -> bool {
        if edit.before == edit.after {
            return false;
        }
        if self.undo.len() == 128 {
            self.undo.pop_front();
        }
        self.undo.push_back(edit);
        self.redo.clear();
        true
    }

    fn replace(&mut self, range: Option<Range<usize>>, value: &str, composing: bool) -> bool {
        let bytes = range
            .map(|r| byte_range(&self.text, r))
            .or(self.marked.take())
            .unwrap_or_else(|| self.selection());
        if composing && self.before_composition.is_none() {
            self.before_composition = Some(self.text.clone());
        }
        let before = self.text[bytes.clone()].to_owned();
        self.text.replace_range(bytes.clone(), value);
        self.select(bytes.start + value.len(), false);
        if composing {
            self.marked = Some(bytes.start..self.caret);
            false
        } else if self.before_composition.is_some() {
            self.finish_composition()
        } else {
            self.remember(Edit {
                start: bytes.start,
                before,
                after: value.into(),
            })
        }
    }

    fn finish_composition(&mut self) -> bool {
        self.marked = None;
        let Some(before) = self.before_composition.take() else {
            return false;
        };
        // A composition can replace a native UTF-16 range more than once.
        // Keep only the final change in undo history, even for a long note.
        let start = before
            .chars()
            .zip(self.text.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a.len_utf8())
            .sum::<usize>();
        let tail = before[start..]
            .chars()
            .rev()
            .zip(self.text[start..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a.len_utf8())
            .sum::<usize>();
        self.remember(Edit {
            start,
            before: before[start..before.len() - tail].to_owned(),
            after: self.text[start..self.text.len() - tail].to_owned(),
        })
    }

    fn history(&mut self, redo: bool) -> bool {
        self.finish_composition();
        let edit = if redo {
            self.redo.pop()
        } else {
            self.undo.pop_back()
        };
        let Some(edit) = edit else { return false };
        let (from, to) = if redo {
            (&edit.before, &edit.after)
        } else {
            (&edit.after, &edit.before)
        };
        self.text
            .replace_range(edit.start..edit.start + from.len(), to);
        self.select(edit.start + to.len(), false);
        if redo {
            self.undo.push_back(edit);
        } else {
            self.redo.push(edit);
        }
        true
    }
}

const FONT_SIZE: f32 = 15.;
const LINE_HEIGHT: f32 = 23.;
const HINT: &str = "Write or dictate a note…";

struct Paragraph {
    start: usize,
    top: Pixels,
    line: WrappedLine,
}

pub struct NotesEditor {
    buffer: Buffer,
    focus: FocusHandle,
    blur: Option<gpui::Subscription>,
    paragraphs: Vec<Paragraph>,
    bounds: Bounds<Pixels>,
    scroll: Pixels,
    max_scroll: Pixels,
    follow_caret: bool,
    dragging: bool,
    preferred_x: Option<Pixels>,
}

impl EventEmitter<NotesEvent> for NotesEditor {}

impl Focusable for NotesEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl NotesEditor {
    pub fn new(text: String, cx: &mut Context<Self>) -> Self {
        let mut editor = Self {
            buffer: Buffer::default(),
            blur: None,
            focus: cx.focus_handle().tab_stop(true),
            paragraphs: Vec::new(),
            bounds: Bounds::default(),
            scroll: px(0.),
            max_scroll: px(0.),
            follow_caret: false,
            dragging: false,
            preferred_x: None,
        };
        editor.set_text(text, cx);
        editor
    }

    pub fn value(&self) -> &str {
        &self.buffer.text
    }

    pub fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.buffer = Buffer {
            caret: text.len(),
            anchor: text.len(),
            text,
            ..Default::default()
        };
        self.paragraphs.clear();
        self.follow_caret = true;
        self.scroll = px(0.);
        self.preferred_x = None;
        cx.notify();
    }

    fn change(&mut self, changed: bool, cx: &mut Context<Self>) {
        self.follow_caret = true;
        self.preferred_x = None;
        self.paragraphs.clear();
        if changed {
            cx.emit(NotesEvent::Changed);
        }
        cx.notify();
    }

    fn insert(&mut self, range: Option<Range<usize>>, value: &str, cx: &mut Context<Self>) {
        let value = value.replace("\r\n", "\n").replace('\r', "\n");
        let changed = self.buffer.replace(range, &value, false);
        self.change(changed, cx);
    }

    fn erase(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.buffer.selection().is_empty() {
            self.buffer.anchor = self.buffer.adjacent(forward);
        }
        self.insert(None, "", cx);
    }

    fn select(&mut self, target: usize, extend: bool, cx: &mut Context<Self>) {
        self.buffer.select(target, extend);
        self.follow_caret = true;
        self.preferred_x = None;
        cx.notify();
    }

    fn horizontal(&mut self, forward: bool, extend: bool, cx: &mut Context<Self>) {
        let range = self.buffer.selection();
        let target = if range.is_empty() || extend {
            self.buffer.adjacent(forward)
        } else if forward {
            range.end
        } else {
            range.start
        };
        self.select(target, extend, cx);
    }

    // These points are relative to the full document, before viewport scrolling.
    fn position(&self, byte: usize) -> Point<Pixels> {
        let Some(p) = self.paragraphs.iter().rev().find(|p| p.start <= byte) else {
            return Point::default();
        };
        let local = byte - p.start;
        // GPUI maps a soft break to the end of its preceding visual row.
        // A caret clicked at the beginning of the next row belongs on that row.
        for (row, boundary) in p.line.wrap_boundaries().iter().enumerate() {
            if p.line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index == local {
                return point(px(0.), p.top + px(LINE_HEIGHT) * (row + 1));
            }
        }
        let offset = p
            .line
            .position_for_index(local, px(LINE_HEIGHT))
            .unwrap_or_default();
        point(offset.x, p.top + offset.y)
    }

    fn index_at(&self, p: Point<Pixels>) -> usize {
        if p.y < px(0.) {
            return 0;
        }
        let Some(row) = self
            .paragraphs
            .iter()
            .find(|row| p.y < row.top + row.line.size(px(LINE_HEIGHT)).height)
        else {
            return self.buffer.text.len();
        };
        row.start
            + row
                .line
                .closest_index_for_position(point(p.x, p.y - row.top), px(LINE_HEIGHT))
                .unwrap_or_else(|i| i)
    }

    fn hit(&self, position: Point<Pixels>) -> usize {
        self.index_at(position - self.bounds.origin + point(px(0.), self.scroll))
            .min(self.buffer.text.len())
    }

    fn vertical(&mut self, down: bool, extend: bool, cx: &mut Context<Self>) {
        if self.paragraphs.is_empty() {
            return;
        }
        let p = self.position(self.buffer.caret);
        let x = self.preferred_x.unwrap_or(p.x);
        let target = self.index_at(point(
            x,
            p.y + px(if down {
                LINE_HEIGHT * 1.5
            } else {
                -LINE_HEIGHT * 0.5
            }),
        ));
        self.select(target, extend, cx);
        self.preferred_x = Some(x);
    }

    fn line_edge(&mut self, end: bool, extend: bool, cx: &mut Context<Self>) {
        let p = self.position(self.buffer.caret);
        self.select(
            self.index_at(point(if end { px(1e7) } else { px(0.) }, p.y)),
            extend,
            cx,
        );
    }

    fn copy(&self, cx: &mut App) {
        let range = self.buffer.selection();
        if !range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.buffer.text[range].to_owned(),
            ));
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            if self.value().is_empty()
                && let Some(url) = crate::web_links::web_url(&text)
            {
                cx.emit(NotesEvent::Capture(url.into()));
            } else {
                self.insert(None, &text, cx);
            }
        }
    }
}

impl EntityInputHandler for NotesEditor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let bytes = byte_range(self.value(), range);
        *adjusted = Some(self.buffer.utf16_range(bytes.clone()));
        Some(self.buffer.text[bytes].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.utf16_range(self.buffer.selection()),
            reversed: self.buffer.anchor > self.buffer.caret,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.buffer
            .marked
            .clone()
            .map(|r| self.buffer.utf16_range(r))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let changed = self.buffer.finish_composition();
        self.change(changed, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        value: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.insert(range, value, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        value: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.buffer.replace(range, value, true);
        if let Some(marked) = &self.buffer.marked
            && let Some(selected) = selected
        {
            let selection = byte_range(value, selected);
            self.buffer.anchor = marked.start + selection.start;
            self.buffer.caret = marked.start + selection.end;
        }
        self.change(false, cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // AppKit can ask for a candidate window immediately after changing the
        // marked text, before GPUI has painted another frame.
        if self.paragraphs.is_empty() {
            self.layout(bounds, window)?;
        }
        let bytes = byte_range(self.value(), range);
        let origin = self.bounds.origin - point(px(0.), self.scroll);
        let start = self.position(bytes.start) + origin;
        let end = self.position(bytes.end) + origin;
        let width = if start.y == end.y {
            end.x - start.x
        } else {
            bounds.right() - start.x
        };
        Some(Bounds::new(start, size(width.max(px(1.)), px(LINE_HEIGHT))))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        if self.paragraphs.is_empty() {
            self.layout(self.bounds, window)?;
        }
        Some(self.value()[..self.hit(position)].encode_utf16().count())
    }
}

impl NotesEditor {
    fn layout(&mut self, bounds: Bounds<Pixels>, window: &Window) -> Option<TextRun> {
        let focus = self.focus.is_focused(window);
        let selected = self.buffer.selection();
        let marked = self.buffer.marked.clone().unwrap_or(0..0);
        let mut boundaries = [
            0,
            self.value().len(),
            selected.start,
            selected.end,
            marked.start,
            marked.end,
        ];
        boundaries.sort_unstable();
        let run = TextRun {
            len: self.value().len(),
            font: window.text_style().font(),
            color: rgb(0x242424).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = boundaries
            .windows(2)
            .filter(|pair| pair[0] < pair[1])
            .map(|pair| TextRun {
                len: pair[1] - pair[0],
                background_color: (focus && selected.contains(&pair[0]))
                    .then(|| rgb(0xd9e1e3).into()),
                underline: marked.contains(&pair[0]).then(|| UnderlineStyle {
                    thickness: px(1.),
                    ..Default::default()
                }),
                ..run.clone()
            })
            .collect::<Vec<_>>();
        let lh = px(LINE_HEIGHT);
        let width = bounds.size.width.max(px(1.));
        let lines = window
            .text_system()
            .shape_text(
                self.value().to_owned().into(),
                px(FONT_SIZE),
                &runs,
                Some(width),
                None,
            )
            .ok()?;
        self.paragraphs.clear();
        let mut start = 0;
        let mut top = px(0.);
        for line in lines {
            let end = start + line.text.len() + 1;
            let height = line.size(lh).height;
            self.paragraphs.push(Paragraph { start, top, line });
            start = end;
            top += height;
        }
        self.bounds = bounds;
        let caret = self.position(self.buffer.caret);
        self.max_scroll = (top - bounds.size.height).max(px(0.));
        if self.follow_caret && focus {
            self.scroll = self
                .scroll
                .min(caret.y)
                .max(caret.y + lh - bounds.size.height);
        }
        self.scroll = self.scroll.clamp(px(0.), self.max_scroll);
        self.follow_caret = false;
        Some(run)
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        window.handle_input(
            &self.focus,
            ElementInputHandler::new(bounds, cx.entity()),
            cx,
        );
        let Some(run) = self.layout(bounds, window) else {
            return;
        };
        let focus = self.focus.is_focused(window);
        let caret = self.position(self.buffer.caret);
        let lh = px(LINE_HEIGHT);
        let origin = bounds.origin - point(px(0.), self.scroll);
        for p in &self.paragraphs {
            let pos = origin + point(px(0.), p.top);
            if pos.y + p.line.size(lh).height <= bounds.top() || pos.y >= bounds.bottom() {
                continue;
            }
            let _ = p
                .line
                .paint_background(pos, lh, TextAlign::Left, None, window, cx);
            let _ = p.line.paint(pos, lh, TextAlign::Left, None, window, cx);
        }
        if self.value().is_empty() {
            let hint = window.text_system().shape_line(
                HINT.into(),
                px(FONT_SIZE),
                &[TextRun {
                    len: HINT.len(),
                    color: rgb(0x777777).into(),
                    ..run
                }],
                None,
            );
            let _ = hint.paint(bounds.origin, lh, window, cx);
        }
        if focus && self.buffer.selection().is_empty() {
            window.paint_quad(fill(
                Bounds::new(
                    origin + caret + point(px(0.), px(2.)),
                    size(px(1.5), lh - px(4.)),
                ),
                rgb(0x242424),
            ));
        }
    }
}

impl Render for NotesEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.blur.is_none() {
            self.blur = Some(cx.on_blur(&self.focus, window, |editor, _, cx| {
                if editor.buffer.finish_composition() {
                    cx.emit(NotesEvent::Changed);
                }
                cx.notify();
            }));
        }
        let editor = cx.entity();
        let surface = canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                editor.update(cx, |s, cx| s.paint(bounds, window, cx));
            },
        )
        .size_full();
        div()
            .id("bookmark-notes")
            .key_context("DatasetInput NotesEditor")
            .track_focus(&self.focus)
            .w_full()
            .h(px(186.))
            .flex_none()
            .p_3()
            .border_1()
            .border_color(rgb(if self.focus.is_focused(window) {
                0x626262
            } else {
                0xcacaca
            }))
            .rounded_sm()
            .bg(rgb(0xffffff))
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|s, event: &gpui::MouseDownEvent, w, cx| {
                    s.dragging = true;
                    s.focus.focus(w);
                    let target = s.hit(event.position);
                    s.select(target, event.modifiers.shift, cx);
                    if event.click_count == 2 {
                        if let Some((start, word)) = s
                            .value()
                            .split_word_bound_indices()
                            .find(|(start, w)| *start + w.len() > target)
                        {
                            let end = start + word.len();
                            s.buffer.anchor = start;
                            s.buffer.caret = end;
                        }
                    } else if event.click_count >= 3 {
                        s.buffer.anchor = s.value()[..target].rfind('\n').map_or(0, |p| p + 1);
                        s.buffer.caret = s.value()[target..]
                            .find('\n')
                            .map_or(s.value().len(), |p| target + p + 1);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|s, _, _, _| s.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|s, _, _, _| s.dragging = false),
            )
            .on_mouse_move(cx.listener(|s, event: &gpui::MouseMoveEvent, _, cx| {
                if s.dragging && event.dragging() {
                    s.select(s.hit(event.position), true, cx);
                }
            }))
            .on_scroll_wheel(cx.listener(|s, event: &gpui::ScrollWheelEvent, _, cx| {
                let scroll = (s.scroll - event.delta.pixel_delta(px(LINE_HEIGHT)).y)
                    .clamp(px(0.), s.max_scroll);
                if scroll != s.scroll {
                    s.scroll = scroll;
                    s.follow_caret = false;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|s, _: &source::Submit, _, cx| s.insert(None, "\n", cx)))
            .on_action(cx.listener(|s, _: &source::Copy, _, cx| s.copy(cx)))
            .on_action(cx.listener(|s, _: &source::Paste, _, cx| s.paste(cx)))
            .on_action(cx.listener(|s, _: &source::Cut, _, cx| {
                s.copy(cx);
                s.insert(None, "", cx);
            }))
            .on_action(cx.listener(|s, _: &source::SelectAll, _, cx| {
                s.buffer.anchor = 0;
                s.select(s.value().len(), true, cx);
            }))
            .on_action(cx.listener(|s, _: &source::Backspace, _, cx| s.erase(false, cx)))
            .on_action(cx.listener(|s, _: &source::DeleteForward, _, cx| s.erase(true, cx)))
            .on_action(cx.listener(|s, _: &source::Left, _, cx| s.horizontal(false, false, cx)))
            .on_action(cx.listener(|s, _: &source::Right, _, cx| s.horizontal(true, false, cx)))
            .on_action(
                cx.listener(|s, _: &source::ExtendLeft, _, cx| s.horizontal(false, true, cx)),
            )
            .on_action(
                cx.listener(|s, _: &source::ExtendRight, _, cx| s.horizontal(true, true, cx)),
            )
            .on_action(cx.listener(|s, _: &source::Home, _, cx| s.line_edge(false, false, cx)))
            .on_action(cx.listener(|s, _: &source::End, _, cx| s.line_edge(true, false, cx)))
            .on_action(cx.listener(|s, _: &source::ExtendHome, _, cx| s.line_edge(false, true, cx)))
            .on_action(cx.listener(|s, _: &source::ExtendEnd, _, cx| s.line_edge(true, true, cx)))
            .on_action(cx.listener(|s, _: &Up, _, cx| s.vertical(false, false, cx)))
            .on_action(cx.listener(|s, _: &Down, _, cx| s.vertical(true, false, cx)))
            .on_action(cx.listener(|s, _: &ExtendUp, _, cx| s.vertical(false, true, cx)))
            .on_action(cx.listener(|s, _: &ExtendDown, _, cx| s.vertical(true, true, cx)))
            .on_action(cx.listener(|s, _: &Undo, _, cx| {
                let changed = s.buffer.history(false);
                s.change(changed, cx);
            }))
            .on_action(cx.listener(|s, _: &Redo, _, cx| {
                let changed = s.buffer.history(true);
                s.change(changed, cx);
            }))
            .child(div().size_full().overflow_hidden().child(surface))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_dictation_replaces_a_utf16_selection_and_undo_restores_it() {
        let mut buffer = Buffer::default();
        assert!(buffer.replace(None, "🌱 old\nsecond", false));
        assert!(buffer.replace(Some(3..6), "new\nparagraph", false));
        assert_eq!(buffer.text, "🌱 new\nparagraph\nsecond");
        assert!(buffer.history(false));
        assert_eq!(buffer.text, "🌱 old\nsecond");
        assert!(buffer.history(true));
        assert_eq!(buffer.text, "🌱 new\nparagraph\nsecond");
    }

    #[test]
    fn marked_text_revisions_are_one_committed_edit() {
        let mut buffer = Buffer::default();
        buffer.replace(None, "a\n", false);
        assert!(!buffer.replace(None, "か", true));
        assert!(!buffer.replace(None, "かな", true));
        assert!(buffer.replace(None, "仮名", false));
        assert_eq!(buffer.text, "a\n仮名");
        assert!(buffer.history(false));
        assert_eq!(buffer.text, "a\n");
        buffer.replace(None, "𠮷", true);
        assert!(buffer.finish_composition());
        assert_eq!(buffer.text, "a\n𠮷");
    }
}
