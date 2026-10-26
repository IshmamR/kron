// Adding a CV: a PDF is dropped or chosen, read on the device, and its text
// shown to check and correct before it's saved. What's saved is the text as
// corrected, since facts are found in it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

use super::parts::{caption, notice};
use crate::database::Database;
use crate::import::{self, Reading, Warning, pdf};
use crate::model::commands;

const PDF_MEDIA_TYPE: &str = "application/pdf";
/// Past this, lines of text get too long to read comfortably.
const PAGE_MAX_WIDTH: gpui_kit::Pixels = px(760.);

pub struct ImportPage {
    database: Entity<Database>,
    step: Step,
    /// The text read, for the user to correct.
    text: Entity<TextareaState>,
    focus_handle: FocusHandle,
    /// Reading the file or saving it; replaced, the one before stops.
    task: Task<()>,
}

impl Render for ImportPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.step {
            Step::Choose { error } => self.render_choose(error.clone(), cx).into_any_element(),
            Step::Reading { file_name } => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(caption(format!("Reading {file_name}…"), cx))
                .into_any_element(),
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
            step: Step::Choose { error: None },
            text: cx.new(|cx| TextareaState::new(window, cx)),
            focus_handle: cx.focus_handle(),
            task: Task::ready(()),
        }
    }

    /// Asks for a file with the system's file picker, and reads it.
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        self.task = cx.spawn_in(window, async move |this, cx| {
            let path = match chosen.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                // Cancelled, or the picker closed without an answer.
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(error)) => {
                    eprintln!("import page: the file picker failed: {error}");
                    None
                }
            };
            if let Some(path) = path
                && let Err(error) =
                    this.update_in(cx, |this, window, cx| this.open(path, window, cx))
            {
                eprintln!("import page: {error}");
            }
        });
    }

    /// Reads the file at `path` off the main thread, then shows its text, or
    /// why it couldn't be read.
    fn open(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let file_name: SharedString = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into();
        self.step = Step::Reading {
            file_name: file_name.clone(),
        };
        let read = cx.background_spawn(async move { read_file(&path) });
        self.task = cx.spawn_in(window, async move |this, cx| {
            let read = read.await;
            if let Err(error) = this.update_in(cx, |this, window, cx| {
                this.show(file_name, read, window, cx)
            }) {
                eprintln!("import page: {error}");
            }
        });
        cx.notify();
    }

    fn show(
        &mut self,
        file_name: SharedString,
        read: Result<(Vec<u8>, Reading), SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step = match read {
            Ok((bytes, reading)) => {
                self.text
                    .update(cx, |text, cx| text.set_value(reading.text, window, cx));
                Step::Confirm(Read {
                    file_name,
                    bytes: Arc::new(bytes),
                    warnings: reading.warnings,
                    saving: false,
                    error: None,
                })
            }
            Err(error) => Step::Choose { error: Some(error) },
        };
        cx.notify();
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
        read.error = None;
        let (file_name, bytes) = (read.file_name.clone(), read.bytes.clone());
        let saved = self.database.read(cx).write(cx, {
            let file_name = file_name.clone();
            move |write| commands::import_document(write, &file_name, &bytes, PDF_MEDIA_TYPE, &text)
        });
        self.task = cx.spawn_in(window, async move |this, cx| {
            let saved = saved.await;
            let updated = this.update_in(cx, |this, window, cx| {
                match saved {
                    Ok(_) => {
                        this.step = Step::Choose { error: None };
                        this.text
                            .update(cx, |text, cx| text.set_value("", window, cx));
                        // The kit's notifications live in its Root, which a
                        // test's window may not have.
                        if window.root::<Root>().flatten().is_some() {
                            window.push_notification(
                                Notification::new().message(format!("Saved {file_name}")),
                                cx,
                            );
                        }
                    }
                    Err(error) => {
                        eprintln!("import page: couldn't save the document: {error}");
                        if let Step::Confirm(read) = &mut this.step {
                            read.saving = false;
                            read.error = Some("Couldn't save it. Try again.".into());
                        }
                    }
                }
                cx.notify();
            });
            if let Err(error) = updated {
                eprintln!("import page: {error}");
            }
        });
        cx.notify();
    }

    fn render_choose(&self, error: Option<SharedString>, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .gap_3()
            .child(title("Import your CV"))
            .child(caption(
                "It's read on this device. You'll see the text before anything is saved.",
                cx,
            ))
            .when_some(error, |this, error| this.child(notice(error, theme.danger)))
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
                        if let Some(path) = paths.paths().first() {
                            this.open(path.clone(), window, cx);
                        }
                    }))
                    .child(
                        Icon::new(IconName::Inbox)
                            .size(px(32.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(div().child("Drop a PDF here"))
                    .child(
                        Button::new("browse")
                            .label("Choose a file…")
                            .primary()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                    ),
            )
    }

    fn render_confirm(&self, read: &Read, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let empty = self.text.read(cx).value().trim().is_empty();
        v_flex()
            .size_full()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(title("Check the text"))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("choose-another")
                                    .label("Choose another file")
                                    .ghost()
                                    .small()
                                    .disabled(read.saving)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.browse(window, cx)),
                                    ),
                            )
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
                    read.file_name
                ),
                cx,
            ))
            .when_some(read.error.clone(), |this, error| {
                this.child(notice(error, theme.danger))
            })
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
    /// Waiting for a file, with why the last one couldn't be read.
    Choose {
        error: Option<SharedString>,
    },
    Reading {
        file_name: SharedString,
    },
    Confirm(Read),
}

