use gpui::{
    Bounds, CursorStyle, Decorations, HitboxBehavior, Hsla, IntoElement, MouseButton, Pixels,
    Point, ResizeEdge, Size, Tiling, Window, canvas, div, point, prelude::*, px, rgb,
    transparent_black,
};

/// The transparent area around the window holds the shadow and doubles as the
/// resize handle.
const SHADOW_SIZE: Pixels = px(10.);
const ROUNDING: Pixels = px(8.);

pub fn window_frame(window: &mut Window, content: impl IntoElement) -> impl IntoElement {
    let Decorations::Client { tiling } = window.window_decorations() else {
        // No compositor: the window manager draws the frame, so there is no
        // shadow area to reserve.
        window.set_client_inset(px(0.));
        return div().id("window-backdrop").size_full().child(content);
    };
    window.set_client_inset(SHADOW_SIZE);

    div()
        .id("window-backdrop")
        .size_full()
        .bg(transparent_black())
        .when(!tiling.top, |div| div.pt(SHADOW_SIZE))
        .when(!tiling.bottom, |div| div.pb(SHADOW_SIZE))
        .when(!tiling.left, |div| div.pl(SHADOW_SIZE))
        .when(!tiling.right, |div| div.pr(SHADOW_SIZE))
        // Edges against the screen (tiled) have no shadow area and can't be
        // resized from; the others keep their handles.
        .when(!fully_tiled(tiling), |div| {
            div.child(
                canvas(
                    |_bounds, window, _cx| {
                        let size = window.window_bounds().get_bounds().size;
                        window.insert_hitbox(
                            Bounds::new(point(px(0.), px(0.)), size),
                            HitboxBehavior::Normal,
                        )
                    },
                    move |_bounds, hitbox, window, _cx| {
                        let size = window.window_bounds().get_bounds().size;
                        if let Some(edge) = resize_edge(window.mouse_position(), size, tiling) {
                            window.set_cursor_style(resize_cursor(edge), &hitbox);
                        }
                    },
                )
                .size_full()
                .absolute(),
            )
            .on_mouse_move(|_, window, _| window.refresh())
            .on_mouse_down(MouseButton::Left, move |event, window, _| {
                let size = window.window_bounds().get_bounds().size;
                if let Some(edge) = resize_edge(event.position, size, tiling) {
                    window.start_window_resize(edge);
                }
            })
        })
        .child(
            div()
                .size_full()
                .cursor(CursorStyle::Arrow)
                .overflow_hidden()
                .border_color(rgb(0x3c3c3c))
                .when(!tiling.top, |div| div.border_t_1())
                .when(!tiling.bottom, |div| div.border_b_1())
                .when(!tiling.left, |div| div.border_l_1())
                .when(!tiling.right, |div| div.border_r_1())
                .when(!(tiling.top || tiling.left), |div| div.rounded_tl(ROUNDING))
                .when(!(tiling.top || tiling.right), |div| {
                    div.rounded_tr(ROUNDING)
                })
                .when(!(tiling.bottom || tiling.left), |div| {
                    div.rounded_bl(ROUNDING)
                })
                .when(!(tiling.bottom || tiling.right), |div| {
                    div.rounded_br(ROUNDING)
                })
                .when(!tiling.is_tiled(), |div| {
                    div.shadow(vec![gpui::BoxShadow {
                        color: Hsla::black().opacity(0.4),
                        blur_radius: SHADOW_SIZE / 2.,
                        spread_radius: px(0.),
                        offset: point(px(0.), px(0.)),
                    }])
                })
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .child(content),
        )
}

fn fully_tiled(tiling: Tiling) -> bool {
    tiling.top && tiling.bottom && tiling.left && tiling.right
}

fn resize_edge(position: Point<Pixels>, size: Size<Pixels>, tiling: Tiling) -> Option<ResizeEdge> {
    let top = !tiling.top && position.y < SHADOW_SIZE;
    let bottom = !tiling.bottom && position.y > size.height - SHADOW_SIZE;
    let left = !tiling.left && position.x < SHADOW_SIZE;
    let right = !tiling.right && position.x > size.width - SHADOW_SIZE;

    Some(match (top, bottom, left, right) {
        (true, _, true, _) => ResizeEdge::TopLeft,
        (true, _, _, true) => ResizeEdge::TopRight,
        (_, true, true, _) => ResizeEdge::BottomLeft,
        (_, true, _, true) => ResizeEdge::BottomRight,
        (true, ..) => ResizeEdge::Top,
        (_, true, ..) => ResizeEdge::Bottom,
        (_, _, true, _) => ResizeEdge::Left,
        (_, _, _, true) => ResizeEdge::Right,
        _ => return None,
    })
}

fn resize_cursor(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}
