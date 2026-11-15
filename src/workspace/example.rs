// An example job, for a new user to see what Kron makes before giving it
// anything: the sample's résumé for one job, its lines citing their facts and
// the claims kept off it. It has databases of its own, so nothing done here
// reaches the user's data; its job view holds them, and drops them with it.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, IntoElement, Subscription,
    Task, Window, div, prelude::*,
};

use super::ShowFact;
use super::job_view::{JobView, JobViewEvent};
use super::parts::caption;
use super::previews::Previews;
use crate::database::Database;
use crate::model::sample;

pub struct Example {
    job_view: Entity<JobView>,
    _writing: Task<()>,
    _subscriptions: Vec<Subscription>,
}

pub enum ExampleEvent {
    Close,
}

impl EventEmitter<ExampleEvent> for Example {}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            // The facts page shows the user's own facts, not the example's.
            .on_action(|_: &ShowFact, _, _| {})
            .child(
                h_flex()
                    .px_6()
                    .py_3()
                    .gap_4()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.primary.opacity(0.06))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("An example: Alex's résumé for a job at Cinder Labs"),
                            )
                            .child(caption(
                                "Click any line to see the facts it came from. Left out lists \
                                 what Alex's facts don't back, so it isn't on the page.",
                                cx,
                            )),
                    )
                    .child(
                        Button::new("close-example")
                            .label("Start with your CV")
                            .primary()
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ExampleEvent::Close))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.job_view.clone()))
    }
}

impl Focusable for Example {
    fn focus_handle(&self, cx: &gpui_kit::App) -> FocusHandle {
        self.job_view.focus_handle(cx)
    }
}

impl Example {
    /// Writes the sample to `example`, opened empty with
    /// `database::open_example`, and shows its job once written.
    pub fn new(example: Entity<Database>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let previews = cx.new(|cx| Previews::new(example.clone(), cx));
        let job_view = cx.new(|cx| JobView::new(example.clone(), previews, window, cx));
        let written = example.read(cx).write(cx, sample::write_example);
        let writing = cx.spawn(async move |this, cx| {
            let job = match written.await {
                Ok(job) => job,
                Err(error) => {
                    eprintln!("example: couldn't write the sample: {error}");
                    return;
                }
            };
            let shown = this.update(cx, |this, cx| {
                this.job_view
                    .update(cx, |job_view, cx| job_view.set_job(job, cx));
            });
            if let Err(error) = shown {
                eprintln!("example: {error}");
            }
        });
        let subscriptions = vec![cx.subscribe(&job_view, |_, _, event, cx| match event {
            JobViewEvent::Close => cx.emit(ExampleEvent::Close),
        })];
        Self {
            job_view,
            _writing: writing,
            _subscriptions: subscriptions,
        }
    }

    #[cfg(test)]
    pub fn job_view(&self) -> &Entity<JobView> {
        &self.job_view
    }
}
