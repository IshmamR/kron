// What a new user sees first: a welcome around the import page, then the
// name their CV seemed to give, to confirm. Shown while the user has nothing
// yet: no documents, jobs or name. Data arriving another way, like the
// sample seeded in debug builds or a sync, ends it.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::{
    Context, Entity, EventEmitter, FontWeight, IntoElement, SharedString, Subscription, Task,
    Window, div, prelude::*,
};
use rusqlite::Connection;

use super::import_page::{ImportPage, ImportPageEvent, PAGE_MAX_WIDTH};
use super::parts::{caption, notice};
use crate::database::{Changed, Database};
use crate::model::DocumentId;
use crate::model::commands;

pub struct Onboarding {
    database: Entity<Database>,
    import_page: Entity<ImportPage>,
    step: Step,
    /// The first document saved, and the name it seemed to give.
    found: Option<Found>,
    name: Entity<InputState>,
    /// Why the name couldn't be saved.
    problem: Option<SharedString>,
    saving: Task<()>,
    _subscriptions: Vec<Subscription>,
}

pub enum OnboardingEvent {
    Done,
}

impl EventEmitter<OnboardingEvent> for Onboarding {}

impl Render for Onboarding {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.step {
            Step::Import => self.render_import(cx).into_any_element(),
            Step::Name => self.render_name(cx).into_any_element(),
        }
    }
}

impl Onboarding {
    pub fn new(
        database: Entity<Database>,
        import_page: Entity<ImportPage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Your name"));
        let subscriptions = vec![
            cx.subscribe_in(
                &import_page,
                window,
                |this, _, event, window, cx| match event {
                    ImportPageEvent::Saved {
                        document,
                        file_name,
                        name,
                    } => {
                        this.found.get_or_insert_with(|| Found {
                            document: *document,
                            file_name: file_name.clone(),
                            name: name.clone(),
                        });
                    }
                    ImportPageEvent::Finished => {
                        if this.found.is_some() && matches!(this.step, Step::Import) {
                            this.ask_name(window, cx);
                        }
                    }
                },
            ),
            cx.subscribe(&database, |this, database, _: &Changed, cx| {
                // While its own import saves, the import page isn't choosing;
                // once saved, the name is asked.
                if !matches!(this.step, Step::Import) || !this.import_page.read(cx).is_choosing() {
                    return;
                }
                match is_new(database.read(cx).connection()) {
                    Ok(true) => {}
                    Ok(false) => cx.emit(OnboardingEvent::Done),
                    Err(error) => eprintln!("onboarding: couldn't check for data: {error}"),
                }
            }),
            cx.subscribe_in(&name, window, |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.confirm(window, cx),
                InputEvent::Change => cx.notify(),
                InputEvent::Focus | InputEvent::Blur => {}
            }),
        ];
        Self {
            database,
            import_page,
            step: Step::Import,
            found: None,
            name,
            problem: None,
            saving: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    fn ask_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step = Step::Name;
        let guess = self
            .found
            .as_ref()
            .and_then(|found| found.name.clone())
            .unwrap_or_default();
        self.name.update(cx, |name, cx| {
            name.set_value(guess, window, cx);
            name.focus(window, cx);
        });
        cx.notify();
    }

