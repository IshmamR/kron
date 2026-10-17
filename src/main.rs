mod appearance;
mod dev_overlay;
#[allow(dead_code)] // Parts of it aren't used by the UI yet.
mod model;
mod typeset;
mod window;
mod workspace;

use gpui_kit::component::{TitleBar, v_flex};
use gpui_kit::{
    App, Bounds, Context, Decorations, Entity, Pixels, Size, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions, div, prelude::*,
    px, size,
};

use dev_overlay::DevOverlay;
use workspace::Workspace;

const DEFAULT_WINDOW_SIZE: Size<Pixels> = size(px(1536.), px(1095.));
/// Same as Zed.
const MIN_WINDOW_SIZE: Size<Pixels> = size(px(360.), px(240.));

const TITLE: &str = "Kron";

/// The window's content. gpui-component's `Root` wraps it and draws the window
/// border and shadow.
struct MainWindow {
    workspace: Entity<Workspace>,
    dev_overlay: Entity<DevOverlay>,
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // On Linux without a compositor gpui falls back to the window
        // manager's frame, which already has a title bar. Windows and macOS
        // always report server decorations, yet hide their own title bar for
        // ours, so they always need it.
        let draws_title_bar = !cfg!(target_os = "linux")
            || matches!(window.window_decorations(), Decorations::Client { .. });

        v_flex()
            .size_full()
            .when(draws_title_bar, |this| {
                // Where the logo will go.
                this.child(TitleBar::new().child(div().text_sm().child(TITLE)))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(self.workspace.clone())
                    .child(self.dev_overlay.clone()),
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
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            appearance::init(cx);
            dev_overlay::init(cx);
            workspace::init(cx);
            let (bounds, maximize) = window_bounds(cx);
            let opened = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(TITLE.into()),
                        ..TitleBar::title_bar_options()
                    }),
                    window_min_size: Some(MIN_WINDOW_SIZE),
                    window_decorations: Some(WindowDecorations::Client),
                    // Lets the shadow area around the window show what's behind it.
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..TitleBar::window_options()
                },
                cx,
                |window, cx| {
                    // Position and maximize requests are applied once the window is
                    // mapped; X11 window managers ignore them before that.
                    window.on_next_frame(move |window, _| {
                        window::center_window();
                        // Maximize a frame later so the window manager records the
                        // centered position as the one to restore to.
                        if maximize {
                            window.on_next_frame(|window, _| window.zoom_window());
                        }
                    });
                    let library = model::sample::library().unwrap_or_else(|error| {
                        eprintln!("couldn't load the sample library: {error}");
                        model::Library::default()
                    });
                    let library = cx.new(|_| library);
                    cx.new(|cx| MainWindow {
                        workspace: cx.new(|cx| Workspace::new(library, window, cx)),
                        dev_overlay: cx.new(DevOverlay::new),
                    })
                },
            );
            if let Err(error) = opened {
                eprintln!("failed to open window: {error:#}");
                cx.quit();
                return;
            }
            cx.activate(true);
        });
}
