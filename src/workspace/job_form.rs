// The form a job is added with: its title, company, link and the
// description pasted from the posting. Fetching the description from the
// link comes later, so the link is kept.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, IntoElement, SharedString, Subscription, Task,
    Window, div, prelude::*,
};

use crate::database::Database;
use crate::model::JobId;
use crate::model::commands::{self, NewJob};
use super::parts::caption;

/// The longest each field may be, in characters, as the database keeps them.
const TITLE_LENGTH: usize = 256;
const COMPANY_LENGTH: usize = 256;
const LINK_LENGTH: usize = 2048;
const DESCRIPTION_LENGTH: usize = 20_480;

pub enum JobFormEvent {
    Added(JobId),
    Cancelled,
}

pub struct JobForm {
    database: Entity<Database>,
    title: Entity<InputState>,
    company: Entity<InputState>,
    link: Entity<InputState>,
    description: Entity<TextareaState>,
    /// Why the job couldn't be added.
    problem: Option<SharedString>,
    task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<JobFormEvent> for JobForm {}

impl Render for JobForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let filled = |input: &Entity<InputState>| !input.read(cx).value().trim().is_empty();
        let ready = filled(&self.title)
            && filled(&self.company)
            && !self.description.read(cx).value().trim().is_empty();
        let label = |text: &'static str| {
            div()
                .text_sm()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .child(text)
        };
        v_flex()
            .gap_4()
            .child(caption("Add the role you're interested in and the posting it belongs to.", cx))
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .min_w_0()
                            .child(label("Role"))
                            .child(Input::new(&self.title)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(label("Company"))
                            .child(Input::new(&self.company)),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Posting link (optional)"))
                    .child(Input::new(&self.link)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Job description"))
                    .child(Textarea::new(&self.description)),
            )
            .children(
                self.problem
                    .clone()
                    .map(|problem| div().text_sm().text_color(theme.danger).child(problem)),
            )
            .child(
                h_flex()
                    .pt_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-job")
                            .label("Cancel")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(JobFormEvent::Cancelled))),
                    )
                    .child(
                        Button::new("add-job")
                            .label("Add job")
                            .primary()
                            .small()
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, _, cx| this.add(cx))),
                    ),
            )
    }
}

impl JobForm {
    pub fn new(database: Entity<Database>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let title = input("Senior Backend Engineer", window, cx);
        let company = input("Company", window, cx);
        let link = input(
            "The posting's address, if you'd like to keep it",
            window,
            cx,
        );
        let description = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(8, 16)
                .placeholder("Paste the job's description from its posting.")
        });
        // Typing in any field changes whether the job can be added.
        let mut subscriptions: Vec<Subscription> = [&title, &company, &link]
            .into_iter()
            .map(|field| cx.observe(field, |_, _, cx| cx.notify()))
            .collect();
        subscriptions.push(cx.observe(&description, |_, _, cx| cx.notify()));
        Self {
            database,
            title,
            company,
            link,
            description,
            problem: None,
            task: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    /// Puts the cursor in the title. Once the dialog it's in has opened: the
    /// dialog takes focus as it opens.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.title.update(cx, |title, cx| title.focus(window, cx));
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let title = value(&self.title);
        let company = value(&self.company);
        let link = value(&self.link);
        let description = self.description.read(cx).value().trim().to_owned();
        if title.is_empty() || company.is_empty() || description.is_empty() {
            return;
        }
        let too_long = [
            ("title", &title, TITLE_LENGTH),
            ("company", &company, COMPANY_LENGTH),
            ("link", &link, LINK_LENGTH),
            ("description", &description, DESCRIPTION_LENGTH),
        ]
        .into_iter()
        .find(|(_, text, length)| text.chars().count() > *length);
        if let Some((field, _, length)) = too_long {
            self.problem =
                Some(format!("The {field} is longer than Kron keeps, {length} characters.").into());
            cx.notify();
            return;
        }
        let added = self.database.read(cx).write(cx, move |write| {
            commands::add_job(
                write,
                &NewJob {
                    title: &title,
                    company: &company,
                    url: Some(link.as_str()).filter(|link| !link.is_empty()),
                    description: &description,
                },
            )
        });
        self.task = cx.spawn(async move |this, cx| {
            let result = added.await;
            if let Err(error) = this.update(cx, |this, cx| match result {
                Ok(job) => cx.emit(JobFormEvent::Added(job)),
                Err(error) => {
                    eprintln!("job form: couldn't add the job: {error}");
                    this.problem = Some(format!("Couldn't add the job: {error}.").into());
                    cx.notify();
                }
            }) {
                eprintln!("job form: {error}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::TestAppContext;

    use super::*;
    use crate::workspace::testing;

    #[test]
    fn a_job_added_is_saved_wishlisted_with_its_description() {
        let mut app = TestAppContext::single();
        let (database, _directory) = testing::open_empty(&mut app);
        let (form, cx) = app.add_window_view({
            let database = database.clone();
            |window, cx| JobForm::new(database, window, cx)
        });
        let added = Rc::new(Cell::new(None));
        cx.update(|_, cx| {
            let added = added.clone();
            cx.subscribe(&form, move |_, event: &JobFormEvent, _| {
                if let JobFormEvent::Added(job) = event {
                    added.set(Some(*job));
                }
            })
            .detach();
        });
        let fill = |cx: &mut gpui_kit::VisualTestContext, title: &str, description: &str| {
            let (title_input, company, description_input) = form.read_with(cx, |form, _| {
                (
                    form.title.clone(),
                    form.company.clone(),
                    form.description.clone(),
                )
            });
            title_input.update_in(cx, |input, window, cx| input.set_value(title, window, cx));
            company.update_in(cx, |input, window, cx| {
                input.set_value(" Cinder Labs ", window, cx)
            });
            description_input.update_in(cx, |input, window, cx| {
                input.set_value(description, window, cx)
            });
        };

        fill(cx, "Senior Rust Engineer", "  ");
        form.update(cx, |form, cx| form.add(cx));
        testing::settle(cx, &database);
        assert_eq!(added.get(), None, "not without a description");

        fill(
            cx,
            "Senior Rust Engineer",
            "Build the ingest pipeline in Rust.\n",
        );
        form.update(cx, |form, cx| form.add(cx));
        testing::settle(cx, &database);
        let Some(job) = added.get() else {
            panic!("the job wasn't added");
        };
        let saved = database.read_with(cx, |database, _| {
            database.connection().query_row(
                "SELECT jobs.title, jobs.company, jobs.url, job_descriptions.text, events.kind
                 FROM jobs
                 JOIN job_descriptions ON job_descriptions.job_id = jobs.id
                 JOIN events ON events.job_id = jobs.id
                 WHERE jobs.id = ?1",
                [job.0],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
        });
        assert_eq!(
            saved.ok(),
            Some((
                "Senior Rust Engineer".to_owned(),
                "Cinder Labs".to_owned(),
                None,
                "Build the ingest pipeline in Rust.".to_owned(),
                "wishlisted".to_owned(),
            ))
        );
    }
}
