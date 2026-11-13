// Reads a PDF's text with hayro, the PDF reader Typst uses for PDF images.
// hayro gives each glyph's text, position and size; this puts the glyphs
// into words, lines and reading order. Columns, like a sidebar, are
// found by XY-cut (Nagy and Seth, 1984): a region is split at its widest
// empty band, down the page or across it, and each part is read in turn.
// The gaps that separate words, runs and lines are pdfminer.six's (LAParams).

use std::ops::Range;
use std::sync::Arc;

use hayro_interpret::font::Glyph as PdfGlyph;
use hayro_interpret::font::GlyphRun;
use hayro_interpret::hayro_cmap::BfString;
use hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, RectExt, SoftMask, TransformExt, interpret_page,
};
use hayro_syntax::content::ops::TypedInstruction;
use hayro_syntax::object::{Array, Dict, Rect as PdfRect, String as PdfString, dict_or_stream};
use hayro_syntax::page::Page;
use hayro_syntax::{LoadPdfError, Pdf};
use kurbo::{Affine, BezPath, Point, Rect};

use super::{Error, Link, Reading, TextLine, Warning};

/// hayro draws glyphs in a 1000-unit em, the unit of PDF font metrics.
const GLYPH_UNITS: f64 = 1000.;
/// Characters on a line further apart than this, relative to the larger of
/// their width and height, are separate words: pdfminer.six's `word_margin`.
const WORD_MARGIN: f64 = 0.1;
/// Further apart than this, relative to the wider one, they're separate
/// runs of text: `char_margin`.
const CHAR_MARGIN: f64 = 2.;
/// Text overlapping by less than this, relative to the shorter one's
/// height, is on different lines: `line_overlap`.
const LINE_OVERLAP: f64 = 0.5;
/// Lines further apart than this, relative to their height, are in separate
/// blocks: `line_margin`.
const LINE_MARGIN: f64 = 0.5;
/// A gap down a region is between the cells of a table, not columns, when
/// more than this share of the runs on one side have one beside them on the
/// other. Ours, from the test PDFs: in a table, the side with fewer cells
/// has every one beside another, as dates beside employers do, while two
/// columns of text line up at most about half the time.
const TABLE_ROWS: f64 = 0.9;
/// The same text drawn again this close, along the line and across it,
/// relative to the font size, is a duplicate, as fake bold and drop shadows
/// draw: poppler's `dupMaxPriDelta` and `dupMaxSecDelta`.
const DUPLICATE_ALONG: f64 = 0.1;
const DUPLICATE_ACROSS: f64 = 0.2;
/// A Type 3 glyph with nothing after it on its line is taken to be this wide,
/// relative to its size: ours, about a lowercase letter's advance.
const DRAWN_WIDTH: f64 = 0.5;

/// The text of the PDF in `bytes`, in reading order.
pub fn read(bytes: Vec<u8>) -> Result<Reading, Error> {
    let pdf = Pdf::new(Arc::new(bytes)).map_err(|error| match error {
        LoadPdfError::Decryption(_) => Error::Encrypted,
        LoadPdfError::Invalid => Error::Invalid,
    })?;
    let cache = InterpreterCache::new();
    let mut reading = Reading::default();
    for (number, page) in pdf.pages().iter().enumerate() {
        let transform = page.initial_transform(true).to_kurbo();
        let (width, height) = page.render_dimensions();
        let mut context = Context::new(
            transform,
            Rect::new(0., 0., width.into(), height.into()),
            &cache,
            page.xref(),
            InterpreterSettings::default(),
        );
        let mut drawn = Drawn {
            actual_texts: actual_texts(page).into_iter(),
            ..Drawn::default()
        };
        interpret_page(page, &mut context, &mut drawn);
        drawn.measure();
        if drawn.picture && drawn.glyphs.is_empty() {
            reading.warnings.push(Warning::NoText { page: number });
        }
        if drawn.lost_text {
            reading.warnings.push(Warning::DrawnFont { page: number });
        }
        let first_line = reading.lines.len();
        let placed = write_page(&mut reading, number, &drawn.glyphs);
        for (address, rect) in links(page, transform) {
            let covered: Vec<&Range<usize>> = placed
                .glyphs
                .iter()
                .filter(|(glyph, _)| rect.contains(glyph.center()))
                .map(|(_, span)| span)
                .collect();
            let span = covered
                .iter()
                .map(|span| span.start)
                .min()
                .zip(covered.iter().map(|span| span.end).max())
                .map(|(start, end)| start..end);
            // The line it covers, or else the one beside it, as an icon
            // beside a project's name is.
            let line = placed
                .lines
                .iter()
                .enumerate()
                .map(|(index, line)| {
                    let covers = rect.x1.min(line.x1) > rect.x0.max(line.x0);
                    (index, covers, rect.y1.min(line.y1) - rect.y0.max(line.y0))
                })
                .filter(|(_, _, overlap)| *overlap > 0.)
                .max_by(|first, second| first.1.cmp(&second.1).then(first.2.total_cmp(&second.2)))
                .map(|(index, ..)| first_line + index);
            reading.links.push(Link {
                address,
                span,
                line,
            });
        }
    }
    if reading.lines.is_empty() {
        return Err(Error::NoText);
    }
    Ok(reading)
}

