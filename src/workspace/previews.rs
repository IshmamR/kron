use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, Pixels, RenderImage, SharedString, Subscription, Task,
    Window,
};
use image::{Frame, RgbaImage};
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use crate::database::schema::{Blocks, Generations, Lines, Sections};
use crate::database::{Changed, Database, Error};
use crate::model::{ResumeId, Section};
use crate::typeset::{self, Paper, ResumeLayout, Typesetter, resume_markup};

/// Width of a US Letter page, the template's paper, in typographic points.
const PAGE_WIDTH_POINTS: f32 = 612.;
/// The pixels kept of renders no view holds: a few job pages, about 2 MB
/// each, so going back to one just seen shows it without a blank frame.
/// Rendering one again takes 3 to 10 ms in release, so keeping more buys
/// little.
const MEMORY_BUDGET: usize = 8 * 1024 * 1024;

/// Typst renders of each job's résumés, made off the main thread on
/// request. A render is kept until the résumé's markup changes, or until it
/// goes over `MEMORY_BUDGET`; either way its images leave the GPU too.
pub struct Previews {
    database: Entity<Database>,
    typesetter: Option<Arc<Typesetter>>,
    slots: HashMap<SlotKey, Slot>,
    /// Counts `get` calls, to tell which slot was shown least recently.
    shown: u64,
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
    /// Built here rather than decoded by GPUI, which would keep every image
    /// in its asset cache for good.
    pub images: Vec<Arc<RenderImage>>,
    pub layout: ResumeLayout,
    /// The images' pixels, in bytes.
    bytes: usize,
}

impl Previews {
    /// The typesetter, once the fonts have loaded.
    pub fn typesetter(&self) -> Option<Arc<Typesetter>> {
        self.typesetter.clone()
    }

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
            shown: 0,
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

