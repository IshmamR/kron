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

use crate::model::{Block, Line, LineId, SectionKind, TailoredResume};

/// The templates that come with the app. Each defines `render(resume)`; the
/// shape of `resume` is described in `resume_markup`.
pub static TEMPLATES: [Template; 3] = [
    Template {
        key: "modern",
        name: "Modern",
        source: include_str!("../assets/typst/modern.typ"),
    },
    Template {
        key: "classic",
        name: "Classic",
        source: include_str!("../assets/typst/classic.typ"),
    },
    Template {
        key: "sidebar",
        name: "Sidebar",
        source: include_str!("../assets/typst/sidebar.typ"),
    },
];
pub const DEFAULT_TEMPLATE: &str = "modern";
const LINE_LABEL_PREFIX: &str = "line-";
/// Clicks this close above or below a line still hit it, so the gaps
/// between lines aren't dead space.
const HIT_SLOP: f32 = 2.;
/// Space kept between the click areas of lines that share a row.
const ROW_GAP: f32 = 4.;
/// Lines on a row this close follow on from each other, like a title and its
/// employer; further apart, they're separate, like a sidebar and the main
/// column.
const RUN_GAP: f32 = 24.;

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

/// A line's place on one page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineBox {
    pub line: LineId,
    pub page: usize,
    /// Around the line's text, and nothing else: what's highlighted.
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    /// What a click selects the line in: its text, widened across its column
    /// so a click beside a short line still finds it. A line sharing its row,
    /// like a title with its dates flush right, shares the row's space.
    pub hit_left: f32,
    pub hit_right: f32,
    /// Where to mark the line from the margin: the left of its column, which
    /// in a two-column design isn't the page's.
    pub mark_left: f32,
    /// The middle of the line's first row, so a mark beside a line that wraps
    /// is beside where it starts.
    pub mark_middle: f32,
}

impl ResumeLayout {
    fn of(document: &PagedDocument) -> Self {
        let mut collector = BoxCollector::default();
        for (page, contents) in document.pages().iter().enumerate() {
            collector.collect(&contents.frame, Transform::identity(), page);
        }
        let tight = collector.boxes;
        let pages = document
            .pages()
            .iter()
            .map(|contents| PageSize {
                width: contents.frame.width().to_pt() as f32,
                height: contents.frame.height().to_pt() as f32,
            })
            .collect();
        let boxes = tight
            .iter()
            .map(|line_box| {
                let (column_left, column_right) = column(line_box, &tight);
                let (mut hit_left, mut hit_right) = (column_left, column_right);
                // Lines sharing a row split the space between them.
                for other in tight.iter().filter(|other| shares_row(line_box, other)) {
                    if other.right <= line_box.left {
                        hit_left = hit_left.max((other.right + line_box.left + ROW_GAP) / 2.);
                    } else if other.left >= line_box.right {
                        hit_right = hit_right.min((line_box.right + other.left - ROW_GAP) / 2.);
                    }
                }
                LineBox {
                    hit_left: hit_left.min(line_box.left),
                    hit_right: hit_right.max(line_box.right),
                    mark_left: column_left,
                    ..*line_box
                }
            })
            .collect();

        Self { pages, boxes }
    }

    /// Where boxes overlap, as a large name's does the contacts set tight
    /// below it, the box whose middle is nearest wins.
    pub fn line_at(&self, page: usize, x: f32, y: f32) -> Option<LineId> {
        self.boxes
            .iter()
            .filter(|line_box| {
                line_box.page == page
                    && (line_box.hit_left..=line_box.hit_right).contains(&x)
                    && (line_box.top - HIT_SLOP..=line_box.bottom + HIT_SLOP).contains(&y)
            })
            .min_by(|a, b| {
                let distance = |line_box: &LineBox| (line_box.top + line_box.bottom - 2. * y).abs();
                distance(a).total_cmp(&distance(b))
            })
            .map(|line_box| line_box.line)
    }

    pub fn boxes_of(&self, line: LineId) -> impl Iterator<Item = &LineBox> {
        self.boxes
            .iter()
            .filter(move |line_box| line_box.line == line)
    }