/// A file read, waiting for the user to check its text and save it.
struct Read {
    file_name: SharedString,
    bytes: Arc<Vec<u8>>,
    warnings: Vec<Warning>,
    saving: bool,
    /// Why saving failed.
    error: Option<SharedString>,
}

fn title(text: &'static str) -> impl IntoElement {
    div()
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

/// The file's bytes and what was read from them, or why they couldn't be,
/// in words for the user.
fn read_file(path: &Path) -> Result<(Vec<u8>, Reading), SharedString> {
    let bytes = fs::read(path).map_err(|error| format!("Couldn't open the file: {error}."))?;
    let reading = pdf::read(bytes.clone()).map_err(|error| match error {
        import::Error::Encrypted => {
            "This PDF is locked with a password. Save a copy without one and import that."
        }
        import::Error::Invalid => {
            "This isn't a PDF, or it's damaged. Only PDFs can be imported for now."
        }
        import::Error::NoText => {
            "This PDF has no text in it, only pictures of text, like a scan. Export your CV \
             again from where you wrote it, as a PDF."
        }
    })?;
    Ok((bytes, reading))
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

    struct Open<'a> {
        page: Entity<ImportPage>,
        database: Entity<Database>,
        cx: &'a mut VisualTestContext,
        _directory: TemporaryDirectory,
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
        fn import(&mut self, path: &str) {
            let path = PathBuf::from(path);
            self.page
                .update_in(self.cx, |page, window, cx| page.open(path, window, cx));
            self.cx.run_until_parked();
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

        fn error(&mut self) -> Option<SharedString> {
            self.page.read_with(self.cx, |page, _| match &page.step {
                Step::Choose { error } => error.clone(),
                Step::Reading { .. } | Step::Confirm(_) => None,
            })
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
        open.import(LATEX);
        let read = open.page.read_with(open.cx, |page, cx| {
            (
                matches!(page.step, Step::Confirm(_)),
                page.text.read(cx).value().to_string(),
            )
        });
        assert!(read.0, "the text is shown to check");
        assert!(read.1.starts_with("Alex Rahman\n"));
        open.set_text("Alex Rahman\nBackend engineer");
        open.save();
        assert_eq!(
            open.saved("latex.pdf"),
            [(
                "Alex Rahman\nBackend engineer".to_owned(),
                PDF_MEDIA_TYPE.to_owned()
            )]
        );
        assert_eq!(open.error(), None, "it's ready for the next file");
        assert!(
            open.page
                .read_with(open.cx, |page, _| matches!(page.step, Step::Choose { .. }))
        );
    }

    #[test]
    fn empty_text_is_not_saved() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.import(LATEX);
        open.set_text("  \n");
        open.save();
        assert_eq!(open.saved("latex.pdf"), []);
    }

    #[test]
    fn a_file_that_cant_be_read_says_why() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.import(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
        let error = open.error().unwrap_or_default();
        assert!(error.starts_with("This isn't a PDF"), "{error}");
        open.import(concat!(env!("CARGO_MANIFEST_DIR"), "/no such file.pdf"));
        let error = open.error().unwrap_or_default();
        assert!(error.starts_with("Couldn't open the file"), "{error}");
    }
}
