// Adding a CV: a PDF is dropped or chosen, read on the device, and its text
// shown to check and correct before it's saved. What's saved is the text as
// corrected, since facts are found in it. Files added together are taken one
// at a time, each asked about first in case it was added by mistake, as is a
// file imported before.

use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Local, Utc};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Root, Sizable, WindowExt, h_flex, v_flex,
};
use gpui_kit::{
    App, Context, Entity, ExternalPaths, FocusHandle, Focusable, FontWeight, IntoElement,
    PathPromptOptions, SharedString, Task, Window, div, prelude::*, px,
};
use rusqlite::{Connection, OptionalExtension};

use super::parts::{caption, notice};
use crate::database::{Database, Error};
use crate::import::{self, Warning, pdf};
use crate::model::commands;

const PDF_MEDIA_TYPE: &str = "application/pdf";
/// Past this, lines of text get too long to read comfortably.
const PAGE_MAX_WIDTH: gpui_kit::Pixels = px(760.);

pub struct ImportPage {
    database: Entity<Database>,
    step: Step,
    /// The files added with the one shown, waiting their turn.
    queue: VecDeque<PathBuf>,
    /// How many files were added together, to say which one this is.
    added: usize,
    /// Why the last file couldn't be read or saved.
    problem: Option<SharedString>,
    /// The text read, for the user to correct.
    text: Entity<TextareaState>,
    focus_handle: FocusHandle,
    /// Reading a file or saving it; replaced, the one before stops.
    task: Task<()>,
}

impl Render for ImportPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.step {
            Step::Choose => self.render_choose(cx).into_any_element(),
            Step::Reading { file_name } => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(caption(format!("Reading {file_name}…"), cx))
                .into_any_element(),
            Step::Ask(file) => self.render_ask(file, cx).into_any_element(),
            Step::Confirm(read) => self.render_confirm(read, cx).into_any_element(),
        };
        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .justify_center()
            .child(div().size_full().max_w(PAGE_MAX_WIDTH).p_4().child(content))
    }
}

impl Focusable for ImportPage {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ImportPage {
    pub fn new(database: Entity<Database>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            database,
            step: Step::Choose,
            queue: VecDeque::new(),
            added: 0,
            problem: None,
            text: cx.new(|cx| TextareaState::new(window, cx)),
            focus_handle: cx.focus_handle(),
            task: Task::ready(()),
        }
    }

