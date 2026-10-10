use gpui::{
    Context, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, SharedString,
    Window, div, prelude::*, px, rgb, svg,
};

// Small jitter during a (double) click shouldn't start a window move.
const DRAG_THRESHOLD: Pixels = px(4.);

pub struct TitleBar {
    title: SharedString,
    /// Where the left button was pressed. The window move starts only past
    /// `DRAG_THRESHOLD`, so clicks and double-clicks still reach the title bar.
    drag_start: Option<Point<Pixels>>,
}

impl TitleBar {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            drag_start: None,
        }
    }
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let maximize = if window.is_maximized() {
            WindowControl::Restore
        } else {
            WindowControl::Maximize
        };

        div()
            .id("title-bar")
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(title_bar_height(window))
            .w_full()
            .border_b_1()
            .border_color(rgb(0x2b2b2b))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    this.drag_start = Some(event.position)
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag_start = None),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.drag_start = None))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, _| {
                let Some(start) = this.drag_start else {
                    return;
                };
                let delta = event.position - start;
                if delta.x.abs().max(delta.y.abs()) >= DRAG_THRESHOLD {
                    this.drag_start = None;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.zoom_window();
                }
            })
            .on_mouse_down(MouseButton::Right, |event, window, _| {
                window.show_window_menu(event.position)
            })
            .child(
                div()
                    .pl_3()
                    .text_sm()
                    .text_color(rgb(0xcccccc))
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .id("window-controls")
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    // Keep clicks on buttons from starting a window move.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(WindowControl::Minimize.render())
                    .child(maximize.render())
                    .child(WindowControl::Close.render()),
            )
    }
}

/// Same as Zed's `platform_title_bar_height`.
fn title_bar_height(window: &Window) -> Pixels {
    (window.rem_size() * 1.75).max(px(34.))
}

#[derive(Clone, Copy)]
enum WindowControl {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl WindowControl {
    fn id(self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::Maximize => "maximize",
            Self::Restore => "restore",
            Self::Close => "close",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Minimize => "icons/minimize.svg",
            Self::Maximize => "icons/maximize.svg",
            Self::Restore => "icons/restore.svg",
            Self::Close => "icons/close.svg",
        }
    }

    fn render(self) -> impl IntoElement {
        let group = self.id();

        div()
            .id(self.id())
            .group(group)
            .flex()
            .items_center()
            .justify_center()
            .size_5()
            .rounded_full()
            .cursor_pointer()
            .hover(|style| style.bg(rgb(0x363c46)))
            .active(|style| style.bg(rgb(0x363c46)))
            .child(
                svg()
                    .size_4()
                    .flex_none()
                    .path(self.icon())
                    .text_color(rgb(0xdce0e5))
                    .group_hover(group, |style| style.text_color(rgb(0xa9afbc))),
            )
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                match self {
                    Self::Minimize => window.minimize_window(),
                    Self::Maximize | Self::Restore => window.zoom_window(),
                    Self::Close => window.remove_window(),
                }
            })
    }
}
