use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, Image, ImageFormat, Pixels, SharedString, Subscription,
    Task, Window,
};
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use crate::database::schema::{Blocks, Generations, Lines, Sections};
use crate::database::{Changed, Database, Error};
use crate::model::{ResumeId, Section};
use crate::typeset::{self, ResumeLayout, Typesetter, resume_markup};

/// Width of a US Letter page, the template's paper, in typographic points.
const PAGE_WIDTH_POINTS: f32 = 612.;

/// Typst renders of each job's résumés, made off the main thread on
/// request and kept until the résumé's markup changes.
pub struct Previews {
    database: Entity<Database>,
    typesetter: Option<Arc<Typesetter>>,
    slots: HashMap<SlotKey, Slot>,
    /// Each résumé's markup's hash as last read, so a résumé is read once,
    /// and again only after the database changes.
    markup_hashes: HashMap<ResumeId, u64>,
    _loading_fonts: Task<()>,
    _changes: Subscription,
}

#[derive(Clone)]
pub enum Render {
    Ready(Arc<RenderedPages>),
    Failed(SharedString),
}

pub struct RenderedPages {
    pub images: Vec<Arc<Image>>,
    pub layout: ResumeLayout,
}

impl Previews {
    pub fn new(database: Entity<Database>, cx: &mut Context<Self>) -> Self {
        let loading_fonts = cx.spawn(async move |this, cx| {
            let typesetter = cx.background_spawn(async { Typesetter::new() }).await;
            // An error only means the previews were dropped meanwhile.
            this.update(cx, |this, cx| {
                this.typesetter = Some(Arc::new(typesetter));
                // Views asked while the fonts loaded; let them ask again.
                cx.notify();
            })
            .ok();
        });

        Self {
            _changes: cx.subscribe(&database, |this, _, changed: &Changed, cx| {
                // A résumé's sections, blocks and lines change only as it's
                // generated, or when everything is erased.
                if changed.touches(Sections) || changed.touches(Blocks) || changed.touches(Lines) {
                    this.markup_hashes.clear();
                    cx.notify();
                } else if changed.touches(Generations) {
                    this.markup_hashes
                        .retain(|resume, _| !changed.touches_row(Generations, resume.0));
                    cx.notify();
                }
            }),
            database,
            typesetter: None,
            slots: HashMap::new(),
            markup_hashes: HashMap::new(),
            _loading_fonts: loading_fonts,
        }
    }

    /// The résumé at `pixels_per_point`, starting a render when there's none
    /// for its current content. While a newer one renders, the previous
    /// render is returned, so edits don't flash. `None` until the first
    /// render is done, or when there's no such résumé.
    pub fn get(
        &mut self,
        resume: ResumeId,
        pixels_per_point: f32,
        cx: &mut Context<Self>,
    ) -> Option<Render> {
        let key = SlotKey {
            resume,
            pixels_per_point: pixels_per_point.to_bits(),
        };
        let mut markup = None;
        let markup_hash = match self.markup_hashes.get(&resume) {
            Some(&markup_hash) => markup_hash,
            None => {
                let read = self.markup(resume, cx)?;
                let markup_hash = hash(&read);
                self.markup_hashes.insert(resume, markup_hash);
                markup = Some(read);
                markup_hash
            }
        };

        let requested = self.slots.get(&key).and_then(|slot| slot.requested);
        if requested != Some(markup_hash)
            && let Some(typesetter) = self.typesetter.clone()
        {
            let markup = match markup {
                Some(markup) => markup,
                None => self.markup(resume, cx)?,
            };
            self.slots.entry(key).or_default().requested = Some(markup_hash);
            self.render(key, markup, markup_hash, typesetter, cx);
        }
        self.slots.get(&key).and_then(|slot| slot.latest.clone())
    }

    /// The résumé's markup, read from the database; `None`, with a log, when
    /// it can't be read.
    fn markup(&self, resume: ResumeId, cx: &App) -> Option<String> {
        match read_markup(self.database.read(cx).connection(), resume) {
            Ok(markup) => markup,
            Err(error) => {
                eprintln!("previews: couldn't read résumé {:?}: {error}", resume.0);
                None
            }
        }
    }