    /// Asks for files with the system's file picker, and imports them.
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import".into()),
        });
        self.task = cx.spawn_in(window, async move |this, cx| {
            let paths = match chosen.await {
                Ok(Ok(Some(paths))) => paths,
                // Cancelled, or the picker closed without an answer.
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    eprintln!("import page: the file picker failed: {error}");
                    return;
                }
            };
            if let Err(error) =
                this.update_in(cx, |this, window, cx| this.import(paths, window, cx))
            {
                eprintln!("import page: {error}");
            }
        });
    }

    /// Takes the files one at a time, from the first.
    fn import(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        self.added = paths.len();
        self.queue = paths.into();
        self.problem = None;
        self.next(window, cx);
    }

    /// Loads the next file waiting, or goes back to choosing once there's
    /// none. A file is loaded and checked against earlier imports before
    /// its text is read.
    fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.text
            .update(cx, |text, cx| text.set_value("", window, cx));
        let Some(path) = self.queue.pop_front() else {
            self.added = 0;
            self.step = Step::Choose;
            cx.notify();
            return;
        };
        let file_name: SharedString = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into();
        self.step = Step::Reading {
            file_name: file_name.clone(),
        };
        let loaded = cx.background_spawn(async move {
            fs::read(&path).map(|bytes| {
                let hash = *blake3::hash(&bytes).as_bytes();
                (bytes, hash)
            })
        });
        self.task = cx.spawn_in(window, async move |this, cx| {
            let loaded = loaded.await;
            let updated = this.update_in(cx, |this, window, cx| match loaded {
                Ok((bytes, hash)) => {
                    let imported = imported_before(this.database.read(cx).connection(), hash)
                        .unwrap_or_else(|error| {
                            eprintln!("import page: couldn't look for earlier imports: {error}");
                            None
                        });
                    let file = File {
                        file_name,
                        bytes: Arc::new(bytes),
                        imported,
                    };
                    if this.added > 1 || file.imported.is_some() {
                        this.step = Step::Ask(file);
                        cx.notify();
                    } else {
                        this.read(file, window, cx);
                    }
                }
                Err(error) => {
                    this.problem = Some(format!("{file_name}: couldn't open it: {error}.").into());
                    this.next(window, cx);
                }
            });
            if let Err(error) = updated {
                eprintln!("import page: {error}");
            }
        });
        cx.notify();
    }

    /// Reads the file's text off the main thread, then shows it to check,
    /// or why it couldn't be read.
    fn read(&mut self, file: File, window: &mut Window, cx: &mut Context<Self>) {
        self.step = Step::Reading {
            file_name: file.file_name.clone(),
        };
        let bytes = file.bytes.to_vec();
        let reading = cx.background_spawn(async move { pdf::read(bytes) });
        self.task = cx.spawn_in(window, async move |this, cx| {
            let reading = reading.await;
            let updated = this.update_in(cx, |this, window, cx| match reading {
                Ok(reading) => {
                    this.text
                        .update(cx, |text, cx| text.set_value(reading.text, window, cx));
                    this.step = Step::Confirm(Read {
                        file,
                        warnings: reading.warnings,
                        saving: false,
                    });
                    cx.notify();
                }
                Err(error) => {
                    this.problem =
                        Some(format!("{}: {}", file.file_name, error_message(error)).into());
                    this.next(window, cx);
                }
            });
            if let Err(error) = updated {
                eprintln!("import page: {error}");
            }
        });
        cx.notify();
    }

    /// Reads the file being asked about.
    fn read_asked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let step = std::mem::replace(&mut self.step, Step::Choose);
        if let Step::Ask(file) = step {
            self.problem = None;
            self.read(file, window, cx);
        } else {
            self.step = step;
        }
    }

    /// Leaves the file shown, unsaved, for the next.
    fn skip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.problem = None;
        self.next(window, cx);
    }

    /// Leaves the file shown and every one waiting, unsaved.
    fn skip_rest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.queue.clear();
        self.skip(window, cx);
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Step::Confirm(read) = &mut self.step else {
            return;
        };
        let text = self.text.read(cx).value().to_string();
        if read.saving || text.trim().is_empty() {
            return;
        }
        read.saving = true;
        self.problem = None;
        let (file_name, bytes) = (read.file.file_name.clone(), read.file.bytes.clone());
        let saved = self.database.read(cx).write(cx, {
            let file_name = file_name.clone();
            move |write| commands::import_document(write, &file_name, &bytes, PDF_MEDIA_TYPE, &text)
        });
        self.task = cx.spawn_in(window, async move |this, cx| {
            let saved = saved.await;
            let updated = this.update_in(cx, |this, window, cx| match saved {
                Ok(_) => {
                    // The kit's notifications live in its Root, which a
                    // test's window may not have.
                    if window.root::<Root>().flatten().is_some() {
                        window.push_notification(
                            Notification::new().message(format!("Saved {file_name}")),
                            cx,
                        );
                    }
                    this.next(window, cx);
                }
                Err(error) => {
                    eprintln!("import page: couldn't save the document: {error}");
                    if let Step::Confirm(read) = &mut this.step {
                        read.saving = false;
                    }
                    this.problem =
                        Some(format!("{file_name}: couldn't save it. Try again.").into());
                    cx.notify();
                }
            });
            if let Err(error) = updated {
                eprintln!("import page: {error}");
            }
        });
        cx.notify();
    }

    /// Which of the files added together is shown, when there are several.
    fn position(&self) -> Option<String> {
        (self.added > 1)
            .then(|| format!("File {} of {}", self.added - self.queue.len(), self.added))
    }

    fn render_problem(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        self.problem
            .clone()
            .map(|problem| notice(problem, cx.theme().danger))
    }

    fn render_choose(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .gap_3()
            .child(title("Import your CV".into()))
            .child(caption(
                "It's read on this device. You'll see the text before anything is saved.",
                cx,
            ))
            .children(self.render_problem(cx))
            .child(
                v_flex()
                    .id("drop-target")
                    .flex_1()
                    .min_h(px(240.))
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .rounded(theme.radius_lg)
                    .border_2()
                    .border_dashed()
                    .border_color(theme.border)
                    .drag_over::<ExternalPaths>(|style, _, _, cx| {
                        style
                            .border_color(cx.theme().primary)
                            .bg(cx.theme().primary.opacity(0.05))
                    })
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                        this.import(paths.paths().to_vec(), window, cx)
                    }))
                    .child(
                        Icon::new(IconName::Inbox)
                            .size(px(32.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(div().child("Drop PDFs here"))
                    .child(
                        Button::new("browse")
                            .label("Choose files…")
                            .primary()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                    ),
            )
    }

    fn render_ask(&self, file: &File, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let earlier = file.imported.as_ref().map(|imported| {
            let date = imported.added_at.with_timezone(&Local).format("%b %-d, %Y");
            if imported.file_name == file.file_name.as_ref() {
                format!("You already imported this file on {date}.")
            } else {
                format!(
                    "You already imported this file on {date}, as {}.",
                    imported.file_name
                )
            }
        });
        v_flex()
            .gap_3()
            .children(self.position().map(|position| caption(position, cx)))
            .child(title(format!("Import {}?", file.file_name).into()))
            .children(self.render_problem(cx))
            .when(self.added > 1, |this| {
                this.child(caption(
                    "Several files were added at once, so each is checked before it's read, in \
                     case one was added by mistake.",
                    cx,
                ))
            })
            .children(earlier.map(|earlier| notice(earlier, theme.warning)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("read")
                            .label("Read it")
                            .primary()
                            .small()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.read_asked(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("skip")
                            .label("Skip")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.skip(window, cx))),
                    )
                    .when(!self.queue.is_empty(), |this| {
                        this.child(
                            Button::new("skip-rest")
                                .label("Skip the rest")
                                .ghost()
                                .small()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.skip_rest(window, cx)),
                                ),
                        )
                    }),
            )
    }

    fn render_confirm(&self, read: &Read, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let empty = self.text.read(cx).value().trim().is_empty();
        // Among several files, the next is already chosen.
        let other = if self.added > 1 {
            Button::new("skip")
                .label("Skip")
                .on_click(cx.listener(|this, _, window, cx| this.skip(window, cx)))
        } else {
            Button::new("choose-another")
                .label("Choose another file")
                .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx)))
        };
        v_flex()
            .size_full()
            .gap_3()
            .children(self.position().map(|position| caption(position, cx)))
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(title("Check the text".into()))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("cancel")
                                    .label("Cancel")
                                    .ghost()
                                    .small()
                                    .disabled(read.saving)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.skip_rest(window, cx)
                                    })),
                            )
                            .child(other.ghost().small().disabled(read.saving))
                            .child(
                                Button::new("save")
                                    .label("Save")
                                    .primary()
                                    .small()
                                    .disabled(read.saving || empty)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.save(window, cx)),
                                    ),
                            ),
                    ),
            )
            .child(caption(
                format!(
                    "The text read from {}. Correct anything that's wrong: facts are taken from \
                     this text. Nothing has been sent anywhere.",
                    read.file.file_name
                ),
                cx,
            ))
            .children(self.render_problem(cx))
            .children(
                read.warnings
                    .iter()
                    .map(|warning| notice(warning_message(warning), theme.warning)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Textarea::new(&self.text).h_full()),
            )
    }
}

