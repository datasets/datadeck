use super::*;

impl Deck {
    pub(super) fn active_grid(&self) -> Entity<Grid> {
        if self.showing_result {
            self.result.clone()
        } else {
            self.grid.clone()
        }
    }

    pub(super) fn update_overview(&mut self, cx: &mut Context<Self>) {
        if !self.overview_open {
            return;
        }
        let (columns, batches, count) = self.active_grid().read(cx).overview_data();
        let total = if self.showing_result {
            self.result_count
        } else {
            count
        };
        self.overview.update(cx, |view, cx| {
            view.show(
                columns,
                batches,
                total,
                self.font_size.into(),
                self.theme,
                cx,
            )
        });
    }

    pub(super) fn view_switcher(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let active = self.active_grid();
        let open = active.read(cx).inspector_open();
        let tab = |id: &'static str, label: &'static str, selected: bool| {
            div()
                .id(id)
                .h_full()
                .flex()
                .items_center()
                .px_2()
                .cursor_pointer()
                .border_b_2()
                .border_color(rgb(if selected { INK } else { PAPER }))
                .text_color(rgb(if selected { INK } else { MUTED }))
                .child(label)
        };
        div()
            .h(px(29.))
            .flex_none()
            .px_3()
            .flex()
            .gap_1()
            .items_center()
            .text_size(px(12.))
            .border_b_1()
            .border_color(rgb(LINE))
            .child(
                tab("view-rows", "Rows", !self.overview_open).on_click(cx.listener(
                    |s, _, w, cx| {
                        s.overview_open = false;
                        s.active_grid().focus_handle(cx).focus(w);
                        cx.notify()
                    },
                )),
            )
            .child(
                tab("view-columns", "All columns", self.overview_open).on_click(cx.listener(
                    |s, _, w, cx| {
                        s.overview_open = true;
                        s.update_overview(cx);
                        s.overview.focus_handle(cx).focus(w);
                        cx.notify()
                    },
                )),
            )
            .child(div().flex_1())
            .when(!self.overview_open, |bar| {
                bar.child(
                    div()
                        .id("toggle-inspector")
                        .cursor_pointer()
                        .px_2()
                        .text_color(rgb(MUTED))
                        .hover(|s| s.text_color(rgb(INK)))
                        .child(if open {
                            "Hide inspector"
                        } else {
                            "Show inspector"
                        })
                        .on_click(cx.listener(move |_, _, _, cx| {
                            active.update(cx, |g, cx| g.set_inspector(!open, cx));
                            cx.notify()
                        })),
                )
            })
    }
}
