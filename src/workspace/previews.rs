use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, Image, ImageFormat, Pixels, SharedString, Subscription, Task,
    Window,
};

use crate::model::{Job, JobId, Library, ResumeId};
use crate::typeset::{ResumeLayout, Typesetter, resume_markup};

/// Width of a US Letter page, the template's paper, in typographic points.
const PAGE_WIDTH_POINTS: f32 = 612.;

/// Typst renders of each job's résumés, made off the main thread on
/// request and kept until the résumé's markup changes.
pub struct Previews {
    library: Entity<Library>,
    typesetter: Option<Arc<Typesetter>>,
    slots: HashMap<SlotKey, Slot>,
    _loading_fonts: Task<()>,
    _library_changed: Subscription,
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
    pub fn new(library: Entity<Library>, cx: &mut Context<Self>) -> Self {
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
            _library_changed: cx.observe(&library, |_, _, cx| cx.notify()),
            library,
            typesetter: None,
            slots: HashMap::new(),
            _loading_fonts: loading_fonts,
        }
    }

    /// The job's current résumé at `pixels_per_point`, starting a render when
    /// there's none for its current content. While a newer one renders, the
    /// previous render is returned, so edits don't flash. `None` until the
    /// first render is done, or when the job has no résumé.
    pub fn get(
        &mut self,
        job: JobId,
        pixels_per_point: f32,
        cx: &mut Context<Self>,
    ) -> Option<Render> {
        let resume = self.library.read(cx).job(job).and_then(Job::current_resume)?.id;
        self.get_version(job, resume, pixels_per_point, cx)
    }

    /// Like `get`, for any version of the job's résumé.
    pub fn get_version(
        &mut self,
        job: JobId,
        resume: ResumeId,
        pixels_per_point: f32,
        cx: &mut Context<Self>,
    ) -> Option<Render> {
        let library = self.library.read(cx);
        let resume = library.job(job).and_then(|job| job.resume(resume))?;
        let markup = resume_markup(resume, library);
        let key = SlotKey {
            resume: resume.id,
            pixels_per_point: pixels_per_point.to_bits(),
        };
        let markup_hash = hash(&markup);

        let slot = self.slots.entry(key).or_default();
        if slot.requested != Some(markup_hash)
            && let Some(typesetter) = self.typesetter.clone()
        {
            slot.requested = Some(markup_hash);
            self.render(key, markup, markup_hash, typesetter, cx);
        }
        self.slots.get(&key).and_then(|slot| slot.latest.clone())
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

fn hash(markup: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    markup.hash(&mut hasher);
    hasher.finish()
}