enum Step {
    Choose,
    /// Loading a file, or reading its text.
    Reading {
        file_name: SharedString,
    },
    /// Asking whether to read a file: one of several added together, or one
    /// imported before.
    Ask(File),
    Confirm(Read),
}

/// A file loaded, not read yet.
struct File {
    file_name: SharedString,
    bytes: Arc<Vec<u8>>,
    /// The last import of the same file, if it was imported before.
    imported: Option<Imported>,
}

struct Imported {
    file_name: String,
    added_at: DateTime<Utc>,
}

/// A file read, waiting for the user to check its text and save it.
struct Read {
    file: File,
    warnings: Vec<Warning>,
    saving: bool,
}

fn title(text: SharedString) -> impl IntoElement {
    div()
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

/// The latest import of the file with this hash that's still there.
fn imported_before(connection: &Connection, hash: [u8; 32]) -> Result<Option<Imported>, Error> {
    Ok(connection
        .query_row(
            "SELECT file_name, added_at FROM documents
             WHERE file_hash = ?1 AND deleted_at IS NULL
             ORDER BY added_at DESC LIMIT 1",
            [hash],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
        .map(|(file_name, added_at)| Imported {
            file_name,
            added_at: DateTime::from_timestamp_millis(added_at).unwrap_or_default(),
        }))
}

/// Why a file couldn't be read, in words for the user.
fn error_message(error: import::Error) -> &'static str {
    match error {
        import::Error::Encrypted => {
            "this PDF is locked with a password. Save a copy without one and import that."
        }
        import::Error::Invalid => {
            "this isn't a PDF, or it's damaged. Only PDFs can be imported for now."
        }
        import::Error::NoText => {
            "this PDF has no text in it, only pictures of text, like a scan. Export your CV \
             again from where you wrote it, as a PDF."
        }
    }
}

fn warning_message(warning: &Warning) -> String {
    match warning {
        Warning::NoText { page } => format!(
            "Page {} is only pictures, like a scan, so none of its text is below. Type in \
             what it says.",
            page + 1
        ),
        Warning::DrawnFont { page } => format!(
            "Page {} uses a font whose text can't be fully read, so some of its words may be \
             split up or missing below.",
            page + 1
        ),
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::database::TemporaryDirectory;
    use crate::workspace::testing;

    const LATEX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/import/pdf/latex.pdf");
    const SIDEBAR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/import/pdf/sidebar.pdf");
    const NOT_A_PDF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");

    struct Open<'a> {
        page: Entity<ImportPage>,
        database: Entity<Database>,
        cx: &'a mut VisualTestContext,
        _directory: TemporaryDirectory,
    }

    /// Where the page is, as a test checks it.
    #[derive(Debug, PartialEq)]
    enum Shown {
        Choose,
        Reading,
        /// The file asked about, and whether it was imported before.
        Ask(String, bool),
        Confirm(String),
    }

    fn open(app: &mut TestAppContext) -> Open<'_> {
        let (database, directory) = testing::open_sample(app);
        let (page, cx) =
            app.add_window_view(|window, cx| ImportPage::new(database.clone(), window, cx));
        Open {
            page,
            database,
            cx,
            _directory: directory,
        }
    }

    impl Open<'_> {
        fn import(&mut self, paths: &[&str]) {
            let paths = paths.iter().map(PathBuf::from).collect();
            self.page
                .update_in(self.cx, |page, window, cx| page.import(paths, window, cx));
            self.cx.run_until_parked();
        }

        fn act(
            &mut self,
            action: impl FnOnce(&mut ImportPage, &mut Window, &mut Context<ImportPage>),
        ) {
            self.page.update_in(self.cx, action);
            self.cx.run_until_parked();
        }

        fn shown(&mut self) -> Shown {
            self.page.read_with(self.cx, |page, _| match &page.step {
                Step::Choose => Shown::Choose,
                Step::Reading { .. } => Shown::Reading,
                Step::Ask(file) => Shown::Ask(file.file_name.to_string(), file.imported.is_some()),
                Step::Confirm(read) => Shown::Confirm(read.file.file_name.to_string()),
            })
        }

        fn text(&mut self) -> String {
            self.page
                .read_with(self.cx, |page, cx| page.text.read(cx).value().to_string())
        }

        fn set_text(&mut self, value: &str) {
            let text = self.page.read_with(self.cx, |page, _| page.text.clone());
            text.update_in(self.cx, |text, window, cx| {
                text.set_value(value.to_owned(), window, cx)
            });
        }

        fn save(&mut self) {
            self.page
                .update_in(self.cx, |page, window, cx| page.save(window, cx));
            testing::settle(self.cx, &self.database);
        }

        fn problem(&mut self) -> String {
            self.page
                .read_with(self.cx, |page, _| page.problem.clone().unwrap_or_default())
                .to_string()
        }

        /// The saved texts of documents imported from `file_name`.
        fn saved(&mut self, file_name: &str) -> Vec<(String, String)> {
            let saved = self.database.read_with(self.cx, |database, _| {
                database
                    .connection()
                    .prepare(
                        "SELECT document_texts.text, files.media_type
                         FROM documents
                         JOIN document_texts ON document_texts.document_id = documents.id
                         JOIN files ON files.hash = documents.file_hash
                         WHERE documents.file_name = ?1",
                    )
                    .and_then(|mut statement| {
                        statement
                            .query_map([file_name], |row| Ok((row.get(0)?, row.get(1)?)))?
                            .collect::<Result<Vec<_>, _>>()
                    })
            });
            match saved {
                Ok(saved) => saved,
                Err(error) => panic!("couldn't read the documents: {error}"),
            }
        }
    }

    #[test]
    fn a_pdf_is_read_and_saved_with_the_text_as_corrected() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.import(&[LATEX]);
        assert_eq!(
            open.shown(),
            Shown::Confirm("latex.pdf".into()),
            "one new file isn't asked about"
        );
        assert!(open.text().starts_with("Alex Rahman\n"));
        open.set_text("Alex Rahman\nBackend engineer");
        open.save();
        assert_eq!(
            open.saved("latex.pdf"),
            [(
                "Alex Rahman\nBackend engineer".to_owned(),
                PDF_MEDIA_TYPE.to_owned()
            )]
        );
        assert_eq!(open.shown(), Shown::Choose, "it's ready for the next file");
    }

    #[test]
    fn empty_text_is_not_saved() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.import(&[LATEX]);
        open.set_text("  \n");
        open.save();
        assert_eq!(open.saved("latex.pdf"), []);
    }

    #[test]
    fn a_file_read_can_be_left_without_saving() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.import(&[LATEX]);
        open.act(|page, window, cx| page.skip_rest(window, cx));
        assert_eq!(open.shown(), Shown::Choose);
        assert_eq!(open.text(), "", "its text doesn't linger");

        open.import(&[LATEX, SIDEBAR]);
        open.act(|page, window, cx| page.read_asked(window, cx));
        assert_eq!(open.shown(), Shown::Confirm("latex.pdf".into()));
        open.act(|page, window, cx| page.skip_rest(window, cx));
        assert_eq!(
            open.shown(),
            Shown::Choose,
            "the files waiting are left too"
        );
        assert_eq!(open.text(), "");
        assert_eq!(open.saved("latex.pdf"), []);
        assert_eq!(open.saved("sidebar.pdf"), []);
    }

    #[test]
    fn a_file_that_cant_be_read_says_why() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.import(&[NOT_A_PDF]);
        assert_eq!(open.shown(), Shown::Choose);
        let problem = open.problem();
        assert!(
            problem.starts_with("Cargo.toml: this isn't a PDF"),
            "{problem}"
        );
        open.import(&[concat!(env!("CARGO_MANIFEST_DIR"), "/no such file.pdf")]);
        let problem = open.problem();
        assert!(
            problem.starts_with("no such file.pdf: couldn't open it"),
            "{problem}"
        );
    }

    #[test]
    fn a_file_imported_before_is_asked_about_first() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.import(&[LATEX]);
        open.save();
        open.import(&[LATEX]);
        assert_eq!(open.shown(), Shown::Ask("latex.pdf".into(), true));
        open.act(|page, window, cx| page.skip(window, cx));
        assert_eq!(open.shown(), Shown::Choose);
        assert_eq!(open.saved("latex.pdf").len(), 1);

        open.import(&[LATEX]);
        open.act(|page, window, cx| page.read_asked(window, cx));
        assert_eq!(open.shown(), Shown::Confirm("latex.pdf".into()));
        open.save();
        assert_eq!(
            open.saved("latex.pdf").len(),
            2,
            "importing it again is the user's call"
        );
    }

    #[test]
    fn files_added_together_are_asked_about_one_at_a_time() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.import(&[LATEX, NOT_A_PDF, SIDEBAR, LATEX]);
        assert_eq!(open.shown(), Shown::Ask("latex.pdf".into(), false));
        open.act(|page, window, cx| page.skip(window, cx));
        assert_eq!(open.shown(), Shown::Ask("Cargo.toml".into(), false));
        open.act(|page, window, cx| page.read_asked(window, cx));
        assert_eq!(open.shown(), Shown::Ask("sidebar.pdf".into(), false));
        let problem = open.problem();
        assert!(
            problem.starts_with("Cargo.toml: this isn't a PDF"),
            "{problem}"
        );
        open.act(|page, window, cx| page.read_asked(window, cx));
        assert_eq!(open.shown(), Shown::Confirm("sidebar.pdf".into()));
        open.save();
        assert_eq!(open.shown(), Shown::Ask("latex.pdf".into(), false));
        open.act(|page, window, cx| page.skip_rest(window, cx));
        assert_eq!(open.shown(), Shown::Choose);
        assert_eq!(open.saved("sidebar.pdf").len(), 1);
        assert_eq!(open.saved("latex.pdf"), [], "skipped files aren't saved");
    }
}
