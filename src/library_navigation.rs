use super::*;

impl Deck {
    pub(super) fn toggle_library(
        &mut self,
        _: &ShowLibrary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.link_probe = None;
        if self.library_open {
            self.leave_library(cx);
        } else {
            self.library_open = true;
        }
        if self.library_open {
            self.url_open = false;
            self.library.update(cx, |v, cx| v.refresh(cx));
            self.library.read(cx).focus(window);
        } else if self.query_open {
            self.editor.read(cx).focus(window, cx);
        } else if self.overview_open {
            self.overview.focus_handle(cx).focus(window);
        } else if self.source.is_empty() {
            self.focus.focus(window);
        } else {
            self.active_grid().focus_handle(cx).focus(window);
        }
        cx.notify();
    }

    pub(super) fn leave_library(&mut self, cx: &mut Context<Self>) {
        if self.library_open {
            self.library.update(cx, |view, cx| view.flush_notes(cx));
            self.library_open = false;
        }
    }

    pub(super) fn bookmark(&mut self, url: String, cx: &mut Context<Self>) {
        self.capture_bookmark(
            web_links::Capture {
                url,
                note: String::new(),
            },
            cx,
        );
    }

    pub(super) fn capture_bookmark(&mut self, item: web_links::Capture, cx: &mut Context<Self>) {
        self.link_probe = None;
        self.library_open = true;
        self.focus_library = true;
        self.url_open = false;
        self.error = None;
        self.library
            .update(cx, |v, cx| v.capture(item.url, item.note, cx));
        cx.notify();
    }

    pub(super) fn open_location(&mut self, source: String, cx: &mut Context<Self>) {
        self.link_probe = None;
        if let Some(url) = web_links::web_url(&source) {
            if !web_links::looks_like_table(&url) {
                let probe = cx
                    .background_executor()
                    .spawn(async move { web_links::is_page(&url) });
                self.link_probe = Some(cx.spawn(async move |this, cx| {
                    let is_page = probe.await;
                    let _ = this.update(cx, |s, cx| {
                        if is_page {
                            s.bookmark(source, cx)
                        } else {
                            s.load(source, None, cx)
                        }
                    });
                }));
                cx.notify();
                return;
            }
        } else if Path::new(&source)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("webloc"))
        {
            match web_links::read_webloc(Path::new(&source)) {
                Ok(link) => self.bookmark(link, cx),
                Err(e) => {
                    self.error = Some(e);
                    cx.notify();
                }
            }
            return;
        }
        self.load(source, None, cx);
    }
}