    fn confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return;
        }
        let document = self.found.as_ref().map(|found| found.document);
        let saved = self
            .database
            .read(cx)
            .write(cx, move |write| commands::add_name(write, &name, document));
        self.saving = cx.spawn(async move |this, cx| {
            let saved = saved.await;
            let updated = this.update(cx, |this, cx| match saved {
                Ok(_) => cx.emit(OnboardingEvent::Done),
                Err(error) => {
                    eprintln!("onboarding: couldn't save the name: {error}");
                    this.problem = Some("Couldn't save your name. Try again.".into());
                    cx.notify();
                }
            });
            if let Err(error) = updated {
                eprintln!("onboarding: {error}");
            }
        });
    }

    fn render_import(&self, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                h_flex().justify_center().child(
                    h_flex()
                        .w_full()
                        .max_w(PAGE_MAX_WIDTH)
                        .px_4()
                        .pt_8()
                        .gap_4()
                        .items_start()
                        .justify_between()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xl()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Welcome to Kron"),
                                )
                                .child(caption(
                                    "Kron builds every résumé from the facts of your work, each line citing \
                                     its source.",
                                    cx,
                                )),
                        )
                        .child(
                            Button::new("skip-onboarding")
                                .label("Skip for now")
                                .ghost()
                                .small()
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(OnboardingEvent::Done)),
                                ),
                        ),
                ),
            )
            .child(div().flex_1().min_h_0().child(self.import_page.clone()))
    }

    fn render_name(&self, cx: &Context<Self>) -> impl IntoElement {
        let found = self.found.as_ref();
        let guessed = found.is_some_and(|found| found.name.is_some());
        let title = if guessed {
            "Is this you?"
        } else {
            "What's your name?"
        };
        let description = found.map(Found::description).unwrap_or_default();
        let empty = self.name.read(cx).value().trim().is_empty();
        h_flex().size_full().items_start().justify_center().child(
            v_flex()
                .w_full()
                .max_w(PAGE_MAX_WIDTH)
                .px_4()
                .pt_8()
                .gap_3()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(caption(description, cx))
                .children(
                    self.problem
                        .clone()
                        .map(|problem| notice(problem, cx.theme().danger)),
                )
                .child(Input::new(&self.name))
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("skip-name")
                                .label("Skip")
                                .ghost()
                                .small()
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(OnboardingEvent::Done)),
                                ),
                        )
                        .child(
                            Button::new("confirm-name")
                                .label("Confirm")
                                .primary()
                                .small()
                                .disabled(empty)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.confirm(window, cx)),
                                ),
                        ),
                ),
        )
    }
}

enum Step {
    Import,
    Name,
}

struct Found {
    document: DocumentId,
    file_name: Option<SharedString>,
    name: Option<String>,
}

impl Found {
    /// Where the name was looked for, and whether it was found there.
    fn description(&self) -> String {
        let source = match &self.file_name {
            Some(file_name) => file_name.to_string(),
            None => "your text".to_owned(),
        };
        let looked = match self.name {
            Some(_) => format!("Found in {source}."),
            None => format!("Couldn't find your name in {source}."),
        };
        format!("{looked} It goes at the top of your résumés.")
    }
}