    fn render(
        &mut self,
        key: SlotKey,
        markup: String,
        markup_hash: u64,
        typesetter: Arc<Typesetter>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let typeset = cx
                .background_spawn(async move {
                    typesetter.render(&markup, f32::from_bits(key.pixels_per_point))
                })
                .await;
            let render = match typeset {
                Ok(typeset) => Render::Ready(Arc::new(RenderedPages {
                    images: typeset
                        .pages
                        .into_iter()
                        .map(|png| Arc::new(Image::from_bytes(ImageFormat::Png, png)))
                        .collect(),
                    layout: typeset.layout,
                })),
                Err(error) => {
                    eprintln!("couldn't render résumé {:?}: {error}", key.resume);
                    Render::Failed(error.into())
                }
            };
            // An error only means the previews were dropped meanwhile.
            this.update(cx, |this, cx| {
                let Some(slot) = this.slots.get_mut(&key) else {
                    return;
                };
                // A render of content that has since changed is dropped; the
                // newer one is on its way.
                if slot.requested == Some(markup_hash) {
                    slot.latest = Some(render);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

/// The resolution to render a page at so that, drawn `width` wide, it's sharp
/// without being scaled.
pub fn pixels_per_point(width: Pixels, window: &Window) -> f32 {
    f32::from(width) * window.scale_factor() / PAGE_WIDTH_POINTS
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SlotKey {
    resume: ResumeId,
    /// `f32` isn't `Hash`; its bits are.
    pixels_per_point: u32,
}

#[derive(Default)]
struct Slot {
    /// Hash of the markup last sent to be rendered.
    requested: Option<u64>,
    latest: Option<Render>,
}

/// The résumé laid out with its template version; `None` when there's no
/// such résumé.
fn read_markup(connection: &Connection, resume: ResumeId) -> Result<Option<String>, Error> {
    let version: Option<Option<Uuid>> = connection
        .prepare_cached(
            "SELECT template_version_id FROM generations WHERE id = ?1 AND deleted_at IS NULL",
        )?
        .query_row([resume.0], |row| row.get(0))
        .optional()?;
    let Some(version) = version else {
        return Ok(None);
    };
    let sections = Section::read_all(connection, resume)?;
    Ok(Some(resume_markup(
        typeset::template_of_version(version),
        &sections,
    )))
}

fn hash(markup: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    markup.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;

    use super::*;
    use crate::database::Write;
    use crate::model::{JobId, commands};
    use crate::workspace::testing;

    fn current_resumes(connection: &Connection) -> rusqlite::Result<Vec<(JobId, ResumeId)>> {
        connection
            .prepare(
                "SELECT job_id, generation_id FROM current_generations WHERE kind = 'resume'",
            )?
            .query_map([], |row| Ok((JobId(row.get(0)?), ResumeId(row.get(1)?))))?
            .collect()
    }

    fn run(
        app: &mut TestAppContext,
        database: &Entity<Database>,
        command: impl FnOnce(&mut Write) -> Result<(), Error> + Send + 'static,
    ) {
        database.read_with(app, |database, _| {
            database.queue(command);
            database.flush();
        });
        app.run_until_parked();
    }

    #[test]
    fn a_change_to_one_resume_reads_only_that_one_again() {
        let mut app = TestAppContext::single();
        let (database, _directory) = testing::open_sample(&mut app);
        let previews = app.new(|cx| Previews::new(database.clone(), cx));
        let resumes = match database.read_with(&app, |database, _| {
            current_resumes(database.connection())
        }) {
            Ok(resumes) => resumes,
            Err(error) => panic!("{error}"),
        };
        let get_all = |app: &mut TestAppContext| {
            for &(_, resume) in &resumes {
                previews.update(app, |previews, cx| previews.get(resume, 0.25, cx));
            }
            app.run_until_parked();
        };
        // Once the fonts load, renders start.
        get_all(&mut app);
        get_all(&mut app);
        for &(_, resume) in &resumes {
            let render = previews.update(&mut app, |previews, cx| previews.get(resume, 0.25, cx));
            assert!(matches!(render, Some(Render::Ready(_))));
        }
        let read = |app: &mut TestAppContext| previews.read_with(app, |previews, _| previews.markup_hashes.len());
        assert_eq!(read(&mut app), resumes.len());

        let (job, switched) = resumes[0];
        run(&mut app, &database, move |write| {
            commands::set_template(write, job, "sidebar")
        });
        assert_eq!(read(&mut app), resumes.len() - 1);
        let forgotten = previews.read_with(&app, |previews, _| {
            !previews.markup_hashes.contains_key(&switched)
        });
        assert!(forgotten, "the switched one is read again");

        run(&mut app, &database, |write| write.erase_everything());
        assert_eq!(read(&mut app), 0);
    }
}
