// Adding a CV: a PDF, Markdown or text file is dropped or chosen, read on
// the device, and its text shown to check and correct before it's saved; or
// text is pasted or written straight in. What's saved is the text as
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
    KeyBinding, PathPromptOptions, SharedString, Task, Window, actions, div, prelude::*, px,
};
use rusqlite::{Connection, OptionalExtension};

use super::parts::{caption, notice};
use crate::database::{Database, Error};
use crate::import::{self, Format, Warning};
use crate::model::commands::{self, DocumentFile};

const KEY_CONTEXT: &str = "ImportPage";
/// Past this, lines of text get too long to read comfortably.
const PAGE_MAX_WIDTH: gpui_kit::Pixels = px(760.);

actions!(import_page, [Paste, Write, Cancel, Save]);

pub fn init(cx: &mut App) {
    // A text field's own paste comes first: its key context is deeper.
    cx.bind_keys([
        KeyBinding::new("secondary-v", Paste, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-n", Write, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(KEY_CONTEXT)),
        // In the text field, where its own Ctrl+Enter would add a line.
        KeyBinding::new(
            "secondary-enter",
            Save,
            Some(&format!("{KEY_CONTEXT} > Input")),
        ),
    ]);
}

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
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &Paste, window, cx| this.paste(window, cx)))
            .on_action(cx.listener(|this, _: &Write, window, cx| this.start_writing(window, cx)))
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &Cancel, window, cx| {
                if !matches!(this.step, Step::Choose) {
                    this.skip_rest(window, cx)
                }
            }))
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
            text: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Your CV, or anything else about your work.")
            }),
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
        // The text field is about to go, and with nothing focused the
        // page's keys, and the app's, stop working.
        if self.text.focus_handle(cx).is_focused(window) {
            self.focus_handle.focus(window, cx);
        }
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
            let loaded = loaded.map(|(bytes, hash)| (Format::of(&file_name, &bytes), bytes, hash));
            let updated = this.update_in(cx, |this, window, cx| match loaded {
                Ok((Err(error), _, _)) => {
                    this.problem = Some(format!("{file_name}: {}", error_message(error)).into());
                    this.next(window, cx);
                }
                Ok((Ok(format), bytes, hash)) => {
                    let imported = imported_before(this.database.read(cx).connection(), hash)
                        .unwrap_or_else(|error| {
                            eprintln!("import page: couldn't look for earlier imports: {error}");
                            None
                        });
                    let file = File {
                        file_name,
                        format,
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
        let (format, bytes) = (file.format, file.bytes.to_vec());
        let reading = cx.background_spawn(async move { import::read(format, bytes) });
        self.task = cx.spawn_in(window, async move |this, cx| {
            let reading = reading.await;
            let updated = this.update_in(cx, |this, window, cx| match reading {
                Ok(reading) => {
                    this.text
                        .update(cx, |text, cx| text.set_value(reading.text, window, cx));
                    this.step = Step::Confirm(Read {
                        file: Some(file),
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

    /// Opens the text on the clipboard to check, or an empty page to write
    /// on when there's none.
    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.step, Step::Choose) {
            return;
        }
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        self.write(text, window, cx);
    }

    /// Opens an empty page to write on.
    fn start_writing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.step, Step::Choose) {
            self.write(String::new(), window, cx);
        }
    }

    /// Opens text with no file to check and save, or to write when empty.
    fn write(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.queue.clear();
        self.added = 0;
        self.problem = None;
        self.text.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.focus(window, cx);
        });
        self.step = Step::Confirm(Read {
            file: None,
            warnings: Vec::new(),
            saving: false,
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
        let file = read.file.as_ref().map(|file| {
            (
                file.file_name.clone(),
                file.bytes.clone(),
                file.format.media_type(),
            )
        });
        let file_name = file.as_ref().map(|(name, _, _)| name.clone());
        let saved = self.database.read(cx).write(cx, move |write| {
            let file = file.as_ref().map(|(name, bytes, media_type)| DocumentFile {
                name,
                bytes,
                media_type,
            });
            commands::import_document(write, file, &text)
        });
        self.task = cx.spawn_in(window, async move |this, cx| {
            let saved = saved.await;
            let updated = this.update_in(cx, |this, window, cx| match saved {
                Ok(_) => {
                    // The kit's notifications live in its Root, which a
                    // test's window may not have.
                    if window.root::<Root>().flatten().is_some() {
                        window.push_notification(
                            Notification::new().message(match &file_name {
                                Some(file_name) => format!("Saved {file_name}"),
                                None => "Saved your text".to_owned(),
                            }),
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
                    this.problem = Some(match &file_name {
                        Some(file_name) => {
                            format!("{file_name}: couldn't save it. Try again.").into()
                        }
                        None => "Couldn't save it. Try again.".into(),
                    });
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
        // The whole page takes a drop, so a file needn't land on the card.
        v_flex()
            .id("drop-target")
            .size_full()
            .gap_3()
            .rounded(theme.radius_lg)
            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                style.bg(cx.theme().primary.opacity(0.04))
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.import(paths.paths().to_vec(), window, cx)
            }))
            .child(title("Import your CV".into()))
            .child(caption(
                "It's read on this device. You'll see the text before anything is saved.",
                cx,
            ))
            .children(self.render_problem(cx))
            .child(
                h_flex()
                    .id("drop-card")
                    .gap_3()
                    .p_4()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_dashed()
                    .border_color(theme.border)
                    .drag_over::<ExternalPaths>(|style, _, _, cx| {
                        style.border_color(cx.theme().primary)
                    })
                    .child(
                        Icon::new(IconName::Inbox)
                            .size(px(20.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_0p5()
                            .child(div().child("Drop your CV here"))
                            .child(caption("PDF, Markdown or plain text.", cx)),
                    )
                    .child(
                        Button::new("browse")
                            .label("Choose files…")
                            .primary()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .p_4()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        Icon::new(IconName::Copy)
                            .size(px(20.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_0p5()
                            .child(div().child("Or paste it, or write it"))
                            .child(caption(
                                "Text from a website, a document or your LinkedIn page.",
                                cx,
                            )),
                    )
                    .child(
                        Button::new("write")
                            .label("Write")
                            .ghost()
                            .small()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.start_writing(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("paste")
                            .label("Paste")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.paste(window, cx))),
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
            Some(
                Button::new("skip")
                    .label("Skip")
                    .on_click(cx.listener(|this, _, window, cx| this.skip(window, cx))),
            )
        } else {
            read.file.as_ref().map(|_| {
                Button::new("choose-another")
                    .label("Choose another file")
                    .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx)))
            })
        };
        let (heading, description) = match &read.file {
            Some(file) => (
                "Check the text",
                format!(
                    "The text read from {}. Correct anything that's wrong: facts are taken from \
                     this text. Nothing has been sent anywhere.",
                    file.file_name
                ),
            ),
            None => (
                "Your text",
                "Paste or write your CV, or anything else about your work: facts are taken \
                 from this text. Nothing has been sent anywhere."
                    .to_owned(),
            ),
        };
        v_flex()
            .size_full()
            .gap_3()
            .children(self.position().map(|position| caption(position, cx)))
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(title(heading.into()))
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
                            .children(
                                other.map(|other| other.ghost().small().disabled(read.saving)),
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
            .child(caption(description, cx))
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
    format: Format,
    bytes: Arc<Vec<u8>>,
    /// The last import of the same file, if it was imported before.
    imported: Option<Imported>,
}

struct Imported {
    file_name: String,
    added_at: DateTime<Utc>,
}

/// Text waiting for the user to check and save it.
struct Read {
    /// The file it was read from; none when pasted or written.
    file: Option<File>,
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
        import::Error::Invalid => "it's damaged, or isn't the kind of file its name says.",
        import::Error::NoText => {
            "this PDF has no text in it, only pictures of text, like a scan. Export your CV \
             again from where you wrote it, as a PDF."
        }
        import::Error::Empty => "there's no text in it.",
        import::Error::Encoding => {
            "its text isn't in a known encoding. Save it as UTF-8 and import it again."
        }
        import::Error::Unsupported => {
            "this kind of file can't be imported yet. Import a PDF, Markdown or text file."
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
    use gpui_kit::{ClipboardItem, TestAppContext, VisualTestContext};

    use super::*;
    use crate::database::TemporaryDirectory;
    use crate::workspace::testing;

    const LATEX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/import/pdf/latex.pdf");
    const SIDEBAR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/import/pdf/sidebar.pdf");
    const MARKDOWN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/import/markdown/cv.md");
    const UNSUPPORTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");

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
        /// Text pasted or written, with no file.
        Typed,
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
                Step::Confirm(read) => match &read.file {
                    Some(file) => Shown::Confirm(file.file_name.to_string()),
                    None => Shown::Typed,
                },
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

        /// The saved texts of documents with no file, pasted or written, as
        /// the sample's notes are, oldest first.
        fn saved_without_file(&mut self) -> Vec<String> {
            let saved = self.database.read_with(self.cx, |database, _| {
                database
                    .connection()
                    .prepare(
                        "SELECT document_texts.text
                         FROM documents
                         JOIN document_texts ON document_texts.document_id = documents.id
                         WHERE documents.file_hash IS NULL
                         ORDER BY documents.id",
                    )
                    .and_then(|mut statement| {
                        statement
                            .query_map([], |row| row.get(0))?
                            .collect::<Result<Vec<_>, _>>()
                    })
            });
            match saved {
                Ok(saved) => saved,
                Err(error) => panic!("couldn't read the documents: {error}"),
            }
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
                Format::Pdf.media_type().to_owned()
            )]
        );
        assert_eq!(open.shown(), Shown::Choose, "it's ready for the next file");
    }

    #[test]
    fn markdown_is_saved_as_the_text_it_shows() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.import(&[MARKDOWN]);
        assert_eq!(open.shown(), Shown::Confirm("cv.md".into()));
        assert!(
            open.text()
                .starts_with("Alex Rahman\n\nalex.rahman@example.com · GitHub\n")
        );
        open.save();
        let saved = open.saved("cv.md");
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].1, "text/markdown");
    }

    #[test]
    fn pasted_text_is_checked_and_saved_with_no_file() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.cx
            .write_to_clipboard(ClipboardItem::new_string("Alex Rahman\nRust".into()));
        open.import(&[LATEX]);
        open.act(|page, window, cx| page.paste(window, cx));
        assert_eq!(
            open.shown(),
            Shown::Confirm("latex.pdf".into()),
            "pasting into the text of a file isn't replacing it"
        );
        open.act(|page, window, cx| page.skip_rest(window, cx));
        open.act(|page, window, cx| page.focus_handle.focus(window, cx));
        open.cx.simulate_keystrokes("ctrl-v");
        assert_eq!(open.shown(), Shown::Typed, "Ctrl+V on the page pastes");
        assert_eq!(open.text(), "Alex Rahman\nRust");
        let before = open.saved_without_file().len();
        open.save();
        assert_eq!(open.shown(), Shown::Choose);
        let saved = open.saved_without_file();
        assert_eq!(saved.len(), before + 1);
        assert_eq!(saved.last().map(String::as_str), Some("Alex Rahman\nRust"));
    }

    #[test]
    fn written_text_starts_empty() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.import(&[LATEX]);
        open.act(|page, window, cx| page.focus_handle.focus(window, cx));
        open.cx.simulate_keystrokes("ctrl-n");
        assert_eq!(
            open.shown(),
            Shown::Confirm("latex.pdf".into()),
            "a file's text isn't left for an empty page"
        );
        open.act(|page, window, cx| page.skip_rest(window, cx));
        open.act(|page, window, cx| page.focus_handle.focus(window, cx));
        open.cx.simulate_keystrokes("ctrl-n");
        assert_eq!(open.shown(), Shown::Typed, "Ctrl+N on the page opens it");
        open.cx.simulate_keystrokes("x escape");
        assert_eq!(
            open.shown(),
            Shown::Choose,
            "Esc in the text field leaves it"
        );
        open.cx.simulate_keystrokes("ctrl-n");
        assert_eq!(open.text(), "");
        open.set_text("Organized Code Fest 2019.");
        let before = open.saved_without_file().len();
        open.cx.simulate_keystrokes("ctrl-enter");
        testing::settle(open.cx, &open.database);
        let saved = open.saved_without_file();
        assert_eq!(saved.len(), before + 1);
        assert_eq!(
            saved.last().map(String::as_str),
            Some("Organized Code Fest 2019.")
        );
        open.cx.simulate_keystrokes("ctrl-n");
        assert_eq!(
            open.shown(),
            Shown::Typed,
            "keys still work once the text field is gone"
        );
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
        open.import(&[UNSUPPORTED]);
        assert_eq!(open.shown(), Shown::Choose);
        let problem = open.problem();
        assert!(
            problem.starts_with("Cargo.toml: this kind of file can't be imported"),
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
        open.import(&[LATEX, UNSUPPORTED, SIDEBAR, LATEX]);
        assert_eq!(open.shown(), Shown::Ask("latex.pdf".into(), false));
        open.act(|page, window, cx| page.skip(window, cx));
        assert_eq!(
            open.shown(),
            Shown::Ask("sidebar.pdf".into(), false),
            "a kind of file that can't be read isn't asked about"
        );
        let problem = open.problem();
        assert!(
            problem.starts_with("Cargo.toml: this kind of file can't be imported"),
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
