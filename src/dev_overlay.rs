// Dev-only tooling: keep it self-contained. The rest of the app only calls
// `init` and renders a `DevOverlay`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui_kit::{
    App, Context, Entity, Global, Hsla, IntoElement, KeyBinding, SharedString, Subscription, Task,
    Window, actions, div, prelude::*, px, rgb,
};
use sysinfo::{CpuRefreshKind, Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

use crate::database::Database;
#[cfg(debug_assertions)]
use crate::model::sample;
#[cfg(debug_assertions)]
use crate::typeset;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
const FRAME_WINDOW: Duration = Duration::from_secs(1);

actions!(dev_overlay, [ToggleDevOverlay]);

pub fn init(cx: &mut App) {
    cx.set_global(Visible::default());
    cx.bind_keys([KeyBinding::new("ctrl-d", ToggleDevOverlay, None)]);
    cx.on_action(|_: &ToggleDevOverlay, cx| {
        cx.update_global::<Visible, _>(|visible, _| visible.0 = !visible.0);
    });
}

pub struct DevOverlay {
    /// For resetting the data, in debug builds.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    database: Entity<Database>,
    /// While the development data is being reset.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    reset: Option<Task<()>>,
    /// Timestamps of recent frames, newest last.
    frames: VecDeque<Instant>,
    /// `None` when sysinfo can't read this system; the overlay then shows no
    /// hardware or process stats.
    system: Option<System>,
    pid: Option<Pid>,
    stats: ProcessStats,
    hardware: Hardware,
    sampler: Option<Task<()>>,
    _visibility: Subscription,
}

impl DevOverlay {
    pub fn new(database: Entity<Database>, cx: &mut Context<Self>) -> Self {
        let system =
            System::new_with_specifics(RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing()))
                .inspect_err(|error| eprintln!("couldn't read system info: {error}"))
                .ok();
        let cpus = system.as_ref().map(System::cpus).unwrap_or_default();
        let cpu = cpus
            .first()
            .map(|cpu| cpu.brand().trim().to_string())
            .unwrap_or_else(|| "unknown".into());
        let threads = cpus.len();
        let cores = match System::physical_core_count() {
            Ok(physical) => format!("{physical} cores / {threads} threads"),
            Err(_) => format!("{threads} threads"),
        };
        let os = System::long_os_version().unwrap_or_else(|_| std::env::consts::OS.into());

        let mut this = Self {
            database,
            reset: None,
            frames: VecDeque::new(),
            system,
            pid: sysinfo::get_current_pid().ok(),
            stats: ProcessStats::default(),
            hardware: Hardware {
                cpu: cpu.into(),
                cores: cores.into(),
                os: os.into(),
            },
            sampler: None,
            _visibility: cx.observe_global::<Visible>(|this, cx| {
                this.on_visibility_changed(cx);
                cx.notify();
            }),
        };
        this.on_visibility_changed(cx);
        this
    }

    fn on_visibility_changed(&mut self, cx: &mut Context<Self>) {
        if !is_visible(cx) {
            self.sampler = None;
            self.frames.clear();
            return;
        }
        if self.sampler.is_some() {
            return;
        }
        self.sample();
        self.sampler = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SAMPLE_INTERVAL).await;
                let updated = this.update(cx, |this, cx| {
                    this.sample();
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        }));
    }

    fn sample(&mut self) {
        let (Some(system), Some(pid)) = (self.system.as_mut(), self.pid) else {
            return;
        };
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        if let Some(process) = system.process(pid) {
            self.stats.cpu = process.cpu_usage();
            self.stats.memory = process.memory();
        }
    }

    /// Replaces everything in the database with the sample data.
    #[cfg(debug_assertions)]
    fn reset_data(&mut self, cx: &mut Context<Self>) {
        if self.reset.is_some() {
            return;
        }
        let reset = self.database.read(cx).write(cx, |write| {
            write.erase_everything()?;
            typeset::install_templates(write)?;
            sample::write_sample(write)
        });
        self.reset = Some(cx.spawn(async move |this, cx| {
            if let Err(error) = reset.await {
                eprintln!("couldn't reset the development data: {error}");
            }
            // The task lives in the overlay, so the overlay is still there.
            if let Err(error) = this.update(cx, |this, cx| {
                this.reset = None;
                cx.notify();
            }) {
                eprintln!("dev overlay: {error}");
            }
        }));
        cx.notify();
    }

    /// Records this frame and returns (fps, average ms, worst ms) over the
    /// last `FRAME_WINDOW`.
    fn frame_stats(&mut self) -> (usize, f64, f64) {
        let now = Instant::now();
        self.frames.push_back(now);
        while self
            .frames
            .front()
            .is_some_and(|&frame| now.duration_since(frame) > FRAME_WINDOW)
        {
            self.frames.pop_front();
        }

        let deltas = self
            .frames
            .iter()
            .zip(self.frames.iter().skip(1))
            .map(|(previous, next)| next.duration_since(*previous).as_secs_f64() * 1000.);
        let (count, sum, worst) = deltas.fold((0, 0., 0f64), |(count, sum, worst), delta| {
            (count + 1, sum + delta, worst.max(delta))
        });
        let average = if count > 0 { sum / count as f64 } else { 0. };
        (self.frames.len(), average, worst)
    }
}