/// A glyph of text as drawn, in points from the page's top left.
#[derive(Debug)]
struct Glyph {
    text: String,
    left: f64,
    right: f64,
    baseline: f64,
    size: f64,
    /// A Type 3 glyph, whose advance hayro doesn't give: its right is found
    /// from what's drawn after it.
    unmeasured: bool,
}

impl Glyph {
    /// Its em box, standing on the baseline.
    fn rect(&self) -> Rect {
        Rect::new(
            self.left,
            self.baseline - self.size,
            self.right,
            self.baseline,
        )
    }

    fn center(&self) -> Point {
        self.rect().center()
    }

    fn width(&self) -> f64 {
        self.right - self.left
    }
}

/// Collects the glyphs a page draws, leaving out icons, spaces and
/// duplicates: an icon font's glyphs have no text, or text from the Private
/// Use Area.
#[derive(Default)]
struct Drawn {
    glyphs: Vec<Glyph>,
    /// Whether it draws an image.
    picture: bool,
    /// Whether it draws a Type 3 glyph that says nothing of what it is,
    /// so a letter is missing. Chrome draws text in them that reads fine.
    lost_text: bool,
    /// Where a Type 3 font draws spaces.
    spaces: Vec<Point>,
    /// The `actual_texts` of the page's marked content not yet begun.
    actual_texts: std::vec::IntoIter<Option<String>>,
    /// Those of the marked content begun and not ended, innermost last;
    /// taken by the first glyph that needs it.
    spans: Vec<Option<String>>,
}

impl<'a> Device<'a> for Drawn {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {
        self.picture = true;
    }
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn begin_marked_content(&mut self, _: &[u8], _: Option<&Dict<'_>>) {
        self.spans.push(self.actual_texts.next().flatten());
    }
    fn end_marked_content(&mut self) {
        self.spans.pop();
    }

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for glyph in run.glyphs() {
            self.draw_glyph(glyph, props.transform * glyph.transform());
        }
    }
}

impl Drawn {
    /// Adds a glyph drawn with `transform`, from its glyph space to the page.
    fn draw_glyph(&mut self, glyph: &PdfGlyph<'_>, transform: Affine) {
        let text = match glyph.as_unicode() {
            Some(BfString::Char(character)) => Some(character.to_string()),
            Some(BfString::String(text)) => Some(text),
            None => None,
        };
        let has_text = text.is_some();
        // A glyph without text of its own, as Chrome draws the en dash it
        // sets between digits, may have it in the span it's drawn in.
        let Some(text) = text
            .filter(|text| !text.chars().any(icon))
            .or_else(|| self.spans.last_mut().and_then(Option::take))
        else {
            // An icon's text is in the Private Use Area: only a glyph with
            // none at all lost a letter.
            if !has_text && let PdfGlyph::Type3(_) = glyph {
                self.lost_text = true;
            }
            return;
        };
        let text = unligated(text);
        let drawn_font = matches!(glyph, PdfGlyph::Type3(_));
        let origin = transform * Point::ZERO;
        if text.chars().all(char::is_whitespace) {
            // Where a Type 3 font's space is drawn, the glyph before it ends.
            if drawn_font {
                self.spaces.push(origin);
            }
            return;
        }
        if text.chars().any(icon) {
            return;
        }
        // Type 3 glyphs are drawings, with no advance to read.
        let advance = match glyph {
            PdfGlyph::Outline(outline) => outline.advance_width().unwrap_or(0.),
            PdfGlyph::Type3(_) => 0.,
        };
        let end = transform * Point::new(advance.into(), 0.);
        // Text set vertically or at an angle isn't read.
        if (end.y - origin.y).abs() > (end.x - origin.x).abs() {
            return;
        }
        let [_, _, x, y, _, _] = transform.as_coeffs();
        let size = x.hypot(y) * GLYPH_UNITS;
        if self.glyphs.iter().any(|drawn| {
            drawn.text == text
                && (drawn.left - origin.x).abs() < DUPLICATE_ALONG * size
                && (drawn.baseline - origin.y).abs() < DUPLICATE_ACROSS * size
        }) {
            return;
        }
        self.glyphs.push(Glyph {
            text,
            left: origin.x,
            right: end.x.max(origin.x),
            baseline: origin.y,
            size,
            unmeasured: drawn_font,
        });
    }

