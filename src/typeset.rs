// Typst is the one layout engine: the same pages become the in-app preview and,
// later, the exported PDF, so the preview can't drift from what gets sent.

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration, Label};
use typst::introspection::{Location, Tag};
use typst::layout::{Frame, FrameItem, Point, Transform};
use typst::syntax::{FileId, Source};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library as TypstLibrary, LibraryExt, World};
use typst_kit::fonts::FontStore;
use typst_layout::PagedDocument;
use typst_render::RenderOptions;
use uuid::Uuid;

use crate::model::{FactKind, Library, LineId, TailoredResume};

const RESUME_TEMPLATE: &str = include_str!("../assets/typst/resume.typ");
const LINE_LABEL_PREFIX: &str = "line-";
/// Clicks this close above or below a line still hit it, so the gaps
/// between lines aren't dead space.
const HIT_SLOP: f32 = 2.;

/// Compiles Typst markup and renders its pages. Loading the system fonts takes
/// a moment, so build it once, off the main thread, and share it.
pub struct Typesetter {
    library: LazyHash<TypstLibrary>,
    fonts: FontStore,
}

impl Typesetter {
    pub fn new() -> Self {
        let mut fonts = FontStore::new();
        fonts.extend(typst_kit::fonts::system());
        // Typst's own fonts, so a page still sets when none of the template's
        // fonts are installed.
        fonts.extend(typst_kit::fonts::embedded());
        Self {
            library: LazyHash::new(TypstLibrary::default()),
            fonts,
        }
    }

