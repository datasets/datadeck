use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementInputHandler, EntityInputHandler,
    EventEmitter, FocusHandle, KeyBinding, MouseButton, PathPromptOptions, Pixels, Point,
    ShapedLine, SharedString, Task, TextRun, UTF16Selection, Window, actions, canvas, div, fill,
    point, prelude::*, px, rgb, size,
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub fn pick_file(cx: &App) -> Task<Result<Option<String>, String>> {
    pick_path(cx, false)
}

pub fn pick_folder(cx: &App) -> Task<Result<Option<String>, String>> {
    pick_path(cx, true)
}

fn pick_path(cx: &App, folder: bool) -> Task<Result<Option<String>, String>> {
    let selection = cx.prompt_for_paths(PathPromptOptions {
        files: !folder,
        directories: folder,
        multiple: false,
        prompt: Some(
            if folder {
                "Open folder"
            } else {
                "Open dataset"
            }
            .into(),
        ),
    });
    cx.background_executor().spawn(async move {
        let paths = selection
            .await
            .map_err(|err| err.to_string())?
            .map_err(|err| err.to_string())?;
        paths
            .and_then(|mut entries| entries.pop())
            .map(|path| {
                path.into_os_string()
                    .into_string()
                    .map_err(|_| "The path contains non-UTF-8 characters.".to_owned())
            })
            .transpose()
    })
}

actions!(
    datadeck_editor,
    [
        Submit,
        Dismiss,
        Paste,
        Copy,
        Cut,
        SelectAll,
        Backspace,
        DeleteForward,
        Left,
        Right,
        ExtendLeft,
        ExtendRight,
        Home,
        End,
        ExtendHome,
        ExtendEnd
    ]
);

pub fn bind_keys(cx: &mut App) {
    let context = Some("DatasetInput");
    let shortcut = |mac: &'static str, other: &'static str| {
        if cfg!(target_os = "macos") {
            mac
        } else {
            other
        }
    };
    cx.bind_keys([
        KeyBinding::new("enter", Submit, context),
        KeyBinding::new("escape", Dismiss, context),
        KeyBinding::new(shortcut("cmd-a", "ctrl-a"), SelectAll, context),
        KeyBinding::new(shortcut("cmd-v", "ctrl-v"), Paste, context),
        KeyBinding::new(shortcut("cmd-c", "ctrl-c"), Copy, context),
        KeyBinding::new(shortcut("cmd-x", "ctrl-x"), Cut, context),
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", DeleteForward, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("shift-left", ExtendLeft, context),
        KeyBinding::new("shift-right", ExtendRight, context),
        KeyBinding::new("home", Home, context),
        KeyBinding::new("end", End, context),
        KeyBinding::new("shift-home", ExtendHome, context),
        KeyBinding::new("shift-end", ExtendEnd, context),
        KeyBinding::new("cmd-left", Home, context),
        KeyBinding::new("cmd-right", End, context),
    ]);
}

pub enum UrlEvent {
    Open(String),
    Cancel,
}