    /// Ends each Type 3 glyph where the next glyph or space on its line
    /// starts, as text is set, so a word's letters touch and the gap a
    /// space leaves stays; or else a letter's width on from its start.
    fn measure(&mut self) {
        for index in 0..self.glyphs.len() {
            let glyph = &self.glyphs[index];
            if !glyph.unmeasured {
                continue;
            }
            let on_line = |point: Point| {
                point.x > glyph.left
                    && point.x - glyph.left < CHAR_MARGIN * glyph.size
                    && (point.y - glyph.baseline).abs() < DUPLICATE_ACROSS * glyph.size
            };
            let next = self
                .glyphs
                .get(index + 1)
                .map(|next| Point::new(next.left, next.baseline))
                .into_iter()
                .chain(self.spaces.iter().copied())
                .filter(|point| on_line(*point))
                .map(|point| point.x)
                .reduce(f64::min);
            let right = next.unwrap_or(glyph.left + DRAWN_WIDTH * glyph.size);
            self.glyphs[index].right = right;
        }
    }
}

/// Where a page's text went in the reading: its glyphs' spans in the text,
/// and its lines' rectangles, in the order of `Reading::lines`.
struct Placed<'a> {
    glyphs: Vec<(&'a Glyph, Range<usize>)>,
    lines: Vec<Rect>,
}

/// Glyphs that follow on from each other on one line, in the order drawn.
struct Run {
    glyphs: Range<usize>,
    rect: Rect,
    size: f64,
    /// The average width of its characters.
    character: f64,
    /// It ends with a comma, so the line under it goes on with it.
    runs_on: bool,
}

/// Adds page `number`'s `glyphs` to `reading` as lines, in reading order.
fn write_page<'a>(reading: &mut Reading, number: usize, glyphs: &'a [Glyph]) -> Placed<'a> {
    let runs = runs(glyphs);
    let mut rows = Vec::new();
    read_order(&runs, (0..runs.len()).collect(), &mut rows);
    let mut placed = Placed {
        glyphs: Vec::new(),
        lines: Vec::new(),
    };
    let mut above: Option<(Rect, f64)> = None;
    for row in rows {
        let rect = row
            .iter()
            .map(|run| runs[*run].rect)
            .reduce(|first, second| first.union(second))
            .unwrap_or_default();
        let size = row.iter().map(|run| runs[*run].size).fold(0., f64::max);
        if !reading.text.is_empty() {
            let new_block = match above {
                // Back up the page means the next column.
                Some((previous, height)) => {
                    rect.y0 < previous.y0 || rect.y0 - previous.y1 > LINE_MARGIN * height.min(size)
                }
                None => true,
            };
            reading.text.push_str(if new_block { "\n\n" } else { "\n" });
        }
        let start = reading.text.len();
        for (position, run) in row.iter().enumerate() {
            if position > 0 {
                reading.text.push(' ');
            }
            let run = &glyphs[runs[*run].glyphs.clone()];
            let tracking = tracking(run);
            let mut previous: Option<&Glyph> = None;
            for glyph in run {
                if previous.is_some_and(|previous| {
                    glyph.left - previous.right - tracking
                        > WORD_MARGIN * glyph.width().max(previous.width()).max(glyph.size)
                }) {
                    reading.text.push(' ');
                }
                let glyph_start = reading.text.len();
                reading.text.push_str(&glyph.text);
                placed.glyphs.push((glyph, glyph_start..reading.text.len()));
                previous = Some(glyph);
            }
        }
        reading.lines.push(TextLine {
            span: start..reading.text.len(),
            page: number,
            size: size as f32,
        });
        placed.lines.push(rect);
        above = Some((rect, size));
    }
    placed
}

/// The space set between every two letters of `run`, as a letter-spaced
/// "C O N T A C T" heading has, so only a wider gap is a word's end: the
/// middle gap, since most gaps are between letters. Ours.
fn tracking(run: &[Glyph]) -> f64 {
    let mut gaps: Vec<f64> = run
        .windows(2)
        .map(|pair| pair[1].left - pair[0].right)
        .collect();
    // Too few gaps to tell letters' from words'.
    if gaps.len() < 3 {
        return 0.;
    }
    gaps.sort_by(f64::total_cmp);
    gaps[gaps.len() / 2].max(0.)
}