    /// The lines in the order they're read on the page: page by page, each
    /// column top to bottom, and each row left to right. Templates place
    /// lines where they like, so this can differ from the résumé's order.
    pub fn reading_order(&self) -> Vec<LineId> {
        // A line's first box decides where it is.
        let mut firsts: Vec<&LineBox> = Vec::new();
        for line_box in &self.boxes {
            if !firsts.iter().any(|first| first.line == line_box.line) {
                firsts.push(line_box);
            }
        }
        // Rows are found top down: a line whose middle is within the row so
        // far, like dates a point higher than their title, is in it.
        firsts.sort_by(|a, b| {
            (a.page, a.mark_left.round() as i32)
                .cmp(&(b.page, b.mark_left.round() as i32))
                .then(a.top.total_cmp(&b.top))
        });
        let mut keyed: Vec<((usize, i32, usize), f32, LineId)> = Vec::new();
        let mut row = 0;
        let mut row_bottom = f32::MIN;
        let mut previous_column = None;
        for line_box in firsts {
            let column = (line_box.page, line_box.mark_left.round() as i32);
            if previous_column != Some(column) || line_box.mark_middle >= row_bottom {
                row += 1;
                row_bottom = line_box.bottom;
            } else {
                row_bottom = row_bottom.max(line_box.bottom);
            }
            previous_column = Some(column);
            keyed.push(((column.0, column.1, row), line_box.left, line_box.line));
        }
        keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        keyed.into_iter().map(|(_, _, line)| line).collect()
    }
}

/// Whether `other` is another line on the same row of the same page. Rows set
/// tight touch by a few points of ascender and descender, so lines share a
/// row only when they overlap by half the shorter one's height.
fn shares_row(line_box: &LineBox, other: &LineBox) -> bool {
    let overlap = line_box.bottom.min(other.bottom) - line_box.top.max(other.top);
    let shorter = (line_box.bottom - line_box.top).min(other.bottom - other.top);
    other.line != line_box.line && other.page == line_box.page && overlap > shorter / 2.
}

/// A design for résumés: Typst source that defines `render(resume)`.
pub struct Template {
    pub key: &'static str,
    pub name: &'static str,
    pub source: &'static str,
}

/// The template called `key`, or the default one for a key that isn't known,
/// so a résumé always has a look.
pub fn template(key: &str) -> &'static Template {
    TEMPLATES
        .iter()
        .find(|template| template.key == key)
        .or_else(|| TEMPLATES.iter().find(|template| template.key == DEFAULT_TEMPLATE))
        .unwrap_or(&TEMPLATES[0])
}

/// The résumé's template, then the résumé as a Typst value handed to the
/// template's `render`:
///
/// ```typst
/// (sections: ((kind: "experience", title: "Experience", blocks: (…)), …))
/// ```
///
/// A section's `kind` is one of "header", "summary", "experience",
/// "education", "projects", "skills" and "other". Its blocks are
/// dictionaries, each with a `kind`:
///
/// - "header": `name`, and `contacts`, each `(text, link)`; `link` is a URL
///   or `none`
/// - "entry": `title`, `organization`, `dates`, `location`, `stack` (what a
///   project was built with), each content or `none`, and `bullets`
/// - "skills": `label`, content or `none`, and `items`
/// - "text" and "item": `body`, a paragraph or an item of a list
///
/// Every piece of content is one line of the résumé, labelled so the app can
/// find where it was laid out. A template may style, move or leave out a
/// line, but must show it as the content it's given: turned into a string,
/// it loses its label.
pub fn resume_markup(resume: &TailoredResume) -> String {
    let mut markup = String::from(template(&resume.template).source);
    markup.push_str("\n#let resume = (sections: (\n");
    for section in &resume.sections {
        markup.push_str(&format!(
            "  (kind: {}, title: {}, blocks: (\n",
            string_literal(section_kind(section.kind)),
            string_literal(&section.title)
        ));
        for block in &section.blocks {
            markup.push_str("    ");
            markup.push_str(&block_value(block));
            markup.push_str(",\n");
        }
        markup.push_str("  )),\n");
    }
    markup.push_str("))\n#render(resume)\n");
    markup
}

fn section_kind(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::Header => "header",
        SectionKind::Summary => "summary",
        SectionKind::Experience => "experience",
        SectionKind::Education => "education",
        SectionKind::Projects => "projects",
        SectionKind::Skills => "skills",
        SectionKind::Other => "other",
    }
}

