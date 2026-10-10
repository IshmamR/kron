mod assets;
mod center;
mod title_bar;
mod window_frame;

use gpui::{
    App, Application, Bounds, Context, Decorations, Entity, Pixels, Size, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions, div, prelude::*,
    px, rgb, size,
};

use title_bar::TitleBar;
use window_frame::window_frame;

const DEFAULT_WINDOW_SIZE: Size<Pixels> = size(px(1536.), px(1095.));
/// Smallest size the window can be resized to (same as Zed).
const MIN_WINDOW_SIZE: Size<Pixels> = size(px(360.), px(240.));

const TITLE: &str = "carear";

struct Root {
    title_bar: Entity<TitleBar>,
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Without a compositor gpui falls back to the window manager's frame,
        // which already has a title bar.
        let client_decorations = matches!(window.window_decorations(), Decorations::Client { .. });

        window_frame(
            window,
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(rgb(0x1e1e1e))
                .when(client_decorations, |div| div.child(self.title_bar.clone()))
                .child(div().flex_1()),
        )
    }
}

/// Returns centered window bounds, and whether the screen is too small for
/// `DEFAULT_WINDOW_SIZE` (in which case the window should be maximized).
///
/// The window fits when it takes at most 90% of the screen, leaving room for
/// panels. When it doesn't, it's shrunk to 90% so the size it restores to
/// after un-maximizing still fits.
fn window_bounds(cx: &App) -> (Bounds<Pixels>, bool) {
    let usable = cx.primary_display().map(|display| {
        let screen = display.bounds().size;
        size(screen.width * 0.9, screen.height * 0.9)
    });
    let fits = usable.is_none_or(|usable| {
        DEFAULT_WINDOW_SIZE.width <= usable.width && DEFAULT_WINDOW_SIZE.height <= usable.height
    });
    let window_size = match usable {
        Some(usable) if !fits => size(
            DEFAULT_WINDOW_SIZE.width.min(usable.width),
            DEFAULT_WINDOW_SIZE.height.min(usable.height),
        ),
        _ => DEFAULT_WINDOW_SIZE,
    };

    (Bounds::centered(None, window_size, cx), !fits)
}

fn main() {
    Application::new()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            let (bounds, maximize) = window_bounds(cx);
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(TITLE.into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    window_min_size: Some(MIN_WINDOW_SIZE),
                    window_decorations: Some(WindowDecorations::Client),
                    // Lets the shadow area around the window show what's behind it.
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| {
                    // Position and maximize requests are applied once the window is
                    // mapped; X11 window managers ignore them before that.
                    window.on_next_frame(move |window, _| {
                        center::center_window();
                        // Maximize a frame later so the window manager records the
                        // centered position as the one to restore to.
                        if maximize {
                            window.on_next_frame(|window, _| window.zoom_window());
                        }
                    });
                    cx.new(|cx| Root {
                        title_bar: cx.new(|_| TitleBar::new(TITLE)),
                    })
                },
            );
            if let Err(err) = opened {
                eprintln!("failed to open window: {err:#}");
                cx.quit();
                return;
            }
            cx.activate(true);
        });
}