#[derive(Clone, Copy, PartialEq)]
enum Purpose {
    Url,
    Query,
    Field(&'static str),
    Capture,
}

pub struct UrlInput {
    purpose: Purpose,
    focus: FocusHandle,
    text: String,
    anchor: usize,
    caret: usize,
    marked: Option<Range<usize>>,
    layout: Option<(Point<Pixels>, ShapedLine)>,
    scroll_x: Pixels,
    error: Option<&'static str>,
}

impl EventEmitter<UrlEvent> for UrlInput {}

/// Platform input uses UTF-16 code units. Rust string slicing requires scalar boundaries.
fn byte_offset(text: &str, utf16: usize, round_up: bool) -> usize {
    let mut count = 0;
    for (index, ch) in text.char_indices() {
        if utf16 == count {
            return index;
        }
        count += ch.len_utf16();
        if utf16 < count {
            return if round_up {
                index + ch.len_utf8()
            } else {
                index
            };
        }
    }
    text.len()
}

pub(crate) fn byte_range(text: &str, range: Range<usize>) -> Range<usize> {
    let start = byte_offset(text, range.start, range.is_empty());
    if range.is_empty() {
        return start..start;
    }
    start..byte_offset(text, range.end, true)
}

impl UrlInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            purpose: Purpose::Url,
            focus: cx.focus_handle(),
            text: String::new(),
            anchor: 0,
            caret: 0,
            marked: None,
            layout: None,
            scroll_x: px(0.),
            error: None,
        }
    }

    pub fn query(cx: &mut Context<Self>) -> Self {
        Self {
            purpose: Purpose::Query,
            ..Self::new(cx)
        }
    }

    pub fn capture(cx: &mut Context<Self>) -> Self {
        Self {
            purpose: Purpose::Capture,
            ..Self::field("", cx)
        }
    }

    fn capture_commit(
        &mut self,
        utf16: Option<Range<usize>>,
        value: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.purpose != Purpose::Capture {
            return false;
        }
        let selection = utf16
            .map(|r| byte_range(&self.text, r))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection());
        let Some(candidate) = crate::capture::insert_capture(&self.text, selection, value) else {
            return false;
        };
        cx.emit(UrlEvent::Open(candidate));
        true
    }

    pub fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if !self.capture_commit(None, text, cx) {
            self.replace(None, text.trim_end_matches(['\r', '\n']), cx);
        }
    }

    pub fn field(hint: &'static str, cx: &mut Context<Self>) -> Self {
        Self {
            purpose: Purpose::Field(hint),
            focus: cx.focus_handle().tab_stop(true),
            ..Self::new(cx)
        }
    }

    pub fn value(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, value: String, cx: &mut Context<Self>) {
        self.layout = None;
        self.text = value;
        self.caret = self.text.len();
        self.anchor = self.caret;
        self.error = None;
        self.marked = None;
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, _: &App) {
        window.focus(&self.focus);
    }

    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    fn select(&mut self, position: usize, extend: bool, cx: &mut Context<Self>) {
        self.caret = position;
        if !extend {
            self.anchor = position;
        }
        cx.notify();
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
                .map_or(0, |(index, _)| index)
        }
    }

    fn move_caret(&mut self, forward: bool, extend: bool, cx: &mut Context<Self>) {
        let range = self.selection();
        let target = if extend || range.is_empty() {
            self.adjacent(forward)
        } else if forward {
            range.end
        } else {
            range.start
        };
        self.select(target, extend, cx);
    }

    fn replace(
        &mut self,
        utf16: Option<Range<usize>>,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Range<usize> {
        let range = utf16
            .map(|r| byte_range(&self.text, r))
            .or(self.marked.take())
            .unwrap_or_else(|| self.selection());
        self.text.replace_range(range.clone(), text);
        self.layout = None;
        self.error = None;
        self.select(range.start + text.len(), false, cx);
        range.start..self.caret
    }

    fn erase(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.selection().is_empty() {
            self.anchor = self.adjacent(forward);
        }
        self.replace(None, "", cx);
    }

    fn copy(&self, cx: &mut App) {
        let selection = self.selection();
        if !selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[selection].to_owned()));
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let value = self.text.trim();
        if self.purpose == Purpose::Url {
            match url::Url::parse(value) {
                Ok(url) if matches!(url.scheme(), "http" | "https") && url.has_host() => {
                    cx.emit(UrlEvent::Open(url.into()));
                }
                _ => {
                    self.error = Some("Enter an HTTP or HTTPS file URL.");
                    cx.notify();
                }
            }
        } else if !value.is_empty() || matches!(self.purpose, Purpose::Field(_) | Purpose::Capture)
        {
            cx.emit(UrlEvent::Open(value.to_owned()));
        }
    }

    fn utf16_range(&self, bytes: Range<usize>) -> Range<usize> {
        self.text[..bytes.start].encode_utf16().count()
            ..self.text[..bytes.end].encode_utf16().count()
    }

    fn hit(&self, position: Point<Pixels>) -> usize {
        self.layout.as_ref().map_or(0, |(origin, line)| {
            line.closest_index_for_x(position.x - origin.x)
                .min(self.text.len())
        })
    }
}