fn block_value(block: &Block) -> String {
    match block {
        Block::Header { name, contacts } => {
            let contacts: String = contacts
                .iter()
                .map(|contact| {
                    let link = contact_link(&contact.text)
                        .map_or("none".to_string(), |link| string_literal(&link));
                    format!("(text: {}, link: {link}), ", content(contact))
                })
                .collect();
            format!(
                "(kind: \"header\", name: {}, contacts: ({contacts}))",
                content(name)
            )
        }
        Block::Entry(entry) => {
            let bullets: String = entry
                .bullets
                .iter()
                .map(|bullet| format!("{}, ", content(bullet)))
                .collect();
            format!(
                "(kind: \"entry\", title: {}, organization: {}, dates: {}, location: {}, \
                 stack: {}, bullets: ({bullets}))",
                content(&entry.title),
                optional_content(entry.organization.as_ref()),
                optional_content(entry.dates.as_ref()),
                optional_content(entry.location.as_ref()),
                optional_content(entry.stack.as_ref()),
            )
        }
        Block::Skills { label, items } => format!(
            "(kind: \"skills\", label: {}, items: {})",
            optional_content(label.as_ref()),
            content(items)
        ),
        Block::Text(line) => format!("(kind: \"text\", body: {})", content(line)),
        Block::Item(line) => format!("(kind: \"item\", body: {})", content(line)),
    }
}

/// The line as content labelled with its id. The text goes in as a string
/// literal rather than markup, so `*`, `#` or `$` in it stay plain text.
fn content(line: &Line) -> String {
    format!(
        "[#{}<{LINE_LABEL_PREFIX}{}>]",
        string_literal(&line.text),
        line.id.0
    )
}

fn optional_content(line: Option<&Line>) -> String {
    line.map_or("none".to_string(), content)
}

fn string_literal(text: &str) -> String {
    let mut literal = String::from("\"");
    for character in text.chars() {
        match character {
            '\\' => literal.push_str("\\\\"),
            '"' => literal.push_str("\\\""),
            '\n' => literal.push_str("\\n"),
            character => literal.push(character),
        }
    }
    literal.push('"');
    literal
}

