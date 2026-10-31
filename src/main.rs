mod appearance;
// The generated schema has every table and column, used yet or not, and the
// device ID waits for sync.
#[allow(dead_code)]
mod database;
mod dev_overlay;
mod import;
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

use database::Database;
use dev_overlay::DevOverlay;
use model::settings::{DeviceSettings, SavedWindow, WindowState};
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

/// Where the window was when it was last left, if it's still on a screen:
/// the screen it was on may have been unplugged since.
fn saved_window(database: &Entity<Database>, cx: &App) -> Option<SavedWindow> {
    let saved = match DeviceSettings::read(database.read(cx).device_connection()) {
        Ok(settings) => settings.window?,
        Err(error) => {
            eprintln!("couldn't read where the window was: {error}");
            return None;
        }
    };
    let bounds = window::restored(saved);
    cx.displays()
        .iter()
        .any(|display| display.bounds().intersects(&bounds))
        .then_some(saved)
}

fn main() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    return_freed_memory();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            appearance::init(cx);
            dev_overlay::init(cx);
            workspace::init(cx);
            let database = match database::open(cx) {
                Ok(db) => db,
                Err(err) => {
                    eprintln!("couldn't open the database: {err}");
                    cx.quit();
                    return;
                }
            };
            let setup = database.read(cx).write(cx, |write| {
                typeset::install_templates(write)?;
                // KRON_NO_SAMPLE starts empty, to see what a new user does.
                #[cfg(debug_assertions)]
                if std::env::var_os("KRON_NO_SAMPLE").is_none() {
                    model::sample::write_if_empty(write)?;
                }
                Ok(())
            });
            cx.background_spawn(async move {
                if let Err(error) = setup.await {
                    eprintln!("couldn't set up the database: {error}");
                }
            })
            .detach();
            let saved = saved_window(&database, cx);
            let (bounds, maximize) = match saved {
                // Opened windowed and set to its state once mapped, like the
                // default size's maximize.
                Some(saved) => (
                    window::restored(saved),
                    saved.state == WindowState::Maximized,
                ),
                None => window_bounds(cx),
            };
            let fullscreen = saved.is_some_and(|saved| saved.state == WindowState::Fullscreen);
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
                        // Saved in logical pixels; X11 places windows in the screen's.
                        let scale = window.scale_factor();
                        window::place_window(saved.map(|saved| {
                            (
                                (saved.x as f32 * scale).round() as i32,
                                (saved.y as f32 * scale).round() as i32,
                            )
                        }));
                        // Maximize a frame later so the window manager records the
                        // position as the one to restore to.
                        if maximize {
                            window.on_next_frame(|window, _| window.zoom_window());
                        } else if fullscreen {
                            window.on_next_frame(|window, _| window.toggle_fullscreen());
                        }
                    });
                    let dev_overlay = cx.new(|cx| DevOverlay::new(database.clone(), cx));
                    cx.new(|cx| MainWindow {
                        workspace: cx.new(|cx| Workspace::new(database, window, cx)),
                        dev_overlay,
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

/// glibc raises its mmap threshold to the size of each large block freed
/// (mallopt(3)), so once a 2 MB page image is freed, the next ones come from
/// the heap, which keeps what's freed. Setting the threshold turns that off:
/// at glibc's initial 128 KiB, each page image gets a mapping of its own,
/// returned when it's freed.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn return_freed_memory() {
    use std::ffi::c_int;
    // From glibc's malloc.h.
    const M_MMAP_THRESHOLD: c_int = -3;
    const MMAP_THRESHOLD: c_int = 128 * 1024;
    unsafe extern "C" {
        fn mallopt(parameter: c_int, value: c_int) -> c_int;
    }
    // SAFETY: mallopt only sets an allocator parameter, and runs before any
    // other thread starts.
    if unsafe { mallopt(M_MMAP_THRESHOLD, MMAP_THRESHOLD) } == 0 {
        eprintln!("couldn't set glibc's mmap threshold");
    }
}