impl EntityInputHandler for UrlInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let bytes = byte_range(&self.text, range);
        *adjusted = Some(self.utf16_range(bytes.clone()));
        Some(self.text[bytes].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.utf16_range(self.selection()),
            reversed: self.anchor > self.caret,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone().map(|bytes| self.utf16_range(bytes))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        value: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if value.chars().count() > 1 && self.capture_commit(range.clone(), value, cx) {
            return;
        }
        self.replace(range, &value.replace(['\r', '\n'], " "), cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        value: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let span = self.replace(range, value, cx);
        if let Some(selection) = selected {
            let local = byte_range(value, selection);
            self.anchor = span.start + local.start;
            self.caret = span.start + local.end;
        }
        self.marked = (!span.is_empty()).then_some(span);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (origin, line) = self.layout.as_ref()?;
        let range = byte_range(&self.text, range);
        Some(Bounds::from_corners(
            point(origin.x + line.x_for_index(range.start), bounds.top()),
            point(origin.x + line.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.text[..self.hit(position)].encode_utf16().count())
    }
}

impl Render for UrlInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let focused = self.focus.is_focused(window);
        let (hint, verb) = match self.purpose {
            Purpose::Url => ("https://example.org/data.parquet", "Open"),
            Purpose::Query => ("Ask about the data or write SQL…", "Run"),
            Purpose::Field(hint) => (hint, ""),
            Purpose::Capture => ("Paste a link…", ""),
        };
        let canvas = canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                entity.update(cx, |editor, cx| {
                    window.handle_input(
                        &editor.focus,
                        ElementInputHandler::new(bounds, cx.entity()),
                        cx,
                    );
                    // Replacing each line break with a one-byte space keeps byte positions aligned.
                    let display: SharedString = if editor.text.is_empty() {
                        hint.into()
                    } else {
                        editor.text.replace(['\n', '\r'], " ").into()
                    };
                    let style = window.text_style();
                    let run = TextRun {
                        len: display.len(),
                        font: style.font(),
                        color: rgb(if editor.text.is_empty() {
                            0x777777
                        } else {
                            0x242424
                        })
                        .into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window
                        .text_system()
                        .shape_line(display, px(13.), &[run], None);
                    let caret_x = line.x_for_index(editor.caret);
                    let width = (bounds.size.width - px(2.)).max(px(1.));
                    editor.scroll_x = editor
                        .scroll_x
                        .min(caret_x)
                        .max(caret_x - width)
                        .max(px(0.))
                        .min((line.width - width).max(px(0.)));
                    let origin = point(bounds.left() - editor.scroll_x, bounds.top());
                    let selection = editor.selection();
                    if editor.focus.is_focused(window) && !selection.is_empty() {
                        let start = line.x_for_index(selection.start);
                        let end = line.x_for_index(selection.end);
                        window.paint_quad(fill(
                            Bounds::new(
                                point(origin.x + start, origin.y),
                                size(end - start, bounds.size.height),
                            ),
                            rgb(0xdbdbd7),
                        ));
                    }
                    let _ = line.paint(origin, bounds.size.height, window, cx);
                    if editor.focus.is_focused(window) && selection.is_empty() {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(origin.x + caret_x, origin.y + px(2.)),
                                size(px(1.5), bounds.size.height - px(4.)),
                            ),
                            rgb(0x282828),
                        ));
                    }
                    editor.layout = Some((origin, line));
                });
            },
        )
        .h(px(22.))
        .w_full();

        let field = div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|editor, event: &gpui::MouseDownEvent, window, cx| {
                    editor.focus(window, cx);
                    editor.select(editor.hit(event.position), event.modifiers.shift, cx);
                }),
            )
            .on_mouse_move(cx.listener(|editor, event: &gpui::MouseMoveEvent, _, cx| {
                if event.dragging() {
                    editor.select(editor.hit(event.position), true, cx);
                }
            }))
            .child(canvas);

        let button = div()
            .id("submit")
            .px_3()
            .py(px(3.))
            .rounded_sm()
            .bg(rgb(0x303030))
            .text_color(rgb(0xffffff))
            .text_size(px(12.))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0x111111)))
            .child(verb)
            .on_click(cx.listener(|editor, _, _, cx| editor.submit(cx)));
        let cancel = div()
            .id("cancel")
            .text_size(px(12.))
            .px_2()
            .py(px(3.))
            .rounded_sm()
            .text_color(rgb(0x5d5d5d))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0xe8e8e8)))
            .child("Cancel")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UrlEvent::Cancel)));

        div()
            .id(match self.purpose {
                Purpose::Url => "url-editor",
                Purpose::Query => "query-editor",
                Purpose::Field(hint) => hint,
                Purpose::Capture => "library-link-input",
            })
            .key_context("DatasetInput")
            .track_focus(&self.focus)
            .on_action(cx.listener(|s, _: &Submit, _, cx| s.submit(cx)))
            .on_action(cx.listener(|_, _: &Dismiss, _, cx| cx.emit(UrlEvent::Cancel)))
            .on_action(cx.listener(|s, _: &Copy, _, cx| s.copy(cx)))
            .on_action(cx.listener(|s, _: &Cut, _, cx| {
                s.copy(cx);
                s.replace(None, "", cx);
            }))
            .on_action(cx.listener(|s, _: &Paste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    s.paste_text(&text, cx);
                }
            }))
            .on_action(cx.listener(|s, _: &SelectAll, _, cx| {
                s.anchor = 0;
                s.select(s.text.len(), true, cx);
            }))
            .on_action(cx.listener(|s, _: &Backspace, _, cx| s.erase(false, cx)))
            .on_action(cx.listener(|s, _: &DeleteForward, _, cx| s.erase(true, cx)))
            .on_action(cx.listener(|s, _: &Left, _, cx| s.move_caret(false, false, cx)))
            .on_action(cx.listener(|s, _: &Right, _, cx| s.move_caret(true, false, cx)))
            .on_action(cx.listener(|s, _: &ExtendLeft, _, cx| s.move_caret(false, true, cx)))
            .on_action(cx.listener(|s, _: &ExtendRight, _, cx| s.move_caret(true, true, cx)))
            .on_action(cx.listener(|s, _: &Home, _, cx| s.select(0, false, cx)))
            .on_action(cx.listener(|s, _: &End, _, cx| s.select(s.text.len(), false, cx)))
            .on_action(cx.listener(|s, _: &ExtendHome, _, cx| s.select(0, true, cx)))
            .on_action(cx.listener(|s, _: &ExtendEnd, _, cx| s.select(s.text.len(), true, cx)))
            .w_full()
            .flex()
            .flex_col()
            .when(
                !matches!(self.purpose, Purpose::Field(_) | Purpose::Capture),
                |s| {
                    s.px_4()
                        .py(px(4.))
                        .border_b_1()
                        .border_color(rgb(0xe2e2e2))
                        .bg(rgb(0xf8f8f8))
                },
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_family("IBM Plex Mono")
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .h(px(28.))
                            .px_2()
                            .overflow_hidden()
                            .rounded_sm()
                            .bg(rgb(0xffffff))
                            .border_1()
                            .border_color(rgb(if focused { 0x8e8e8e } else { 0xcfcfcf }))
                            .child(field),
                    )
                    .when(
                        !matches!(self.purpose, Purpose::Field(_) | Purpose::Capture),
                        |s| s.child(cancel).child(button),
                    ),
            )
            .when_some(self.error, |this, message| {
                this.child(
                    div()
                        .pt_1()
                        .text_size(px(12.))
                        .text_color(rgb(0x484848))
                        .child(message),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_text_ranges_select_whole_unicode_characters() {
        let text = "é😀A";
        assert_eq!(&text[byte_range(text, 1..3)], "😀");
        assert_eq!(byte_range(text, 3..99), 6..7);
        assert_eq!(&text[byte_range(text, 2..3)], "😀");
        assert_eq!(&text[byte_range(text, 1..2)], "😀");
        assert_eq!(byte_range(text, 2..2), 6..6);
    }
}