/// `text` with typographic ligatures, as "ﬁ", written as their letters, so
/// "proﬁling" is found as "profiling".
fn unligated(text: String) -> String {
    if !text
        .chars()
        .any(|character| ('\u{FB00}'..='\u{FB06}').contains(&character))
    {
        return text;
    }
    text.chars()
        .map(|character| match character {
            '\u{FB00}' => "ff".to_owned(),
            '\u{FB01}' => "fi".to_owned(),
            '\u{FB02}' => "fl".to_owned(),
            '\u{FB03}' => "ffi".to_owned(),
            '\u{FB04}' => "ffl".to_owned(),
            '\u{FB05}' | '\u{FB06}' => "st".to_owned(),
            other => other.to_string(),
        })
        .collect()
}

/// `glyphs` grouped into runs: glyphs drawn one after another, on the same
/// line and close enough to be one piece of text.
fn runs(glyphs: &[Glyph]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (index, glyph) in glyphs.iter().enumerate() {
        let follows = index
            .checked_sub(1)
            .and_then(|previous| glyphs.get(previous));
        let joins = follows.is_some_and(|previous| {
            let (first, second) = (previous.rect(), glyph.rect());
            let overlap = first.y1.min(second.y1) - first.y0.max(second.y0);
            let distance = (second.x0 - first.x1).max(first.x0 - second.x1).max(0.);
            overlap > LINE_OVERLAP * previous.size.min(glyph.size)
                && distance < CHAR_MARGIN * previous.width().max(glyph.width())
        });
        match runs.last_mut() {
            Some(run) if joins => {
                run.glyphs.end = index + 1;
                run.rect = run.rect.union(glyph.rect());
                run.size = run.size.max(glyph.size);
            }
            _ => runs.push(Run {
                glyphs: index..index + 1,
                rect: glyph.rect(),
                size: glyph.size,
                character: 0.,
                runs_on: false,
            }),
        }
    }
    for run in &mut runs {
        let count = run.glyphs.len() as f64;
        run.character = glyphs[run.glyphs.clone()]
            .iter()
            .map(Glyph::width)
            .sum::<f64>()
            / count;
        run.runs_on = glyphs[run.glyphs.clone()]
            .last()
            .is_some_and(|glyph| glyph.text.trim_end().ends_with(','));
    }
    runs
}

/// Adds the `region`'s runs to `rows` in reading order, each row the runs
/// side by side on one line, left to right. A region is split down its
/// widest gap that separates columns, or else across its widest gap; one
/// it can't split is a row.
fn read_order(runs: &[Run], mut region: Vec<usize>, rows: &mut Vec<Vec<usize>>) {
    if region.is_empty() {
        return;
    }
    if region.len() > 1 {
        if let Some(right) = split_down(runs, &mut region) {
            read_order(runs, region, rows);
            read_order(runs, right, rows);
            return;
        }
        if let Some(below) = split_across(runs, &mut region) {
            read_order(runs, region, rows);
            read_order(runs, below, rows);
            return;
        }
    }
    region.sort_by(|first, second| runs[*first].rect.x0.total_cmp(&runs[*second].rect.x0));
    rows.push(region);
}

/// Splits `region` at its widest gap down the page that separates columns,
/// leaving the left in `region` and returning the right. A gap is at least
/// as wide as the one that separates runs, and isn't between the cells of a
/// table, unless the two sides' lines are out of step as a table's rows
/// never are.
fn split_down(runs: &[Run], region: &mut Vec<usize>) -> Option<Vec<usize>> {
    region.sort_by(|first, second| runs[*first].rect.x0.total_cmp(&runs[*second].rect.x0));
    let character =
        region.iter().map(|run| runs[*run].character).sum::<f64>() / region.len() as f64;
    let mut gaps = Vec::new();
    let mut right_edge = f64::NEG_INFINITY;
    for (index, run) in region.iter().enumerate() {
        let rect = runs[*run].rect;
        if index > 0 && rect.x0 - right_edge >= CHAR_MARGIN * character {
            gaps.push((index, rect.x0 - right_edge));
        }
        right_edge = right_edge.max(rect.x1);
    }
    gaps.sort_by(|first, second| second.1.total_cmp(&first.1));
    let (index, _) = gaps.into_iter().find(|(index, _)| {
        let (left, right) = region.split_at(*index);
        beside(runs, left, right).max(beside(runs, right, left)) <= TABLE_ROWS
            || out_of_step(runs, left, right)
    })?;
    Some(region.split_off(index))
}

/// The share of `side`'s runs that have one of `other`'s beside them, on
/// the same line.
fn beside(runs: &[Run], side: &[usize], other: &[usize]) -> f64 {
    let beside = side
        .iter()
        .filter(|first| {
            other
                .iter()
                .any(|second| same_line(&runs[**first], &runs[*second]))
        })
        .count();
    beside as f64 / side.len() as f64
}