        self.shown += 1;
        let slot = self.slots.entry(key).or_default();
        slot.last_shown = self.shown;
        let requested = slot.requested;
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
                Ok(typeset) => {
                    Render::Ready(Arc::new(RenderedPages::new(typeset.pages, typeset.layout)))
                }
                Err(error) => {
                    eprintln!("couldn't render résumé {:?}: {error}", key.resume);
                    Render::Failed(error.into())
                }
            };
            // An error only means the previews were dropped meanwhile.
            this.update(cx, |this, cx| {
                // A render of content that has since changed, or of a slot
                // evicted meanwhile, is dropped: it was never drawn.
                let Some(slot) = this
                    .slots
                    .get_mut(&key)
                    .filter(|slot| slot.requested == Some(markup_hash))
                else {
                    return;
                };
                if let Some(replaced) = slot.latest.replace(render) {
                    drop_images(&replaced, cx);
                }
                this.evict(key, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Drops the renders shown least recently until those no view holds fit
    /// `MEMORY_BUDGET`, keeping `newest`.
    fn evict(&mut self, newest: SlotKey, cx: &mut App) {
        let mut used: usize = self
            .slots
            .values()
            .filter(|slot| !slot.held())
            .map(Slot::bytes)
            .sum();
        while used > MEMORY_BUDGET {
            let oldest = self
                .slots
                .iter()
                .filter(|(key, slot)| **key != newest && slot.bytes() > 0 && !slot.held())
                .min_by_key(|(_, slot)| slot.last_shown)
                .map(|(key, _)| *key);
            let Some(slot) = oldest.and_then(|key| self.slots.remove(&key)) else {
                break;
            };
            used -= slot.bytes();
            if let Some(render) = &slot.latest {
                drop_images(render, cx);
            }
        }
    }
}

impl RenderedPages {
    fn new(pages: Vec<typeset::Pixels>, layout: ResumeLayout) -> Self {
        let bytes = pages.iter().map(|page| page.rgba.len()).sum();
        let images = pages
            .into_iter()
            .filter_map(|page| {
                let mut bgra = page.rgba;
                // GPUI draws BGRA.
                for pixel in bgra.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                let buffer = RgbaImage::from_raw(page.width, page.height, bgra)?;
                Some(Arc::new(RenderImage::new([Frame::new(buffer)])))
            })
            .collect();
        Self {
            images,
            layout,
            bytes,
        }
    }
}

/// Frees the render's images from every window's GPU atlas. A view that
/// draws one again uploads it again.
fn drop_images(render: &Render, cx: &mut App) {
    if let Render::Ready(pages) = render {
        for image in &pages.images {
            cx.drop_image(image.clone(), None);
        }
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
    /// `Previews::shown` when last asked for.
    last_shown: u64,
}

impl Slot {
    fn bytes(&self) -> usize {
        match &self.latest {
            Some(Render::Ready(pages)) => pages.bytes,
            _ => 0,
        }
    }

    /// A view keeps the pages it shows, like the job view its résumé's.
    fn held(&self) -> bool {
        matches!(&self.latest, Some(Render::Ready(pages)) if Arc::strong_count(pages) > 1)
    }
}

/// The résumé laid out with its template version, on its paper; `None` when
/// there's no such résumé.
pub(super) fn read_markup(
    connection: &Connection,
    resume: ResumeId,
) -> Result<Option<String>, Error> {
    let generation: Option<(Option<Uuid>, Option<String>)> = connection
        .prepare_cached(
            "SELECT template_version_id, paper FROM generations
             WHERE id = ?1 AND deleted_at IS NULL",
        )?
        .query_row([resume.0], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((version, paper)) = generation else {
        return Ok(None);
    };
    let sections = Section::read_all(connection, resume)?;
    Ok(Some(resume_markup(
        typeset::template_of_version(version),
        &sections,
        Paper::from_stored(paper.as_deref()),
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
    use crate::database::{TemporaryDirectory, Write};
    use crate::model::{JobId, commands};
    use crate::workspace::testing;

    fn current_resumes(connection: &Connection) -> rusqlite::Result<Vec<(JobId, ResumeId)>> {
        connection
            .prepare("SELECT job_id, generation_id FROM current_generations WHERE kind = 'resume'")?
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
        let resumes =
            match database.read_with(&app, |database, _| current_resumes(database.connection())) {
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
        let read = |app: &mut TestAppContext| {
            previews.read_with(app, |previews, _| previews.markup_hashes.len())
        };
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

    /// Every current résumé rendered at `pixels_per_point`, waiting for the
    /// fonts and the renders.
    fn render_all(
        app: &mut TestAppContext,
        previews: &Entity<Previews>,
        resumes: &[ResumeId],
        pixels_per_point: f32,
    ) -> Vec<Option<Render>> {
        // The first pass starts nothing until the fonts load.
        for _ in 0..2 {
            for &resume in resumes {
                previews.update(app, |previews, cx| {
                    previews.get(resume, pixels_per_point, cx)
                });
                app.run_until_parked();
            }
        }
        resumes
            .iter()
            .map(|&resume| {
                previews.update(app, |previews, cx| {
                    previews.get(resume, pixels_per_point, cx)
                })
            })
            .collect()
    }

    struct Opened {
        previews: Entity<Previews>,
        database: Entity<Database>,
        resumes: Vec<(JobId, ResumeId)>,
        _directory: TemporaryDirectory,
    }

    fn open(app: &mut TestAppContext) -> Opened {
        let (database, directory) = testing::open_sample(app);
        let previews = app.new(|cx| Previews::new(database.clone(), cx));
        let resumes =
            match database.read_with(app, |database, _| current_resumes(database.connection())) {
                Ok(resumes) => resumes,
                Err(error) => panic!("{error}"),
            };
        Opened {
            previews,
            database,
            resumes,
            _directory: directory,
        }
    }

    /// The bytes of the renders kept that nobody else holds.
    fn unheld_bytes(app: &TestAppContext, previews: &Entity<Previews>) -> usize {
        previews.read_with(app, |previews, _| {
            previews
                .slots
                .values()
                .filter(|slot| !slot.held())
                .map(Slot::bytes)
                .sum()
        })
    }

    /// Fixed before: every render was kept, so memory only grew.
    #[test]
    fn renders_nobody_holds_are_kept_only_up_to_the_budget() {
        let mut app = TestAppContext::single();
        let Opened {
            previews,
            resumes,
            _directory,
            ..
        } = open(&mut app);
        let resumes: Vec<ResumeId> = resumes.into_iter().map(|(_, resume)| resume).collect();
        // Job view size: the sample's résumés come to about twice the budget.
        let renders = render_all(&mut app, &previews, &resumes, 1.);
        let last = renders.last().cloned().flatten();
        drop(renders);
        assert!(matches!(last, Some(Render::Ready(_))));
        drop(last);
        let largest = previews.read_with(&app, |previews, _| {
            previews
                .slots
                .values()
                .map(Slot::bytes)
                .max()
                .unwrap_or_default()
        });
        assert!(unheld_bytes(&app, &previews) <= MEMORY_BUDGET + largest);
        let kept = previews.read_with(&app, |previews, _| {
            previews
                .slots
                .values()
                .filter(|slot| slot.bytes() > 0)
                .count()
        });
        assert!(kept < resumes.len(), "some were evicted");
    }

    /// As when the window moves to a screen of another scale: the renders
    /// at the old one go once nothing shows them.
    #[test]
    fn renders_at_a_scale_no_longer_shown_go() {
        let mut app = TestAppContext::single();
        let Opened {
            previews,
            resumes,
            _directory,
            ..
        } = open(&mut app);
        let resumes: Vec<ResumeId> = resumes.into_iter().map(|(_, resume)| resume).collect();
        drop(render_all(&mut app, &previews, &resumes, 1.));
        drop(render_all(&mut app, &previews, &resumes, 1.5));
        let at_old_scale = previews.read_with(&app, |previews, _| {
            previews
                .slots
                .iter()
                .filter(|(key, _)| key.pixels_per_point == 1f32.to_bits())
                .map(|(_, slot)| slot.bytes())
                .sum::<usize>()
        });
        assert_eq!(
            at_old_scale, 0,
            "the newer, larger renders fill the budget first"
        );
    }

    #[test]
    fn a_render_a_view_holds_is_never_evicted() {
        let mut app = TestAppContext::single();
        let Opened {
            previews,
            resumes,
            _directory,
            ..
        } = open(&mut app);
        let resumes: Vec<ResumeId> = resumes.into_iter().map(|(_, resume)| resume).collect();
        let Some(Some(Render::Ready(held))) =
            render_all(&mut app, &previews, &resumes[..1], 1.).pop()
        else {
            panic!("no render");
        };
        render_all(&mut app, &previews, &resumes[1..], 1.);
        let kept = previews.update(&mut app, |previews, cx| previews.get(resumes[0], 1., cx));
        assert!(matches!(kept, Some(Render::Ready(pages)) if Arc::ptr_eq(&pages, &held)));
    }

    #[test]
    fn a_render_of_changed_markup_replaces_the_old_one() {
        let mut app = TestAppContext::single();
        let Opened {
            previews,
            database,
            resumes,
            _directory,
        } = open(&mut app);
        let (job, resume) = resumes[0];
        let Some(Some(Render::Ready(before))) =
            render_all(&mut app, &previews, &[resume], 0.25).pop()
        else {
            panic!("no render");
        };
        run(&mut app, &database, move |write| {
            commands::set_template(write, job, "sidebar")
        });
        let Some(Some(Render::Ready(after))) =
            render_all(&mut app, &previews, &[resume], 0.25).pop()
        else {
            panic!("no render");
        };
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(
            Arc::strong_count(&before),
            1,
            "only this test still has the old one"
        );
    }
}
