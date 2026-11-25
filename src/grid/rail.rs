use gpui::{prelude::*, *};
use std::{cell::Cell, rc::Rc};

pub(super) type Drag = Rc<Cell<Option<(Axis, Pixels)>>>;

/// Size the thumb from the painted viewport so wheel and keyboard movement use the same position.
pub(super) fn scrollbar(
    axis: Axis,
    scroll: ScrollHandle,
    drag: Drag,
    view: EntityId,
) -> impl IntoElement + Styled {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |track, hit, window, _| {
            let range = scroll.max_offset().along(axis);
            let track_len = track.size.along(axis);
            if range <= px(0.) || track_len <= px(24.) {
                return;
            }
            let visible = scroll.bounds().size.along(axis);
            let size =
                (track_len * (visible / (visible + range))).clamp(px(20.), track_len - px(1.));
            let travel = track_len - size;
            let thumb_start = travel * (-scroll.offset().along(axis) / range).clamp(0., 1.);
            let thumb = Bounds::new(
                track
                    .origin
                    .apply_along(axis, |v| v + thumb_start)
                    .apply_along(axis.invert(), |v| v + px(2.)),
                track
                    .size
                    .apply_along(axis, |_| size)
                    .apply_along(axis.invert(), |v| v - px(4.)),
            );
            window.paint_quad(quad(
                thumb,
                px(2.),
                rgb(0xc6c6c6),
                px(0.),
                rgb(0xc6c6c6),
                BorderStyle::default(),
            ));
            let scroll_to = move |mouse: Point<Pixels>, grab: Pixels, cx: &mut App| {
                let fraction =
                    ((mouse.along(axis) - track.origin.along(axis) - grab) / travel).clamp(0., 1.);
                scroll.set_offset(scroll.offset().apply_along(axis, |_| -range * fraction));
                cx.notify(view);
            };
            let on_down = scroll_to.clone();
            let down_drag = drag.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase.bubble() && event.button == MouseButton::Left && hit.is_hovered(window) {
                    let grab = if thumb.contains(&event.position) {
                        event.position.along(axis) - thumb.origin.along(axis)
                    } else {
                        size * 0.5
                    };
                    down_drag.set(Some((axis, grab)));
                    on_down(event.position, grab, cx);
                    cx.stop_propagation();
                }
            });
            let move_drag = drag.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if phase.bubble()
                    && event.dragging()
                    && let Some((active, grab)) = move_drag.get()
                    && active == axis
                {
                    scroll_to(event.position, grab, cx);
                    cx.stop_propagation();
                }
            });
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, _| {
                if phase.capture() && event.button == MouseButton::Left {
                    drag.set(None);
                }
            });
        },
    )
    .when(axis == Axis::Horizontal, |el| el.h(px(9.)).w_full())
    .when(axis == Axis::Vertical, |el| el.w(px(9.)))
    .flex_shrink_0()
}