/// Where a contact leads: an email address, a phone number or a site.
fn contact_link(text: &str) -> Option<String> {
    let text = text.trim();
    if text.contains(' ') && !text.starts_with('+') {
        return None;
    }
    if text.contains('@') {
        Some(format!("mailto:{text}"))
    } else if text.starts_with('+')
        || text.chars().all(|character| character.is_ascii_digit() || " -()".contains(character))
    {
        let digits: String = text
            .chars()
            .filter(|character| character.is_ascii_digit() || *character == '+')
            .collect();
        Some(format!("tel:{digits}"))
    } else if text.starts_with("https://") || text.starts_with("http://") {
        Some(text.to_string())
    } else if text.contains('.') {
        Some(format!("https://{text}"))
    } else {
        None
    }
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
                    let (left, top) = (top_left.x.to_pt() as f32, top_left.y.to_pt() as f32);
                    let (right, bottom) =
                        (bottom_right.x.to_pt() as f32, bottom_right.y.to_pt() as f32);
                    self.extend(LineBox {
                        line,
                        page,
                        left,
                        top,
                        right,
                        bottom,
                        hit_left: left,
                        hit_right: right,
                        mark_left: left,
                        mark_middle: (top + bottom) / 2.,
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
            // `mark_middle` stays the first run's: text comes in reading
            // order, so that's the line's first row.
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

/// The left and right of the column `line_box` is in: as far as the lines
/// above and below its run of text reach, and the lines above and below
/// those. A sidebar's lines don't reach under the main column's, so the two
/// stay apart.
fn column(line_box: &LineBox, boxes: &[LineBox]) -> (f32, f32) {
    let mut span = (run_end(line_box, boxes, false), run_end(line_box, boxes, true));
    // Each round only widens, so this settles; the bound is a guard.
    for _ in 0..boxes.len() {
        let wider = boxes
            .iter()
            .filter(|other| other.page == line_box.page && other.left < span.1 && span.0 < other.right)
            .fold(span, |(left, right), other| (left.min(other.left), right.max(other.right)));
        if wider == span {
            break;
        }
        span = wider;
    }
    span
}

/// Where the run of text `line_box` is part of starts, or with `rightward`,
/// ends. Lines on its row it follows closely, like a title and its employer,
/// are one run; ones further off, like a sidebar beside it, aren't.
fn run_end(line_box: &LineBox, boxes: &[LineBox], rightward: bool) -> f32 {
    let mut end = line_box;
    // Each step moves outward, so this ends; the bound is a guard all the
    // same.
    for _ in 0..boxes.len() {
        let next = boxes
            .iter()
            .filter(|other| shares_row(end, other))
            .filter(|other| {
                if rightward {
                    other.left >= end.right && other.left - end.right < RUN_GAP
                } else {
                    other.right <= end.left && end.left - other.right < RUN_GAP
                }
            })
            .min_by(|a, b| {
                if rightward {
                    a.left.total_cmp(&b.left)
                } else {
                    b.right.total_cmp(&a.right)
                }
            });
        match next {
            Some(next) => end = next,
            None => break,
        }
    }
    if rightward { end.right } else { end.left }
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
    use crate::model::{Job, Library, ResumeId, Section};

    fn sample_library() -> Library {
        match crate::model::sample::library() {
            Ok(library) => library,
            Err(error) => panic!("{error}"),
        }
    }

    fn typeset(typesetter: &Typesetter, resume: &TailoredResume) -> TypesetResume {
        match typesetter.render(&resume_markup(resume), 0.25) {
            Ok(typeset) => typeset,
            Err(error) => panic!("didn't compile in {:?}: {error}", resume.template),
        }
    }

    /// Every sample résumé, laid out with every template.
    fn every_sample_in_every_template() -> Vec<(TailoredResume, TypesetResume)> {
        let typesetter = Typesetter::new();
        let library = sample_library();
        library
            .jobs
            .iter()
            .filter_map(Job::current_resume)
            .flat_map(|resume| {
                TEMPLATES.iter().map(|template| TailoredResume {
                    template: template.key.to_string(),
                    ..resume.clone()
                })
            })
            .map(|resume| {
                let typeset = typeset(&typesetter, &resume);
                (resume, typeset)
            })
            .collect()
    }

    #[test]
    fn string_literals_escape_quotes_and_backslashes() {
        assert_eq!(string_literal(r#"say "hi" \o/"#), r#""say \"hi\" \\o/""#);
    }

    #[test]
    fn contacts_link_where_they_lead() {
        assert_eq!(
            contact_link("alex@example.com").as_deref(),
            Some("mailto:alex@example.com")
        );
        assert_eq!(contact_link("+880 1711 555 210").as_deref(), Some("tel:+8801711555210"));
        assert_eq!(
            contact_link("github.com/alexr").as_deref(),
            Some("https://github.com/alexr")
        );
        assert_eq!(contact_link("Dhaka, Bangladesh"), None);
    }

    #[test]
    fn every_line_is_located_in_every_template() {
        for (resume, typeset) in every_sample_in_every_template() {
            for line in resume.lines() {
                assert!(
                    typeset.layout.boxes_of(line.id).next().is_some(),
                    "{:?} has no box in {:?}",
                    line.text,
                    resume.template
                );
            }
        }
    }

    #[test]
    fn a_click_inside_a_line_finds_it_in_every_template() {
        for (resume, typeset) in every_sample_in_every_template() {
            for line_box in &typeset.layout.boxes {
                let x = (line_box.left + line_box.right) / 2.;
                let y = (line_box.top + line_box.bottom) / 2.;
                let found = typeset.layout.line_at(line_box.page, x, y);
                let text = |id: Option<LineId>| {
                    id.and_then(|id| resume.line(id)).map(|line| line.text.clone())
                };
                assert_eq!(
                    text(found),
                    text(Some(line_box.line)),
                    "in {:?}, {line_box:?}",
                    resume.template
                );
            }
        }
    }

    fn overlap(a: (f32, f32), b: (f32, f32)) -> bool {
        a.0 < b.1 && b.0 < a.1
    }

    #[test]
    fn a_click_area_never_covers_another_lines_text() {
        for (resume, typeset) in every_sample_in_every_template() {
            let boxes = &typeset.layout.boxes;
            for line_box in boxes {
                for other in boxes.iter().filter(|other| shares_row(line_box, other)) {
                    assert!(
                        !overlap(
                            (line_box.hit_left, line_box.hit_right),
                            (other.left, other.right)
                        ),
                        "in {:?}, clicking {:?} could select {:?}",
                        resume.template,
                        resume.line(other.line).map(|line| &line.text),
                        resume.line(line_box.line).map(|line| &line.text),
                    );
                }
            }
        }
    }

    #[test]
    fn sidebar_lines_and_main_column_lines_stay_apart() {
        for (resume, typeset) in every_sample_in_every_template() {
            if resume.template != "sidebar" {
                continue;
            }
            let Some(Block::Header { name, .. }) = resume.sections.first().and_then(|section| section.blocks.first()) else {
                panic!("no header");
            };
            let Some(main_left) = typeset.layout.boxes_of(name.id).next().map(|line_box| line_box.left) else {
                panic!("the name isn't on the page");
            };
            for line_box in &typeset.layout.boxes {
                let in_main = line_box.left >= main_left - 1.;
                let text = resume.line(line_box.line).map(|line| &line.text);
                if in_main {
                    assert!(line_box.hit_left >= main_left - 1., "{text:?} reaches into the sidebar");
                    assert!(line_box.mark_left >= main_left - 1., "{text:?} is marked in the sidebar");
                } else {
                    assert!(line_box.hit_right < main_left, "{text:?} reaches into the main column");
                }
            }
        }
    }

    #[test]
    fn an_entry_title_is_on_the_page_its_first_bullet_is() {
        for (resume, typeset) in every_sample_in_every_template() {
            let entries = resume.sections.iter().flat_map(|section| &section.blocks).filter_map(
                |block| match block {
                    Block::Entry(entry) => Some(entry),
                    _ => None,
                },
            );
            for entry in entries {
                let Some(bullet) = entry.bullets.first() else {
                    continue;
                };
                let page = |line: &Line| typeset.layout.boxes_of(line.id).next().map(|line_box| line_box.page);
                assert_eq!(
                    page(&entry.title),
                    page(bullet),
                    "in {:?}, {:?} is left at the foot of a page",
                    resume.template,
                    entry.title.text
                );
            }
        }
    }

    #[test]
    fn reading_order_reads_rows_left_to_right_and_columns_one_at_a_time() {
        for (resume, typeset) in every_sample_in_every_template() {
            let order = typeset.layout.reading_order();
            let position = |line: &Line| order.iter().position(|&id| id == line.id);
            for line in resume.lines() {
                assert!(position(line).is_some(), "{:?} isn't in the order", line.text);
            }
            let Some(Block::Header { name, contacts }) =
                resume.sections.first().and_then(|section| section.blocks.first())
            else {
                panic!("no header");
            };
            if resume.template == "sidebar" {
                // The contacts are in the sidebar, read before the main column.
                assert!(contacts.iter().all(|contact| position(contact) < position(name)));
            } else {
                assert!(
                    contacts.iter().all(|contact| position(name) < position(contact)),
                    "in {:?}",
                    resume.template
                );
            }
        }
    }

    #[test]
    fn a_line_split_by_a_page_break_has_a_box_on_each_page() {
        let text = "A sentence long enough to fill some of the page width. ".repeat(250);
        let line = Line {
            id: LineId::new(),
            generated: text.clone(),
            text,
            cites: Vec::new(),
        };
        let resume = TailoredResume {
            id: ResumeId::new(),
            generated_at: chrono::Utc::now(),
            template: DEFAULT_TEMPLATE.to_string(),
            sections: vec![Section {
                kind: SectionKind::Summary,
                title: String::new(),
                blocks: vec![Block::Text(line.clone())],
            }],
            unsupported_claims: Vec::new(),
            basis: Vec::new(),
        };
        let typeset = typeset(&Typesetter::new(), &resume);
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
        assert!(typeset(&Typesetter::new(), long).layout.pages.len() >= 2);
    }

    #[test]
    fn an_unknown_template_falls_back_to_the_default() {
        assert_eq!(template("gone").key, DEFAULT_TEMPLATE);
    }
}

