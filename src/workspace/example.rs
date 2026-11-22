// An example, for a new user to see what Kron makes before giving it
// anything: the sample's résumé for one job, its lines citing their facts and
// the claims kept off it, and those facts on a facts page of its own, which
// leads on to the sample's other résumés. It has
// databases of its own, so nothing done here reaches the user's data; its
// views hold them, and drop them with it.

use super::parts::CONTROL;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, IntoElement, Subscription,
    Task, Window, div, prelude::*,
};

use super::facts_page::FactsPage;
use super::job_view::{JobView, JobViewEvent};
use super::parts::caption;
use super::previews::Previews;
use super::{OpenLine, ShowFact};
use crate::database::Database;
use crate::model::sample;

pub struct Example {
    database: Entity<Database>,
    job_view: Entity<JobView>,
    /// Alex's facts, made when one is first shown; `None` while the résumé is.
    facts_page: Option<Entity<FactsPage>>,
    showing_facts: bool,
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
        let (title, explanation) = if self.showing_facts {
            (
                "An example: Alex's career facts",
                "Each fact is quoted from Alex's CV. Used in lists the résumé lines built \
                 from it; click one to go back to it.",
            )
        } else {
            (
                "An example: a résumé Kron wrote for Alex",
                "Click any line to see the facts it came from. Left out lists what Alex's \
                 facts don't back, so it isn't on the page.",
            )
        };
        let page = match (&self.facts_page, self.showing_facts) {
            (Some(facts_page), true) => facts_page.clone().into_any_element(),
            _ => self.job_view.clone().into_any_element(),
        };
        v_flex()
            .size_full()
            // Kept here, so they show the example's own facts and lines
            // rather than the user's.
            .on_action(
                cx.listener(|this, action: &ShowFact, window, cx| {
                    this.show_fact(action, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, action: &OpenLine, window, cx| {
                    this.open_line(action, window, cx)
                }),
            )
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
                                    .child(title),
                            )
                            .child(caption(explanation, cx)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .when(self.showing_facts, |this| {
                                this.child(
                                    Button::new("example-back-to-resume")
                                        .label("Back to the résumé")
                                        .ghost()
                                        .with_size(CONTROL)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.show_resume(window, cx)
                                        })),
                                )
                            })
                            .child(
                                Button::new("close-example")
                                    .label("Start with your CV")
                                    .primary()
                                    .with_size(CONTROL)
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(ExampleEvent::Close)),
                                    ),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().child(page))
    }
}

impl Focusable for Example {
    fn focus_handle(&self, cx: &gpui_kit::App) -> FocusHandle {
        match (&self.facts_page, self.showing_facts) {
            (Some(facts_page), true) => facts_page.focus_handle(cx),
            _ => self.job_view.focus_handle(cx),
        }
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
            database: example,
            job_view,
            facts_page: None,
            showing_facts: false,
            _writing: writing,
            _subscriptions: subscriptions,
        }
    }

    fn show_fact(&mut self, action: &ShowFact, window: &mut Window, cx: &mut Context<Self>) {
        let database = self.database.clone();
        let facts_page = self
            .facts_page
            .get_or_insert_with(|| cx.new(|cx| FactsPage::new(database, window, cx)))
            .clone();
        facts_page.update(cx, |facts_page, cx| {
            facts_page.show_fact(action.fact, window, cx)
        });
        self.showing_facts = true;
        facts_page.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn open_line(&mut self, action: &OpenLine, window: &mut Window, cx: &mut Context<Self>) {
        self.show_resume(window, cx);
        self.job_view.update(cx, |job_view, cx| {
            job_view.set_job(action.job, cx);
            job_view.select_line(action.line, cx);
        });
    }

    fn show_resume(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.showing_facts = false;
        self.job_view.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    #[cfg(test)]
    pub fn job_view(&self) -> &Entity<JobView> {
        &self.job_view
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use crate::model::{FactId, JobId, LineId};
    use gpui_kit::TestAppContext;

    #[test]
    fn show_in_facts_opens_the_examples_own_facts_and_a_use_opens_its_resume_at_the_line() {
        let mut app = TestAppContext::single();
        let (database, _directory) = testing::open_empty(&mut app);
        let (example, cx) =
            app.add_window_view(|window, cx| Example::new(database.clone(), window, cx));
        testing::settle(cx, &database);
        cx.update(|window, cx| {
            let focus = example.read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        let cited = database.read_with(cx, |database, _| {
            database.connection().query_row(
                "SELECT line_citations.fact_id FROM line_citations
                 JOIN lines ON lines.id = line_citations.line_id
                 JOIN blocks ON blocks.id = lines.block_id
                 JOIN sections ON sections.id = blocks.section_id
                 JOIN current_generations
                   ON current_generations.generation_id = sections.generation_id
                 JOIN jobs ON jobs.id = current_generations.job_id
                 WHERE jobs.company = 'Cinder Labs' AND current_generations.kind = 'resume'
                 LIMIT 1",
                [],
                |row| Ok(FactId(row.get(0)?)),
            )
        });
        let fact = match cited {
            Ok(cited) => cited,
            Err(error) => panic!("the example's résumé cites no fact: {error}"),
        };

        cx.dispatch_action(ShowFact { fact });
        cx.run_until_parked();
        let shown = example.read_with(cx, |example, cx| {
            let facts_page = example.facts_page.as_ref()?;
            example
                .showing_facts
                .then(|| facts_page.read(cx).selected())
                .flatten()
        });
        assert_eq!(shown, Some(fact), "the example shows its own fact");

        // Its facts are used on the sample's other résumés too.
        let used = database.read_with(cx, |database, _| {
            database.connection().query_row(
                "SELECT jobs.id, line_citations.line_id FROM line_citations
                 JOIN lines ON lines.id = line_citations.line_id
                 JOIN blocks ON blocks.id = lines.block_id
                 JOIN sections ON sections.id = blocks.section_id
                 JOIN current_generations
                   ON current_generations.generation_id = sections.generation_id
                 JOIN jobs ON jobs.id = current_generations.job_id
                 WHERE line_citations.fact_id = ?1 AND jobs.company <> 'Cinder Labs'
                   AND current_generations.kind = 'resume'
                 LIMIT 1",
                [fact.0],
                |row| Ok((JobId(row.get(0)?), LineId(row.get(1)?))),
            )
        });
        let (other_job, other_line) = match used {
            Ok(used) => used,
            Err(error) => panic!("the fact is used on no other résumé: {error}"),
        };
        cx.dispatch_action(OpenLine {
            job: other_job,
            line: other_line,
        });
        cx.run_until_parked();
        let back = example.read_with(cx, |example, cx| {
            let job_view = example.job_view.read(cx);
            (
                example.showing_facts,
                job_view.job(),
                job_view.selected_line(),
            )
        });
        assert_eq!(
            back,
            (false, Some(other_job), Some(other_line)),
            "on that résumé, at the line"
        );
    }
}