fn same_line(first: &Run, second: &Run) -> bool {
    let overlap = first.rect.y1.min(second.rect.y1) - first.rect.y0.max(second.rect.y0);
    overlap > LINE_OVERLAP * first.size.min(second.size)
}

/// Whether a line going on from the one above it on one side is beside a
/// line starting afresh on the other, as when two columns of skill groups
/// wrap at different lines. A table's cells wrap within their row.
fn out_of_step(runs: &[Run], left: &[usize], right: &[usize]) -> bool {
    let goes_on = |side: &[usize], run: &Run| {
        side.iter().any(|above| {
            let above = &runs[*above];
            above.runs_on
                && run.rect.y0 > above.rect.y0
                && run.rect.y0 - above.rect.y1 < LINE_MARGIN * above.size.max(run.size)
                && run.rect.x0 < above.rect.x1
                && above.rect.x0 < run.rect.x1
        })
    };
    [(left, right), (right, left)].iter().any(|(side, other)| {
        side.iter().any(|run| {
            let run = &runs[*run];
            goes_on(side, run)
                && other.iter().any(|beside| {
                    let beside = &runs[*beside];
                    same_line(run, beside) && !goes_on(other, beside)
                })
        })
    })
}

/// Splits `region` at its widest gap across the page, between runs on
/// different lines, leaving the top in `region` and returning the rest.
fn split_across(runs: &[Run], region: &mut Vec<usize>) -> Option<Vec<usize>> {
    region.sort_by(|first, second| runs[*first].rect.y0.total_cmp(&runs[*second].rect.y0));
    let mut widest: Option<(usize, f64)> = None;
    let mut bottom = f64::NEG_INFINITY;
    // The smallest text on the line so far.
    let mut size = f64::INFINITY;
    for (index, run) in region.iter().enumerate() {
        let run = &runs[*run];
        let gap = run.rect.y0 - bottom;
        if index > 0 && -gap <= LINE_OVERLAP * size.min(run.size) {
            if widest.is_none_or(|(_, widest)| gap > widest) {
                widest = Some((index, gap));
            }
            size = run.size;
        } else {
            size = size.min(run.size);
        }
        bottom = bottom.max(run.rect.y1);
    }
    let (index, _) = widest?;
    Some(region.split_off(index))
}

/// The page's links to addresses, with the rectangles they cover, in the
/// page's coordinates after `transform`.
fn links(page: &Page<'_>, transform: Affine) -> Vec<(String, Rect)> {
    let Some(annotations) = page.raw().get::<Array<'_>>(b"Annots") else {
        return Vec::new();
    };
    annotations
        .iter::<Dict<'_>>()
        .filter_map(|annotation| {
            let rect = annotation.get::<PdfRect>(b"Rect")?.to_kurbo();
            let address = annotation
                .get::<Dict<'_>>(b"A")?
                .get::<PdfString<'_>>(b"URI")?;
            let address = String::from_utf8_lossy(address.as_bytes())
                .trim()
                .to_owned();
            (!address.is_empty()).then(|| (address, transform.transform_rect_bbox(rect)))
        })
        .collect()
}