    /// Lays out `markup` (from `resume_markup`) and renders each page as a PNG,
    /// `pixels_per_point` pixels per typographic point (a US Letter page is
    /// 612 points wide).
    pub fn render(&self, markup: &str, pixels_per_point: f32) -> Result<TypesetResume, String> {
        let document = self.compile(markup)?;
        let options = RenderOptions {
            pixel_per_pt: f64::from(pixels_per_point).into(),
            ..RenderOptions::default()
        };
        let pages = document
            .pages()
            .iter()
            .map(|page| {
                typst_render::render(page, &options)
                    .encode_png()
                    .map_err(|error| format!("couldn't encode the page: {error}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(TypesetResume {
            pages,
            layout: ResumeLayout::of(&document),
        })
    }

    fn compile(&self, markup: &str) -> Result<PagedDocument, String> {
        let world = SingleFileWorld {
            typesetter: self,
            source: Source::detached(markup),
        };
        typst::compile::<PagedDocument>(&world)
            .output
            .map_err(|errors| {
                errors
                    .iter()
                    .map(|error| error.message.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            })
    }
}

pub struct TypesetResume {
    /// One PNG per page.
    pub pages: Vec<Vec<u8>>,
    pub layout: ResumeLayout,
}

/// Where each line landed, in typographic points from each page's top left.
#[derive(Clone, Debug, Default)]
pub struct ResumeLayout {
    pub pages: Vec<PageSize>,
    /// One box per line per page it's on; a line split by a page break has
    /// two.
    pub boxes: Vec<LineBox>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineBox {
    pub line: LineId,
    pub page: usize,
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl ResumeLayout {
    fn of(document: &PagedDocument) -> Self {
        let mut collector = BoxCollector::default();
        for (page, contents) in document.pages().iter().enumerate() {
            collector.collect(&contents.frame, Transform::identity(), page);
        }
        let mut boxes = collector.boxes;
        let pages: Vec<PageSize> = document
            .pages()
            .iter()
            .map(|page| PageSize {
                width: page.frame.width().to_pt() as f32,
                height: page.frame.height().to_pt() as f32,
            })
            .collect();
        // Span each page's full text column (the template's margins are
        // symmetric), so a click beside a short line still selects it and
        // highlights line up.
        for (page, size) in pages.iter().enumerate() {
            let left = boxes
                .iter()
                .filter(|line_box| line_box.page == page)
                .map(|line_box| line_box.left)
                .fold(f32::MAX, f32::min);
            let right = size.width - left;
            for line_box in boxes.iter_mut().filter(|line_box| line_box.page == page) {
                line_box.left = left;
                line_box.right = line_box.right.max(right);
            }
        }

        Self { pages, boxes }
    }

    pub fn line_at(&self, page: usize, x: f32, y: f32) -> Option<LineId> {
        self.boxes
            .iter()
            .find(|line_box| {
                line_box.page == page
                    && (line_box.left..=line_box.right).contains(&x)
                    && (line_box.top - HIT_SLOP..=line_box.bottom + HIT_SLOP).contains(&y)
            })
            .map(|line_box| line_box.line)
    }

    pub fn boxes_of(&self, line: LineId) -> impl Iterator<Item = &LineBox> {
        self.boxes
            .iter()
            .filter(move |line_box| line_box.line == line)
    }
}

/// The template followed by one labelled call per line.
pub fn resume_markup(resume: &TailoredResume, library: &Library) -> String {
    let mut markup = String::from(RESUME_TEMPLATE);
    for section in &resume.sections {
        if !section.title.is_empty() {
            markup.push_str(&call("section", &section.title));
            markup.push('\n');
        }
        for line in &section.lines {
            let function = match library.line_kind(line) {
                Some(FactKind::Name) => "name",
                Some(FactKind::Contact) => "contact",
                Some(FactKind::Title) => "role",
                Some(FactKind::Bullet) => "bullet",
                _ => "plain",
            };
            markup.push_str(&call(function, &line.text));
            markup.push_str(&format!(" <{LINE_LABEL_PREFIX}{}>\n", line.id.0));
        }
    }
    markup
}

/// `#function("text")` on its own line. Lines go in as string literals rather
/// than markup, so `*`, `#` or `$` in them stay plain text.
fn call(function: &str, text: &str) -> String {
    let mut call = format!("\n#{function}(\"");
    for character in text.chars() {
        match character {
            '\\' => call.push_str("\\\\"),
            '"' => call.push_str("\\\""),
            '\n' => call.push_str("\\n"),
            character => call.push(character),
        }
    }
    call.push_str("\")");
    call
}

/// Walks the laid-out frames in document order. Between a line's start and
/// end tags, every run of text belongs to that line.
#[derive(Default)]
struct BoxCollector {
    open: Vec<(Location, LineId)>,
    boxes: Vec<LineBox>,
}

impl BoxCollector {
    fn collect(&mut self, frame: &Frame, transform: Transform, page: usize) {
        for (position, item) in frame.items() {
            match item {
                FrameItem::Group(group) => {
                    let transform = transform
                        .pre_concat(Transform::translate(position.x, position.y))
                        .pre_concat(group.transform);
                    self.collect(&group.frame, transform, page);
                }
                FrameItem::Tag(Tag::Start(content, _)) => {
                    let line = content.label().and_then(line_id);
                    if let Some((location, line)) = content.location().zip(line) {
                        self.open.push((location, line));
                    }
                }
                FrameItem::Tag(Tag::End(location, ..)) => {
                    self.open.retain(|(open, _)| open != location);
                }
                FrameItem::Text(text) => {
                    let Some(&(_, line)) = self.open.last() else {
                        continue;
                    };
                    let metrics = text.font.metrics();
                    // `position` is on the baseline; the box spans the font's
                    // ascender and descender so every line is evenly tall.
                    let top_left =
                        Point::new(position.x, position.y - metrics.ascender.at(text.size))
                            .transform(transform);
                    let bottom_right = Point::new(
                        position.x + text.width(),
                        position.y - metrics.descender.at(text.size),
                    )
                    .transform(transform);
                    self.extend(LineBox {
                        line,
                        page,
                        left: top_left.x.to_pt() as f32,
                        top: top_left.y.to_pt() as f32,
                        right: bottom_right.x.to_pt() as f32,
                        bottom: bottom_right.y.to_pt() as f32,
                    });
                }
                _ => {}
            }
        }
    }

    fn extend(&mut self, new: LineBox) {
        match self
            .boxes
            .iter_mut()
            .find(|line_box| line_box.line == new.line && line_box.page == new.page)
        {
            Some(line_box) => {
                line_box.left = line_box.left.min(new.left);
                line_box.top = line_box.top.min(new.top);
                line_box.right = line_box.right.max(new.right);
                line_box.bottom = line_box.bottom.max(new.bottom);
            }
            None => self.boxes.push(new),
        }
    }
}

fn line_id(label: Label) -> Option<LineId> {
    let name = label.resolve();
    let id = name.as_str().strip_prefix(LINE_LABEL_PREFIX)?;
    Uuid::parse_str(id).ok().map(LineId)
}

/// A Typst world with one source file and no access to the disk.
struct SingleFileWorld<'a> {
    typesetter: &'a Typesetter,
    source: Source,
}

impl World for SingleFileWorld<'_> {
    fn library(&self) -> &LazyHash<TypstLibrary> {
        &self.typesetter.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.typesetter.fonts.book()
    }

    fn main(&self) -> FileId {
        self.source.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.source.id() {
            Ok(self.source.clone())
        } else {
            Err(FileError::AccessDenied)
        }
    }

    fn file(&self, _id: FileId) -> FileResult<Bytes> {
        Err(FileError::AccessDenied)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.typesetter.fonts.font(index)
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Job;

    fn sample_library() -> Library {
        match crate::model::sample::library() {
            Ok(library) => library,
            Err(error) => panic!("{error}"),
        }
    }

    fn typeset(resume: &TailoredResume, library: &Library) -> TypesetResume {
        let typesetter = Typesetter::new();
        match typesetter.render(&resume_markup(resume, library), 0.25) {
            Ok(typeset) => typeset,
            Err(error) => panic!("didn't compile: {error}"),
        }
    }

    #[test]
    fn call_escapes_quotes_and_backslashes() {
        assert_eq!(
            call("plain", r#"say "hi" \o/"#),
            "\n#plain(\"say \\\"hi\\\" \\\\o/\")"
        );
    }

    #[test]
    fn every_line_of_every_sample_is_located() {
        let library = sample_library();
        for resume in library.jobs.iter().filter_map(Job::current_resume) {
            let typeset = typeset(resume, &library);
            for line in resume.lines() {
                assert!(
                    typeset.layout.boxes_of(line.id).next().is_some(),
                    "{:?} has no box",
                    line.text
                );
            }
        }
    }

    #[test]
    fn a_click_inside_a_line_finds_it() {
        let library = sample_library();
        for resume in library.jobs.iter().filter_map(Job::current_resume) {
            let typeset = typeset(resume, &library);
            for line_box in &typeset.layout.boxes {
                let x = (line_box.left + line_box.right) / 2.;
                let y = (line_box.top + line_box.bottom) / 2.;
                assert_eq!(
                    typeset.layout.line_at(line_box.page, x, y),
                    Some(line_box.line)
                );
            }
        }
    }

    #[test]
    fn a_line_split_by_a_page_break_has_a_box_on_each_page() {
        let text = "A sentence long enough to fill some of the page width. ".repeat(250);
        let line = crate::model::Line {
            id: LineId::new(),
            generated: text.clone(),
            text,
            cites: Vec::new(),
        };
        let resume = TailoredResume {
            id: crate::model::ResumeId::new(),
            generated_at: chrono::Utc::now(),
            sections: vec![crate::model::Section {
                title: String::new(),
                lines: vec![line.clone()],
            }],
            unsupported_claims: Vec::new(),
            basis: Vec::new(),
        };
        let typeset = typeset(&resume, &Library::default());
        let pages: Vec<usize> = typeset
            .layout
            .boxes_of(line.id)
            .map(|line_box| line_box.page)
            .collect();
        // The paragraph fills every page, so it has one box on each.
        let page_count = typeset.layout.pages.len();
        assert!(page_count >= 2);
        assert_eq!(pages, (0..page_count).collect::<Vec<_>>());
    }

    #[test]
    fn the_long_sample_runs_onto_a_second_page() {
        let library = sample_library();
        let Some(long) = library
            .jobs
            .iter()
            .filter_map(Job::current_resume)
            .max_by_key(|resume| resume.lines().count())
        else {
            panic!("no sample résumés");
        };
        assert!(typeset(long, &library).layout.pages.len() >= 2);
    }
}