/// Whether the user has nothing yet: no documents, jobs or name.
pub fn is_new(connection: &Connection) -> Result<bool, rusqlite::Error> {
    connection.query_row(
        "SELECT NOT (
             EXISTS (SELECT 1 FROM documents WHERE deleted_at IS NULL)
             OR EXISTS (SELECT 1 FROM jobs WHERE deleted_at IS NULL)
             OR EXISTS (SELECT 1 FROM facts WHERE kind = 'name' AND deleted_at IS NULL)
         )",
        [],
        |row| row.get(0),
    )
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::database::TemporaryDirectory;
    use crate::workspace::testing;

    const LATEX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/import/pdf/latex.pdf");

    struct Open<'a> {
        onboarding: Entity<Onboarding>,
        database: Entity<Database>,
        cx: &'a mut VisualTestContext,
        done: Rc<Cell<bool>>,
        _done: Subscription,
        _directory: TemporaryDirectory,
    }

    fn open(app: &mut TestAppContext) -> Open<'_> {
        let (database, directory) = testing::open_empty(app);
        let (onboarding, cx) = app.add_window_view(|window, cx| {
            let import_page = cx.new(|cx| ImportPage::new(database.clone(), window, cx));
            Onboarding::new(database.clone(), import_page, window, cx)
        });
        let done = Rc::new(Cell::new(false));
        let subscription = cx.update(|_, cx| {
            let done = done.clone();
            cx.subscribe(&onboarding, move |_, _: &OnboardingEvent, _| done.set(true))
        });
        Open {
            onboarding,
            database,
            cx,
            done,
            _done: subscription,
            _directory: directory,
        }
    }

    impl Open<'_> {
        fn import_page(&mut self) -> Entity<ImportPage> {
            self.onboarding
                .read_with(self.cx, |onboarding, _| onboarding.import_page.clone())
        }

        /// Imports a file and saves its text, as the user would.
        fn import(&mut self, path: &str) {
            let import_page = self.import_page();
            import_page.update_in(self.cx, |page, window, cx| {
                page.import(vec![path.into()], window, cx);
            });
            self.cx.run_until_parked();
            import_page.update_in(self.cx, |page, window, cx| page.save(window, cx));
            testing::settle(self.cx, &self.database);
        }

        /// Writes text and saves it, as the user would.
        fn write(&mut self, text: &str) {
            let import_page = self.import_page();
            import_page.update_in(self.cx, |page, window, cx| {
                page.write(text.to_owned(), window, cx);
                page.save(window, cx);
            });
            testing::settle(self.cx, &self.database);
        }

        fn asking_name(&mut self) -> Option<String> {
            self.onboarding.read_with(self.cx, |onboarding, cx| {
                matches!(onboarding.step, Step::Name)
                    .then(|| onboarding.name.read(cx).value().to_string())
            })
        }

        fn names(&mut self) -> Vec<(String, Option<i64>)> {
            let names = self.database.read_with(self.cx, |database, _| {
                database
                    .connection()
                    .prepare(
                        "SELECT facts.text, fact_sources.span_start
                         FROM facts
                         LEFT JOIN fact_sources ON fact_sources.fact_id = facts.id
                         WHERE facts.kind = 'name'",
                    )
                    .and_then(|mut statement| {
                        statement
                            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                            .collect::<Result<Vec<_>, _>>()
                    })
            });
            match names {
                Ok(names) => names,
                Err(error) => panic!("couldn't read the names: {error}"),
            }
        }
    }

    #[test]
    fn the_name_found_in_the_first_import_is_confirmed() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.import(LATEX);
        assert_eq!(open.asking_name().as_deref(), Some("Alex Rahman"));
        open.onboarding
            .update_in(open.cx, |onboarding, window, cx| {
                onboarding.confirm(window, cx)
            });
        testing::settle(open.cx, &open.database);
        assert!(open.done.get());
        assert_eq!(open.names(), [("Alex Rahman".to_owned(), Some(0))]);
    }

    #[test]
    fn without_a_name_found_it_asks_with_the_box_empty() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.write("Experience\nLed the migration.\n");
        assert_eq!(open.asking_name().as_deref(), Some(""));
        let description = open.onboarding.read_with(open.cx, |onboarding, _| {
            onboarding.found.as_ref().map(Found::description)
        });
        assert_eq!(
            description.as_deref(),
            Some("Couldn't find your name in your text. It goes at the top of your résumés.")
        );
        open.onboarding
            .update_in(open.cx, |onboarding, window, cx| {
                onboarding.confirm(window, cx)
            });
        testing::settle(open.cx, &open.database);
        assert!(!open.done.get(), "an empty name isn't confirmed");
        assert_eq!(open.names(), []);
    }

    #[test]
    fn data_arriving_another_way_ends_it() {
        let mut app = TestAppContext::single();
        let open = open(&mut app);
        assert!(!open.done.get());
        // As the sample is seeded in debug builds, after the window opens.
        testing::run(open.cx, &open.database, testing::write_sample);
        assert!(open.done.get());
    }

    #[test]
    fn its_own_import_doesnt_end_it() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.write("Alex Rahman\nalex@example.com\n");
        assert!(!open.done.get(), "the name is asked first");
        assert!(open.asking_name().is_some());
    }
}