/// The ActualText of each marked-content sequence the page's content
/// begins, in order, for `Drawn` to match with those hayro begins. Those in
/// a form a page draws aren't listed, so the ones after it pair wrongly, but
/// only glyphs without text of their own take them.
fn actual_texts(page: &Page<'_>) -> Vec<Option<String>> {
    let mut operations = page.typed_operations();
    let mut texts = Vec::new();
    // A lending iterator, not an `Iterator`.
    while let Some(operation) = operations.next() {
        match operation {
            TypedInstruction::BeginMarkedContent(_) => texts.push(None),
            TypedInstruction::BeginMarkedContentWithProperties(begin) => texts.push(
                dict_or_stream(begin.1)
                    .and_then(|(properties, _)| properties.get::<PdfString<'_>>(b"ActualText"))
                    .map(|text| text_string(text.as_bytes())),
            ),
            _ => {}
        }
    }
    texts
}

/// A PDF text string: UTF-16 after a byte order mark, or else one byte a
/// character, read as Latin-1, which PDFDocEncoding matches but for a few
/// symbols.
fn text_string(bytes: &[u8]) -> String {
    match bytes.strip_prefix(&[0xFE, 0xFF]) {
        Some(utf16) => String::from_utf16_lossy(
            &utf16
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        ),
        None => bytes.iter().map(|&byte| char::from(byte)).collect(),
    }
}

/// Whether `character` is an icon: icon fonts put their glyphs in the
/// Private Use Areas, and an unknown glyph reads as the replacement mark.
fn icon(character: char) -> bool {
    matches!(character, '\u{E000}'..='\u{F8FF}' | '\u{F0000}'.. | '\u{FFFD}')
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATEX: &[u8] = include_bytes!("pdf/latex.pdf");
    const SIDEBAR: &[u8] = include_bytes!("pdf/sidebar.pdf");
    const WRITER: &[u8] = include_bytes!("pdf/writer.pdf");

    fn reading(bytes: &[u8]) -> Reading {
        read(bytes.to_vec()).expect("a test PDF that reads")
    }

    fn lines(reading: &Reading) -> Vec<&str> {
        reading
            .lines
            .iter()
            .map(|line| &reading.text[line.span.clone()])
            .collect()
    }

    /// A PDF whose pages each draw their content, with their annotations.
    /// They can draw in Helvetica as `/F1`, which the PDF names but doesn't
    /// embed; in a Type 3 font that draws blank "a" and "b" as `/F3`, and
    /// in one that draws them without saying what they are as `/F4`; and a
    /// grey image as `/Image`.
    fn pdf(pages: &[(&str, &str)]) -> Vec<u8> {
        let stream = |content: &str| {
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len() + 1
            )
        };
        let kids: Vec<String> = (0..pages.len())
            .map(|page| format!("{} 0 R", 9 + 2 * page))
            .collect();
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                kids.join(" "),
                pages.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_owned(),
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray \
             /BitsPerComponent 8 /Length 1 >>\nstream\n0\nendstream"
                .to_owned(),
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 500 700] \
             /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 6 0 R /b 6 0 R >> \
             /Encoding << /Type /Encoding /Differences [97 /a /b] >> \
             /FirstChar 97 /LastChar 98 /Widths [500 500] /ToUnicode 7 0 R >>"
                .to_owned(),
            stream("500 0 d0"),
            stream(
                "/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
                 1 begincodespacerange <00> <FF> endcodespacerange \
                 1 beginbfrange <61> <62> <0061> endbfrange \
                 endcmap CMapName currentdict /CMap defineresource pop end end",
            ),
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 500 700] \
             /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /g1 6 0 R /g2 6 0 R >> \
             /Encoding << /Type /Encoding /Differences [97 /g1 /g2] >> \
             /FirstChar 97 /LastChar 98 /Widths [500 500] >>"
                .to_owned(),
        ];
        for (page, (content, annotations)) in pages.iter().enumerate() {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R \
                 /Resources << /Font << /F1 3 0 R /F3 5 0 R /F4 8 0 R >> /XObject << /Image 4 0 R >> >> \
                 /Annots [{annotations}] >>",
                10 + 2 * page
            ));
            objects.push(stream(content));
        }
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (number, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend(format!("{} 0 obj\n{object}\nendobj\n", number + 1).bytes());
        }
        let table = bytes.len();
        bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            bytes.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        bytes.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n",
                objects.len() + 1
            )
            .bytes(),
        );
        bytes
    }

    #[test]
    fn words_and_lines_come_out_as_written() {
        let latex = reading(LATEX);
        let lines = lines(&latex);
        assert_eq!(lines[0], "Alex Rahman");
        assert_eq!(
            lines[1],
            "+880 1711 555 210 — alex.rahman@example.com — alexrahman — alexr — Dhaka, Bangladesh",
            "the icons are left out"
        );
        assert!(lines.contains(&"– Cut p99 latency of the routing API from 480 ms to 95 ms."));
        assert!(
            latex
                .text
                .contains("without a single customer-facing\noutage during the move.\n– Cut p99"),
            "a wrapped line stays as the PDF wraps it"
        );
    }

    #[test]
    fn a_date_beside_its_employer_stays_on_its_line() {
        let latex = reading(LATEX);
        let latex = lines(&latex);
        for line in [
            "Northwind Logistics 2021 – present",
            "Brightline Health 2018 – 2021",
            "Backend Engineer Remote",
            "University of Dhaka 2014 – 2018",
        ] {
            assert!(latex.contains(&line), "{line}");
        }
        let writer = reading(WRITER);
        assert!(lines(&writer).contains(&"Kite Analytics, Junior Developer 2016 – 2018"));
    }

    #[test]
    fn a_table_is_read_row_by_row() {
        let latex = reading(LATEX);
        assert!(
            lines(&latex)
                .contains(&"Languages Rust, Go, Python, SQL Tools Git, Grafana, Terraform")
        );
    }

    #[test]
    fn a_grid_whose_groups_wrap_is_read_column_by_column() {
        // Two columns of skill groups, a group on the left wrapping onto the
        // line beside the start of one on the right.
        let document = pdf(&[(
            "BT /F1 10 Tf 72 700 Td (Languages Go, Python) Tj ET\n\
             BT /F1 10 Tf 72 688 Td (Frontend React, Next.js, Tailwind CSS,) Tj ET\n\
             BT /F1 10 Tf 72 676 Td (TanStack Router, TanStack Query) Tj ET\n\
             BT /F1 10 Tf 72 664 Td (Backend Node.js, Hono) Tj ET\n\
             BT /F1 10 Tf 340 700 Td (Databases MongoDB, Redis) Tj ET\n\
             BT /F1 10 Tf 340 688 Td (Design Figma) Tj ET\n\
             BT /F1 10 Tf 340 676 Td (Testing Vitest, Jest) Tj ET\n\
             BT /F1 10 Tf 340 664 Td (DevOps Docker, AWS) Tj ET",
            "",
        )]);
        assert_eq!(
            lines(&reading(&document)),
            [
                "Languages Go, Python",
                "Frontend React, Next.js, Tailwind CSS,",
                "TanStack Router, TanStack Query",
                "Backend Node.js, Hono",
                "Databases MongoDB, Redis",
                "Design Figma",
                "Testing Vitest, Jest",
                "DevOps Docker, AWS",
            ]
        );
    }

    #[test]
    fn a_sidebar_is_read_before_the_main_column() {
        let sidebar = reading(SIDEBAR);
        let lines = lines(&sidebar);
        assert_eq!(
            lines[..12],
            [
                "Contact",
                "alex.rahman@example.com",
                "+880 1711 555 210 (Mobile)",
                "www.linkedin.com/in/alexrahman",
                "Top Skills",
                "Rust",
                "Kafka",
                "OpenTelemetry",
                "Languages",
                "Bangla (Native)",
                "English (Professional)",
                "Alex Rahman",
            ]
        );
        assert!(lines.contains(&"Northwind Logistics"));
    }

    #[test]
    fn blocks_are_apart_and_a_paragraph_holds_together() {
        let sidebar = reading(SIDEBAR);
        assert!(sidebar.text.contains("OpenTelemetry\n\nLanguages"));
        assert!(sidebar.text.contains("building\nreliable services"));
    }

    #[test]
    fn the_name_is_the_largest_text() {
        for bytes in [LATEX, SIDEBAR, WRITER] {
            let reading = reading(bytes);
            let largest = reading
                .lines
                .iter()
                .max_by(|first, second| first.size.total_cmp(&second.size))
                .map(|line| &reading.text[line.span.clone()]);
            assert_eq!(largest, Some("Alex Rahman"));
        }
    }

    #[test]
    fn icons_are_left_out() {
        for bytes in [LATEX, SIDEBAR, WRITER] {
            assert!(!reading(bytes).text.chars().any(icon));
        }
        let writer = reading(WRITER);
        assert!(
            lines(&writer)
                .contains(&"Led the migration of 12 backend services from Python to Rust."),
            "a bullet drawn from an icon font is left out"
        );
    }

    #[test]
    fn a_link_keeps_its_address_its_text_and_its_line() {
        let latex = reading(LATEX);
        let text = |span: &Option<Range<usize>>| span.clone().map(|span| &latex.text[span]);
        let line =
            |line: Option<usize>| line.map(|line| &latex.text[latex.lines[line].span.clone()]);
        let email = latex
            .links
            .iter()
            .find(|link| link.address == "mailto:alex.rahman@example.com")
            .expect("the email link");
        assert_eq!(text(&email.span), Some("alex.rahman@example.com"));
        let project: Vec<_> = latex
            .links
            .iter()
            .filter(|link| link.address.contains("tracewire"))
            .collect();
        assert_eq!(project.len(), 2);
        for link in project {
            assert_eq!(text(&link.span), None, "an icon covers no text");
            assert_eq!(
                line(link.line),
                Some("tracewire"),
                "it sits on the project's line"
            );
        }
    }

    #[test]
    fn a_link_belongs_to_the_line_it_covers_not_one_beside_it() {
        // Two columns, whose first lines happen to be at the same height.
        let document = pdf(&[(
            "BT /F1 10 Tf 72 700 Td (alex@example.com) Tj ET\n\
             BT /F1 10 Tf 72 680 Td (Rust) Tj ET\n\
             BT /F1 10 Tf 72 660 Td (Kafka) Tj ET\n\
             BT /F1 10 Tf 300 700 Td (Dhaka, Bangladesh) Tj ET\n\
             BT /F1 10 Tf 300 650 Td (Backend engineer) Tj ET\n\
             BT /F1 10 Tf 300 610 Td (Northwind Logistics) Tj ET",
            "<< /Type /Annot /Subtype /Link /Rect [70 697 160 709] \
             /A << /S /URI /URI (mailto:alex@example.com) >> >>",
        )]);
        let reading = reading(&document);
        assert_eq!(lines(&reading)[..3], ["alex@example.com", "Rust", "Kafka"]);
        let link = &reading.links[0];
        assert_eq!(
            link.line
                .map(|line| &reading.text[reading.lines[line].span.clone()]),
            Some("alex@example.com")
        );
    }

    #[test]
    fn a_font_the_pdf_does_not_embed_is_still_read() {
        let document = pdf(&[(
            "BT /F1 20 Tf 72 720 Td (Alex Rahman) Tj ET\n\
             BT /F1 10 Tf 72 700 Td (Led the migration of 12 services.) Tj ET",
            "",
        )]);
        let reading = reading(&document);
        assert_eq!(
            lines(&reading),
            ["Alex Rahman", "Led the migration of 12 services."]
        );
        assert_eq!(reading.lines[0].size, 20.);
    }

    #[test]
    fn a_pdf_without_text_or_that_isnt_one_is_an_error() {
        assert_eq!(read(pdf(&[("", "")])).err(), Some(Error::NoText));
        assert_eq!(read(b"not a PDF".to_vec()).err(), Some(Error::Invalid));
    }

    #[test]
    fn fake_bold_is_read_once() {
        // Fake bold draws the text again, a little to the side.
        let document = pdf(&[(
            "BT /F1 12 Tf 72 700 Td (Led the migration) Tj ET\n\
             BT /F1 12 Tf 72.5 700.2 Td (Led the migration) Tj ET\n\
             BT /F1 12 Tf 72 680 Td (Led the migration) Tj ET",
            "",
        )]);
        assert_eq!(
            lines(&reading(&document)),
            ["Led the migration", "Led the migration"],
            "the same text on another line is kept"
        );
    }

    #[test]
    fn a_page_of_only_pictures_is_warned_about() {
        let document = pdf(&[
            ("BT /F1 10 Tf 72 700 Td (Alex Rahman) Tj ET", ""),
            ("q 500 0 0 700 50 50 cm /Image Do Q", ""),
            ("", ""),
        ]);
        let reading = reading(&document);
        assert_eq!(lines(&reading), ["Alex Rahman"]);
        assert_eq!(
            reading.warnings,
            [Warning::NoText { page: 1 }],
            "a blank page is just blank"
        );
        for fixture in [LATEX, SIDEBAR, WRITER] {
            assert_eq!(
                read(fixture.to_vec()).map(|reading| reading.warnings),
                Ok(Vec::new()),
                "a PDF with text on every page warns about nothing"
            );
        }
    }

    #[test]
    fn a_font_drawn_as_pictures_is_read_as_words() {
        // Chrome prints the variable font Inter as Type 3 fonts.
        let junior = reading(include_bytes!("../model/extraction/junior.pdf"));
        assert_eq!(
            junior.warnings,
            [],
            "every letter is read, so nothing is warned about"
        );
        let lines = lines(&junior);
        assert_eq!(lines.first(), Some(&"Tasnim Chowdhury"));
        assert!(
            lines.contains(&"Paired with the backend team to move the fare estimate API from polling to WebSockets."),
            "{lines:#?}"
        );
        // Inter's dash between digits has no code of its own: Chrome gives
        // it one from the Private Use Area and its text as ActualText.
        assert!(
            lines
                .iter()
                .any(|line| line.ends_with("Jun 2024 – Present")),
            "{lines:#?}"
        );
    }

    #[test]
    fn letter_spaced_headings_and_ligatures_are_read_as_words() {
        let columns = reading(include_bytes!("../model/extraction/columns.pdf"));
        let lines = lines(&columns);
        for line in ["CONTACT", "SKILLS", "LANGUAGES", "performance profiling"] {
            assert!(lines.contains(&line), "{line}: {lines:#?}");
        }
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("Shipped offline-first transfers")),
            "{lines:#?}"
        );
    }

    #[test]
    fn a_font_drawn_as_pictures_is_warned_about_only_when_letters_are_lost() {
        let document = pdf(&[
            ("BT /F1 10 Tf 72 700 Td (Alex) Tj ET", ""),
            ("BT /F3 10 Tf 72 700 Td (ab) Tj ET", ""),
            ("BT /F4 10 Tf 72 700 Td (ab) Tj ET", ""),
        ]);
        assert_eq!(
            reading(&document).warnings,
            [Warning::DrawnFont { page: 2 }]
        );
    }
}