impl Render for DevOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !is_visible(cx) {
            return div();
        }
        // Keep redrawing so frame timings stay live.
        window.request_animation_frame();

        let (fps, average_ms, worst_ms) = self.frame_stats();
        let gpu = window.gpu_specs();
        let bounds = window.bounds();

        let overlay = div()
            .absolute()
            .top_2()
            .right_2()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .min_w(px(280.))
            .rounded_md()
            // See-through enough to show what's under it; at 60% the light text
            // still reads over a white page.
            .bg(Hsla::black().opacity(0.6))
            .border_1()
            .border_color(rgb(0x3c3c3c))
            .text_xs()
            .text_color(rgb(0xdce0e5))
            .child(section(
                "Frame",
                [
                    ("FPS", fps.to_string()),
                    ("Frame time", format!("{average_ms:.2} ms avg")),
                    ("Worst frame", format!("{worst_ms:.2} ms")),
                    (
                        "Window",
                        format!(
                            "{:.0}×{:.0} @ {:.2}x",
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                            window.scale_factor()
                        ),
                    ),
                ],
            ))
            .child(section(
                "Hardware",
                [
                    ("CPU", self.hardware.cpu.to_string()),
                    ("Cores", self.hardware.cores.to_string()),
                    (
                        "GPU",
                        gpu.as_ref()
                            .map(|gpu| gpu.device_name.clone())
                            .unwrap_or_else(|| "unknown".into()),
                    ),
                    (
                        "Driver",
                        gpu.as_ref()
                            .map(|gpu| format!("{} {}", gpu.driver_name, gpu.driver_info))
                            .unwrap_or_else(|| "unknown".into()),
                    ),
                    ("OS", self.hardware.os.to_string()),
                ],
            ))
            .child(section(
                "Usage",
                [
                    ("App CPU", format!("{:.1}% of a core", self.stats.cpu)),
                    ("App RAM", format_bytes(self.stats.memory)),
                ],
            ));
        // Release builds carry no sample data to reset to.
        #[cfg(debug_assertions)]
        let overlay = overlay.child(
            div()
                .id("reset-data")
                .px_2()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(rgb(0x3c3c3c))
                .text_center()
                .cursor_pointer()
                .hover(|style| style.bg(Hsla::white().opacity(0.1)))
                .child(if self.reset.is_some() {
                    "Resetting…"
                } else {
                    "Reset data to the sample"
                })
                .on_click(cx.listener(|this, _, _, cx| this.reset_data(cx))),
        );
        overlay
    }
}

/// Shared by all windows.
#[derive(Default)]
struct Visible(bool);

impl Global for Visible {}

fn is_visible(cx: &App) -> bool {
    cx.try_global::<Visible>().is_some_and(|visible| visible.0)
}

#[derive(Default)]
struct ProcessStats {
    cpu: f32,
    memory: u64,
}

struct Hardware {
    cpu: SharedString,
    cores: SharedString,
    os: SharedString,
}

fn section<const N: usize>(
    title: &'static str,
    rows: [(&'static str, String); N],
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(div().text_color(rgb(0x8b929e)).child(title))
        .children(rows.into_iter().map(|(label, value)| {
            div()
                .flex()
                .justify_between()
                .gap_4()
                .child(div().text_color(rgb(0xa9afbc)).child(label))
                .child(value)
        }))
}

fn format_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024. * 1024.;
    let mebibytes = bytes as f64 / MIB;
    if mebibytes >= 1024. {
        format!("{:.2} GiB", mebibytes / 1024.)
    } else {
        format!("{mebibytes:.0} MiB")
    }
}
