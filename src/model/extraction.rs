// Finds a document's facts with a model. The model is sent the text with the
// user's name and contact details hidden (`redaction`) and quotes each fact;
// each quote is looked up in the text sent, never trusting the model to say
// where it is, so a fact says what the document says and cites where. A
// quote that isn't there word for word is kept and marked unsure: the model
// may have read it right and copied it wrong. Where the document breaks a
// word across lines, the fact is the word joined, marked for a look. The
// model may also fix a slip in how the text reads, like a word split in two,
// next to the quote; a fix that changes what the text says is ignored.

mod fixes;

use std::ops::Range;

use rig_core::completion::CompletionRequest;
use rig_core::completion::options::{GenerationOptions, Reasoning};
use rusqlite::Connection;
use serde::Deserialize;
use uuid::Uuid;

use super::commands::custom_section;
use super::redaction::{Identifier, Redaction};
use super::{DocumentId, EntryKind, FactKind, Review, ReviewReason, possible_duplicate};
use crate::database::schema::{Entries, FactSources, Facts, entries, fact_sources, facts};
use crate::database::{Error, Write};

/// What the model is told. The answer's shape is spelled out, since DeepSeek
/// takes "a JSON object" rather than a schema.
const PREAMBLE: &str = r#"You read a CV, or other text about a person's work, and list what it says as exact quotes.

Answer with one JSON object. Each Quote in it is {"quote": string, "fixed": string or null}, as the rules say.
{
  "summary": a Quote of the profile or summary paragraph, or null,
  "contacts": the placeholders, like "[EMAIL_1]", that are this person's own email, phone, links and profiles,
  "entries": [
    {
      "kind": "experience", "education", "project", "certification", "skills" or "other",
      "section": for "other", the heading it is under, as written; otherwise null,
      "title": a Quote of the job title, degree, project or certificate name, or a skills line's label (like "Languages"), or null,
      "organization": a Quote of the employer, school or issuer, or null,
      "location": a Quote, or null,
      "dates": a Quote of them as written, like "Mar 2021 – Present", without a duration the text adds, like "(2 years 3 months)", or null,
      "bullets": [a Quote of each bullet point about it, or each sentence of a paragraph about it],
      "skills": [a Quote of each skill or technology listed for it]
    }
  ]
}

Rules:
- A Quote's "quote" is copied exactly as the text has it: the same words, spelling, case and punctuation, slips and all. Never rephrase, shorten, or join text from different places.
- A Quote's "fixed" is null, unless the text has a slip in how it was read or typed, like a word split in two ("Work ing"), a missing dash between dates ("2021 2023") or a misspelt word: then it's the quote as it should read. Fix only such slips: never change a number, or which words are there.
- Leave out bullet markers (•, -, *) at the start of a point.
- A bullet point is one quote, even when it wraps onto the next line: quote all of it, with the line break as a space.
- Placeholders in square brackets, like [EMAIL_1], stand for hidden details. Copy them as they are. Never guess what they hide.
- Each job, degree, project and certification is its own entry, in the order the text has them. Two roles at one employer are two entries, each with the employer as its organization, even when the text names it once above both.
- A skills line like "Languages: Go, Java" is an entry of kind "skills" with the title "Languages" and the skills "Go" and "Java".
- Skills or technologies named with a project or job go in that entry's skills.
- A paragraph about a job, degree or project is split into its sentences, each one bullet. The summary stays whole.
- A list of spoken languages under a heading, like the skills, is a skills entry too, titled as the heading. A sentence about them is a bullet.
- Talks, awards, publications, volunteering and anything else go in entries of kind "other", each item a bullet.
- Leave out the person's name, section headings and the placeholders themselves.
- Use null or [] for what the text doesn't give. Never add anything the text doesn't say."#;
/// Room for a long CV's facts as JSON. Ours: a two-page CV is about 4,000
/// tokens of answer.
const MAX_TOKENS: u64 = 16_000;
/// Marks a point starts with, which quotes leave out.
const BULLETS: [char; 8] = ['•', '-', '*', '–', '▪', '◦', '·', '‣'];

/// The request asking the model for the facts in `redacted`. It's asked not
/// to reason: copying quotes needs none, and it's slower and costs more.
pub fn request(redacted: &str) -> CompletionRequest {
    CompletionRequest::new(redacted.to_owned())
        .preamble(PREAMBLE)
        .temperature(0.)
        .max_tokens(MAX_TOKENS)
        .options(GenerationOptions::new().reasoning(Reasoning::Off))
        .additional_params(serde_json::json!({ "response_format": { "type": "json_object" } }))
}

/// The model's answer, as the preamble describes it.
#[derive(Debug, Default, Deserialize)]
pub struct Answer {
    #[serde(default)]
    summary: Option<Quoted>,
    #[serde(default)]
    contacts: Vec<String>,
    #[serde(default)]
    entries: Vec<AnswerEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct AnswerEntry {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    section: Option<String>,
    #[serde(default)]
    title: Option<Quoted>,
    #[serde(default)]
    organization: Option<Quoted>,
    #[serde(default)]
    location: Option<Quoted>,
    #[serde(default)]
    dates: Option<Quoted>,
    #[serde(default)]
    bullets: Vec<Quoted>,
    #[serde(default)]
    skills: Vec<Quoted>,
}

/// A quote, and how it should read if the model fixed a slip in it.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Quoted {
    Exact(String),
    Fixed {
        quote: String,
        #[serde(default)]
        fixed: Option<String>,
    },
}

impl Quoted {
    fn quote(&self) -> &str {
        match self {
            Quoted::Exact(quote) | Quoted::Fixed { quote, .. } => quote,
        }
    }

    fn fixed(&self) -> Option<&str> {
        match self {
            Quoted::Exact(_) => None,
            Quoted::Fixed { fixed, .. } => fixed.as_deref(),
        }
    }
}

impl Answer {
    /// Reads the model's answer, even wrapped in a Markdown code block.
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        let json = match (text.find('{'), text.rfind('}')) {
            (Some(start), Some(end)) if start < end => &text[start..=end],
            _ => text,
        };
        serde_json::from_str(json)
    }
}

/// The facts found, ready to save.
#[derive(Debug, Default, PartialEq)]
pub struct Extraction {
    /// Facts about the person, in no entry: the summary and contacts.
    pub profile: Vec<FoundFact>,
    pub entries: Vec<FoundEntry>,
}

#[derive(Debug, PartialEq)]
pub struct FoundEntry {
    pub kind: EntryKind,
    /// The heading an entry of kind `Other` is under.
    pub section: Option<String>,
    pub facts: Vec<FoundFact>,
}

#[derive(Debug, PartialEq)]
pub struct FoundFact {
    pub kind: FactKind,
    pub text: String,
    /// Where it is in the original text, and what's there; none when the
    /// quote wasn't found.
    pub source: Option<(Range<usize>, String)>,
    pub review: Option<Review>,
}

impl Extraction {
    /// Looks up each of the answer's quotes in `redaction`'s text, and takes
    /// the facts from `original`, the text it was made from.
    pub fn locate(answer: Answer, original: &str, redaction: &Redaction) -> Self {
        let mut quotes = Quotes::new(&redaction.text);
        let mut fact = |kind: FactKind, quoted: &Quoted| -> Option<FoundFact> {
            let trim = |quote: &'_ str| quote.trim().trim_start_matches(BULLETS).trim().to_owned();
            let quote = trim(quoted.quote());
            if quote.is_empty() {
                return None;
            }
            // Found as quoted, or else as the model fixed it without saying,
            // when the fix is one it may make.
            let found = quotes
                .find(&quote)
                .map(|(redacted, joined)| (redacted, Some(joined)))
                .or_else(|| {
                    let redacted = fixes::nearest(&redaction.text, &quote, quotes.cursor)?;
                    fixes::acceptable(&redaction.text[redacted.clone()], &quote).then(|| {
                        quotes.cursor = redacted.end;
                        (redacted, None)
                    })
                });
            let Some((redacted, joined)) = found else {
                return Some(FoundFact {
                    kind,
                    text: redaction.restore(&quote, original),
                    source: None,
                    review: Some(Review {
                        reason: ReviewReason::Unsure,
                        note: Some(
                            "This isn't in the document word for word. Check it says what the \
                             document does."
                                .to_owned(),
                        ),
                    }),
                });
            };
            let sent = &redaction.text[redacted.clone()];
            let span = redaction.original(redacted);
            let written = &original[span.clone()];
            let cleaned = clean(written, joined.unwrap_or(false));
            let fixed = quoted
                .fixed()
                .map(trim)
                .into_iter()
                .chain(joined.is_none().then(|| quote.clone()))
                .find(|fixed| fixes::acceptable(sent, fixed))
                .map(|fixed| redaction.restore(&fixed, original))
                .filter(|fixed| *fixed != cleaned);
            let source = Some((span, written.to_owned()));
            if let Some(fixed) = fixed {
                return Some(FoundFact {
                    kind,
                    text: fixed,
                    source,
                    review: Some(Review {
                        reason: ReviewReason::Cleaned,
                        note: Some(format!(
                            "Fixed from the document's \u{201c}{}\u{201d}. Check it reads right.",
                            clean(written, false)
                        )),
                    }),
                });
            }
            let broken = written
                .match_indices('-')
                .any(|(index, _)| broken_word(&written[index..]).is_some());
            Some(FoundFact {
                kind,
                text: cleaned,
                source,
                review: broken.then(|| Review {
                    reason: ReviewReason::Cleaned,
                    note: Some(
                        "A word was split across two lines in the document. Check it's \
                         joined right."
                            .to_owned(),
                    ),
                }),
            })
        };

        let mut extraction = Extraction::default();
        extraction.profile.extend(
            answer
                .summary
                .as_ref()
                .and_then(|summary| fact(FactKind::Summary, summary)),
        );
        for entry in answer.entries {
            let kind = match entry.kind.as_str() {
                "experience" => EntryKind::Experience,
                "education" => EntryKind::Education,
                "project" => EntryKind::Project,
                "certification" => EntryKind::Certification,
                "skills" => EntryKind::Skills,
                _ => EntryKind::Other,
            };
            let title = if kind == EntryKind::Skills {
                FactKind::SkillGroup
            } else {
                FactKind::Title
            };
            let fields = [
                (title, entry.title),
                (FactKind::Organization, entry.organization),
                (FactKind::Location, entry.location),
                (FactKind::Date, entry.dates),
            ];
            let mut facts: Vec<FoundFact> = fields
                .into_iter()
                .filter_map(|(kind, quote)| fact(kind, quote.as_ref()?))
                .collect();
            facts.extend(
                entry
                    .skills
                    .iter()
                    .filter_map(|skill| fact(FactKind::Skill, skill)),
            );
            facts.extend(
                entry
                    .bullets
                    .iter()
                    .filter_map(|bullet| fact(FactKind::Bullet, bullet)),
            );
            if !facts.is_empty() {
                extraction.entries.push(FoundEntry {
                    kind,
                    section: entry
                        .section
                        .filter(|_| kind == EntryKind::Other)
                        .map(|section| section.trim().to_owned())
                        .filter(|section| !section.is_empty()),
                    facts,
                });
            }
        }
        // Contacts are only ever placeholders: their text never left.
        for placeholder in &answer.contacts {
            let replacement = redaction.replacements.iter().find(|replacement| {
                replacement.placeholder == placeholder.trim()
                    && replacement.identifier != Identifier::Name
            });
            if let Some(replacement) = replacement {
                let written = &original[replacement.original.clone()];
                // A link wrapped onto the next line has no space in it.
                let text = match replacement.identifier {
                    Identifier::Link => written.lines().map(str::trim).collect(),
                    _ => clean(written, false),
                };
                if !extraction.profile.iter().any(|fact| fact.text == text) {
                    extraction.profile.push(FoundFact {
                        kind: FactKind::Contact,
                        text,
                        source: Some((replacement.original.clone(), written.to_owned())),
                        review: None,
                    });
                }
            }
        }
        extraction
    }
}

/// Saves what was found in `document`: its entries, their facts and where
/// each was found. A fact already there, worded the same in the same entry,
/// gains this source instead of a copy, and an entry already there (the
/// same title, organization and start) gains the facts it lacks.
pub fn save(
    write: &mut Write,
    document: DocumentId,
    extraction: &Extraction,
) -> Result<Saved, Error> {
    let mut saved = Saved::default();
    let mut profile = known(write, None)?;
    for fact in &extraction.profile {
        save_fact(write, document, None, &mut profile, fact, &mut saved)?;
    }
    for entry in &extraction.entries {
        let section = match entry.kind {
            EntryKind::Other => Some(custom_section(
                write,
                entry
                    .section
                    .as_deref()
                    .unwrap_or(super::commands::OTHER_SECTION),
            )?),
            _ => None,
        };
        let (id, mut facts) = match same_entry(write, entry, section)? {
            Some(id) => (id, known(write, Some(id))?),
            None => {
                let id = Uuid::now_v7();
                write.insert(
                    Entries,
                    id,
                    &[
                        entries::KIND.value(&entry.kind.stored()),
                        entries::CUSTOM_SECTION_ID.value(&section),
                    ],
                )?;
                (id, Vec::new())
            }
        };
        for fact in &entry.facts {
            save_fact(write, document, Some(id), &mut facts, fact, &mut saved)?;
        }
    }
    Ok(saved)
}

/// The documents whose facts haven't been found, as saved with no AI set
/// up, oldest first, with their file names: none for text pasted or written.
/// Only the user's name confirmed while starting out may cite one, and an
/// extraction that found nothing still counts as done.
pub fn unread(connection: &Connection) -> Result<Vec<(DocumentId, Option<String>)>, Error> {
    Ok(connection
        .prepare_cached(
            "SELECT documents.id, documents.file_name FROM documents
             WHERE documents.deleted_at IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM fact_sources
                   JOIN facts ON facts.id = fact_sources.fact_id
                   WHERE fact_sources.document_id = documents.id AND facts.kind != 'name')
               AND NOT EXISTS (
                   SELECT 1 FROM llm_runs
                   WHERE llm_runs.document_id = documents.id
                     AND llm_runs.task = 'extract_facts' AND llm_runs.outcome = 'done')
             ORDER BY documents.added_at, documents.id",
        )?
        .query_map([], |row| Ok((DocumentId(row.get(0)?), row.get(1)?)))?
        .collect::<Result<_, _>>()?)
}

/// What an extraction's `save` did.
#[derive(Debug, Default, PartialEq)]
pub struct Saved {
    pub added: usize,
    /// Facts found that were there already.
    pub there: usize,
    /// Of those added, how many are marked for a look.
    pub to_review: usize,
}

/// A fact already saved, to tell a fact found again from a new one.
struct Known {
    /// The fact a source of it goes to: itself, or the one it was merged into.
    root: Uuid,
    kind: String,
    text: String,
    /// Its text and as extracted, `normalized`.
    words: [String; 2],
    /// Neither merged nor saved by this import, so a new fact that differs
    /// from it is worth a look.
    settled: bool,
}

/// The facts of `entry`, or with none the facts in no entry.
fn known(write: &Write, entry: Option<Uuid>) -> Result<Vec<Known>, Error> {
    let rows: Vec<(Uuid, String, String, String, Option<Uuid>)> = write
        .prepare(
            "SELECT id, kind, text, extracted, merged_into FROM facts
             WHERE entry_id IS ?1 AND deleted_at IS NULL ORDER BY id",
        )?
        .query_map([entry], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    let mut facts = Vec::with_capacity(rows.len());
    for (id, kind, text, extracted, merged_into) in rows {
        let mut root = id;
        let mut next = merged_into;
        // Merges chain when a fact kept is merged later; a cycle can't be
        // written, but the bound keeps a corrupt one from hanging the import.
        for _ in 0..64 {
            let Some(into) = next else { break };
            root = into;
            next = write
                .prepare("SELECT merged_into FROM facts WHERE id = ?1")?
                .query_row([into], |row| row.get(0))?;
        }
        facts.push(Known {
            root,
            words: [normalized(&text), normalized(&extracted)],
            kind,
            text,
            settled: merged_into.is_none(),
        });
    }
    Ok(facts)
}

/// An entry already saved that `entry` is about: the same kind and section,
/// title and organization, and start date when both have dates.
fn same_entry(
    write: &Write,
    entry: &FoundEntry,
    section: Option<Uuid>,
) -> Result<Option<Uuid>, Error> {
    let field = |kinds: &[FactKind]| {
        entry
            .facts
            .iter()
            .find(|fact| kinds.contains(&fact.kind))
            .map(|fact| fact.text.as_str())
    };
    let title = field(&[FactKind::Title, FactKind::SkillGroup]).map(normalized);
    let organization = field(&[FactKind::Organization]).map(normalized);
    let began = field(&[FactKind::Date]).map(start);
    let grouped = matches!(entry.kind, EntryKind::Skills | EntryKind::Other);
    if title.is_none() && organization.is_none() && !grouped {
        return Ok(None);
    }

    let mut statement = write.prepare(
        "SELECT entries.id, facts.kind, facts.text, facts.extracted FROM entries
         LEFT JOIN facts ON facts.entry_id = entries.id AND facts.deleted_at IS NULL
             AND facts.kind IN ('title', 'skill_group', 'organization', 'date')
         WHERE entries.kind = ?1 AND entries.custom_section_id IS ?2
             AND entries.deleted_at IS NULL
         ORDER BY entries.id",
    )?;
    let mut rows = statement.query((entry.kind.stored(), section))?;
    // Each entry and its fields' values, as written and as extracted: dates
    // by their start.
    let mut candidates: Vec<(Uuid, Vec<(String, String)>)> = Vec::new();
    while let Some(row) = rows.next()? {
        let id: Uuid = row.get(0)?;
        if candidates.last().is_none_or(|(last, _)| *last != id) {
            candidates.push((id, Vec::new()));
        }
        let kind: Option<String> = row.get(1)?;
        if let (Some(kind), Some((_, fields))) = (kind, candidates.last_mut()) {
            let value = if kind == "date" { start } else { normalized };
            fields.push((kind.clone(), value(&row.get::<_, String>(2)?)));
            fields.push((kind, value(&row.get::<_, String>(3)?)));
        }
    }
    Ok(candidates
        .into_iter()
        .find(|(_, fields)| {
            let values = |kinds: &[&str]| {
                fields
                    .iter()
                    .filter(|(kind, _)| kinds.contains(&kind.as_str()))
                    .map(|(_, value)| value.as_str())
                    .collect::<Vec<_>>()
            };
            let agrees = |found: &Option<String>, kinds: &[&str]| {
                let values = values(kinds);
                match found {
                    Some(found) => values.contains(&found.as_str()),
                    None => values.is_empty(),
                }
            };
            let dates = values(&["date"]);
            agrees(&title, &["title", "skill_group"])
                && agrees(&organization, &["organization"])
                && match &began {
                    Some(found) if !dates.is_empty() => dates.contains(&found.as_str()),
                    _ => true,
                }
        })
        .map(|(id, _)| id))
}

/// Saves `fact` in `entry`, unless it's among `known` already, where it
/// gains this source instead.
fn save_fact(
    write: &mut Write,
    document: DocumentId,
    entry: Option<Uuid>,
    known: &mut Vec<Known>,
    fact: &FoundFact,
    saved: &mut Saved,
) -> Result<(), Error> {
    let kind = fact.kind.stored();
    let words = normalized(&fact.text);
    let there = known
        .iter()
        .find(|known| known.kind == kind && known.words.contains(&words))
        .map(|known| known.root);
    if let Some(root) = there {
        if let Some(source) = &fact.source {
            save_source(write, document, root, source)?;
        }
        saved.there += 1;
        return Ok(());
    }

    let review = fact.review.clone().or_else(|| repeated(fact, known));
    let id = Uuid::now_v7();
    write.insert(
        Facts,
        id,
        &[
            facts::KIND.value(&kind),
            facts::TEXT.value(&fact.text),
            facts::EXTRACTED.value(&fact.text),
            facts::ENTRY_ID.value(&entry),
            facts::REVIEW.value(&review.as_ref().map(|review| review.reason.stored())),
            facts::REVIEW_NOTE.value(&review.as_ref().and_then(|review| review.note.as_deref())),
        ],
    )?;
    if let Some(source) = &fact.source {
        save_source(write, document, id, source)?;
    }
    known.push(Known {
        root: id,
        kind: kind.to_owned(),
        text: fact.text.clone(),
        words: [words.clone(), words],
        settled: false,
    });
    saved.added += 1;
    saved.to_review += usize::from(review.is_some());
    Ok(())
}

/// Why a new fact may repeat one already there: an entry's title, dates or
/// such differing from what it had, or a bullet or summary saying much the
/// same in other words.
fn repeated(fact: &FoundFact, known: &[Known]) -> Option<Review> {
    let kind = fact.kind.stored();
    let mut settled = known
        .iter()
        .filter(|known| known.settled && known.kind == kind);
    match fact.kind {
        FactKind::Title
        | FactKind::Organization
        | FactKind::Date
        | FactKind::Location
        | FactKind::SkillGroup => settled.next().map(|known| Review {
            reason: ReviewReason::Duplicate,
            note: Some(format!(
                "This entry already had \u{201c}{}\u{201d}. Keep the one that's right.",
                known.text
            )),
        }),
        FactKind::Bullet | FactKind::Summary => {
            let repeats = possible_duplicate(&fact.text);
            settled.any(|known| repeats(&known.text)).then_some(Review {
                reason: ReviewReason::Duplicate,
                note: None,
            })
        }
        FactKind::Name | FactKind::Skill | FactKind::Contact => None,
    }
}

fn save_source(
    write: &mut Write,
    document: DocumentId,
    fact: Uuid,
    (span, written): &(Range<usize>, String),
) -> Result<(), Error> {
    write.insert(
        FactSources,
        Uuid::now_v7(),
        &[
            fact_sources::FACT_ID.value(&fact),
            fact_sources::DOCUMENT_ID.value(&document.0),
            fact_sources::SPAN_START.value(&(span.start as i64)),
            fact_sources::SPAN_END.value(&(span.end as i64)),
            fact_sources::ORIGINAL_TEXT.value(written),
        ],
    )
}

/// Text as compared to tell the same fact: its letters and digits, in
/// lowercase. Spacing and punctuation differ with how a document was read
/// and set, but a different word or number is a different fact.
fn normalized(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A date's start, `normalized`: "mar2021" in "Mar 2021 – Present", so a
/// job that has ended since is the same job.
fn start(date: &str) -> String {
    let date = date.to_lowercase();
    let start = date
        .split([
            '-', '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}', '\u{2212}',
        ])
        .next()
        .unwrap_or(&date);
    normalized(start.split(" to ").next().unwrap_or(start))
}

/// Finds quotes in a text: as written, or else as the model may have
/// copied it, with line breaks as spaces, straight quotes and hyphens for
/// typographic ones, and a word split across lines whole. Each is found
/// after the last, where it can be, so the same words twice cite the right
/// place.
struct Quotes<'a> {
    text: &'a str,
    /// Line breaks as spaces, a word split across lines kept hyphenated.
    spaced: Normalized,
    /// The same, with a word split across lines joined.
    joined: Normalized,
    /// Where the last quote found ends.
    cursor: usize,
}

impl<'a> Quotes<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            spaced: Normalized::new(text, false),
            joined: Normalized::new(text, true),
            cursor: 0,
        }
    }

    /// Where `quote` is in the text, and whether a word split across lines
    /// had to be joined to find it.
    fn find(&mut self, quote: &str) -> Option<(Range<usize>, bool)> {
        let found = whole_words(self.text, quote, self.cursor)
            .map(|range| (range, false))
            .or_else(|| {
                let quote = Normalized::new(quote, false).text;
                [(&self.spaced, false), (&self.joined, true)]
                    .into_iter()
                    .find_map(|(normalized, joined)| {
                        let from = normalized.position_of(self.cursor);
                        let range = whole_words(&normalized.text, &quote, from)?;
                        Some((normalized.original(range), joined))
                    })
            })?;
        self.cursor = found.0.end;
        Some(found)
    }
}

/// A text with whitespace runs as one space and typographic punctuation
/// plain, knowing where each of its bytes came from.
struct Normalized {
    text: String,
    /// For each byte of `text`, the original character's bytes.
    origins: Vec<Range<usize>>,
}

impl Normalized {
    fn new(original: &str, join: bool) -> Self {
        let mut normalized = Self {
            text: String::with_capacity(original.len()),
            origins: Vec::with_capacity(original.len()),
        };
        let mut characters = original.char_indices().peekable();
        while let Some((start, character)) = characters.next() {
            let end = start + character.len_utf8();
            if character.is_whitespace() {
                while characters
                    .next_if(|(_, next)| next.is_whitespace())
                    .is_some()
                {}
                if !normalized.text.is_empty() {
                    normalized.push(' ', start..end);
                }
                continue;
            }
            if character == '-'
                && let Some(after) = broken_word(&original[start..])
            {
                if !join {
                    normalized.push('-', start..end);
                }
                // Skip the line break, to the next word's first letter.
                while characters
                    .next_if(|(index, _)| *index < start + after)
                    .is_some()
                {}
                continue;
            }
            let plain = match character {
                '‘' | '’' | 'ʼ' => '\'',
                '“' | '”' => '"',
                '–' | '—' | '‐' | '‑' => '-',
                other => other,
            };
            normalized.push(plain, start..end);
        }
        if normalized.text.ends_with(' ') {
            normalized.text.pop();
            normalized.origins.pop();
        }
        normalized
    }

    fn push(&mut self, character: char, origin: Range<usize>) {
        self.text.push(character);
        for _ in 0..character.len_utf8() {
            self.origins.push(origin.clone());
        }
    }

    /// The original bytes `range` of the normalized text came from.
    fn original(&self, range: Range<usize>) -> Range<usize> {
        let start = self
            .origins
            .get(range.start)
            .map_or(0, |origin| origin.start);
        let end = range
            .end
            .checked_sub(1)
            .and_then(|last| self.origins.get(last))
            .map_or(start, |origin| origin.end);
        start..end
    }

    /// The first byte of the normalized text from `original` on.
    fn position_of(&self, original: usize) -> usize {
        self.origins
            .partition_point(|origin| origin.start < original)
    }
}

/// Where `text` breaks a word across lines with a hyphen it starts with, as
/// "pipe-\nline": how far on the next word starts.
fn broken_word(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('-')?;
    let gap = rest.len() - rest.trim_start().len();
    let next = rest[gap..].chars().next()?;
    (rest[..gap].contains('\n') && next.is_lowercase()).then_some(1 + gap)
}

/// Where `quote` is in `text` as whole words, from `from` on or else the
/// nearest before.
fn whole_words(text: &str, quote: &str, from: usize) -> Option<Range<usize>> {
    let from = (0..=from.min(text.len()))
        .rev()
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(0);
    let bounded = |start: usize| {
        let end = start + quote.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        let edge = |outside: Option<char>, inside: Option<char>| {
            !(outside.is_some_and(char::is_alphanumeric)
                && inside.is_some_and(char::is_alphanumeric))
        };
        edge(before, quote.chars().next()) && edge(after, quote.chars().next_back())
    };
    let after = text[from..]
        .match_indices(quote)
        .map(|(start, _)| from + start)
        .find(|start| bounded(*start));
    // Starting before `from`, not ending before it: a skill found inside a
    // bullet leaves `from` in the middle of it.
    let before = || {
        text.match_indices(quote)
            .map(|(start, _)| start)
            .take_while(|start| *start < from)
            .filter(|start| bounded(*start))
            .last()
    };
    after
        .or_else(before)
        .map(|start| start..start + quote.len())
}

/// The fact's text from what the document has: on one line, with a word
/// split across lines joined, without its hyphen when `joined`.
fn clean(written: &str, joined: bool) -> String {
    let mut text = String::with_capacity(written.len());
    let mut rest = written;
    while let Some(character) = rest.chars().next() {
        if character == '-'
            && let Some(after) = broken_word(rest)
        {
            if !joined {
                text.push('-');
            }
            rest = &rest[after..];
            continue;
        }
        if character.is_whitespace() {
            if !text.ends_with(' ') {
                text.push(' ');
            }
        } else {
            text.push(character);
        }
        rest = &rest[character.len_utf8()..];
    }
    text.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Writer;
    use crate::model::redaction;

    const TEXT: &str = "Alex Rahman\n\
        alex@example.com · github.com/alexr · linkedin.com/in/alexrah\nman-dev\n\
        Backend engineer who likes quiet on-call weeks.\n\
        Senior Engineer, Northwind (Berlin) Mar 2021 – Present\n\
        • Designed the event pipe-\n\
        line for shipment updates.\n\
        • Cut p99 latency from 480 ms to\n95 ms.\n\
        Engineer, Northwind (Berlin) 2018 – 2021\n\
        • Built a real-\ntime tracker in Java.\n\
        • Wrote the team’s runbooks.\n\
        Languages: Java, JavaScript, Go\n\
        Databases: PostgreSQL, SQL\n\
        • Mentored two new engin eers.\n";

    fn redacted() -> Redaction {
        Redaction::new(
            TEXT,
            &redaction::find(TEXT, &["Alex Rahman"], &[]),
            "Jordan Ellis",
        )
    }

    fn answer(json: &str) -> Extraction {
        let answer = match Answer::parse(json) {
            Ok(answer) => answer,
            Err(error) => panic!("a test answer that parses: {error}"),
        };
        Extraction::locate(answer, TEXT, &redacted())
    }

    fn texts(entry: &FoundEntry) -> Vec<(FactKind, &str)> {
        entry
            .facts
            .iter()
            .map(|fact| (fact.kind, fact.text.as_str()))
            .collect()
    }

    #[test]
    fn quotes_are_found_in_the_text_and_cite_it() {
        let extraction = answer(
            r#"```json
            {"summary": "Backend engineer who likes quiet on-call weeks.",
             "contacts": ["[EMAIL_1]", "[LINK_1]", "[LINK_2]"],
             "entries": [
               {"kind": "experience", "title": "Senior Engineer", "organization": "Northwind",
                "location": "Berlin", "dates": "Mar 2021 - Present",
                "bullets": ["• Designed the event pipeline for shipment updates.",
                            "Cut p99 latency from 480 ms to 95 ms."]},
               {"kind": "experience", "title": "Engineer", "organization": "Northwind",
                "location": "Berlin", "dates": "2018 – 2021",
                "bullets": ["Built a real-time tracker in Java.", "Wrote the team's runbooks."]},
               {"kind": "skills", "title": "Languages", "skills": ["Java", "JavaScript", "Go"]},
               {"kind": "skills", "title": "Databases", "skills": ["SQL"]}
             ]}
            ```"#,
        );
        let profile: Vec<(FactKind, &str)> = extraction
            .profile
            .iter()
            .map(|fact| (fact.kind, fact.text.as_str()))
            .collect();
        assert_eq!(
            profile,
            [
                (
                    FactKind::Summary,
                    "Backend engineer who likes quiet on-call weeks."
                ),
                (FactKind::Contact, "alex@example.com"),
                (FactKind::Contact, "github.com/alexr"),
                (FactKind::Contact, "linkedin.com/in/alexrahman-dev"),
            ]
        );
        assert_eq!(
            texts(&extraction.entries[0]),
            [
                (FactKind::Title, "Senior Engineer"),
                (FactKind::Organization, "Northwind"),
                (FactKind::Location, "Berlin"),
                (FactKind::Date, "Mar 2021 – Present"),
                (
                    FactKind::Bullet,
                    "Designed the event pipeline for shipment updates."
                ),
                (FactKind::Bullet, "Cut p99 latency from 480 ms to 95 ms."),
            ],
            "dashes and line breaks as the document has them"
        );
        assert_eq!(
            texts(&extraction.entries[1])[4],
            (FactKind::Bullet, "Built a real-time tracker in Java."),
            "a hyphen the model kept stays"
        );
        assert_eq!(
            texts(&extraction.entries[1])[5],
            (FactKind::Bullet, "Wrote the team’s runbooks."),
            "a straight quote finds the document's curly one, which the fact keeps"
        );
        let sql = extraction.entries[3].facts[1]
            .source
            .as_ref()
            .map(|(span, _)| &TEXT[span.start - 2..span.end]);
        assert_eq!(sql, Some(", SQL"), "not the end of PostgreSQL");
        assert_eq!(
            texts(&extraction.entries[2]),
            [
                (FactKind::SkillGroup, "Languages"),
                (FactKind::Skill, "Java"),
                (FactKind::Skill, "JavaScript"),
                (FactKind::Skill, "Go"),
            ]
        );

        for fact in extraction
            .profile
            .iter()
            .chain(extraction.entries.iter().flat_map(|entry| &entry.facts))
        {
            let Some((span, written)) = &fact.source else {
                panic!("{} wasn't found", fact.text);
            };
            assert_eq!(&TEXT[span.clone()], written);
        }
        let organization = |entry: usize| {
            extraction.entries[entry].facts[1]
                .source
                .as_ref()
                .map(|(span, _)| span.start)
        };
        assert!(
            organization(1) > organization(0),
            "the second Northwind cites its own line"
        );
        let java = extraction.entries[2].facts[1]
            .source
            .as_ref()
            .map(|(span, _)| span.clone());
        assert_eq!(
            java.map(|span| &TEXT[span.start..span.end + 1]),
            Some("Java,"),
            "a skill is a whole word, not the start of JavaScript or the Java in a bullet before"
        );
    }

    #[test]
    fn a_word_split_across_lines_is_joined_and_marked_for_a_look() {
        let extraction = answer(
            r#"{"entries": [{"kind": "experience",
                "bullets": ["Designed the event pipeline for shipment updates.",
                            "Built a real-time tracker in Java.",
                            "Cut p99 latency from 480 ms to 95 ms."]}]}"#,
        );
        let reviews: Vec<Option<ReviewReason>> = extraction.entries[0]
            .facts
            .iter()
            .map(|fact| fact.review.as_ref().map(|review| review.reason))
            .collect();
        assert_eq!(
            reviews,
            [
                Some(ReviewReason::Cleaned),
                Some(ReviewReason::Cleaned),
                None
            ],
            "a line that just wraps needs no look"
        );
    }

    #[test]
    fn a_bullet_is_found_after_a_skill_found_inside_it() {
        let extraction = answer(
            r#"{"entries": [{"kind": "experience", "title": "Engineer",
                             "skills": ["Java"],
                             "bullets": ["Built a real-time tracker in Java."]}]}"#,
        );
        let facts = &extraction.entries[0].facts;
        assert!(
            facts.iter().all(|fact| fact.source.is_some()),
            "every quote is cited: {facts:?}"
        );
    }

    #[test]
    fn a_slip_the_model_fixes_is_kept_fixed_and_marked() {
        let extraction = answer(
            r#"{"entries": [{"kind": "experience", "bullets": [
                {"quote": "Mentored two new engin eers.", "fixed": "Mentored two new engineers."},
                {"quote": "Cut p99 latency from 480 ms to 95 ms.",
                 "fixed": "Cut p99 latency from 480 ms to 90 ms."},
                {"quote": "Wrote the team's runbooks.", "fixed": null}]}]}"#,
        );
        let facts = &extraction.entries[0].facts;
        assert_eq!(facts[0].text, "Mentored two new engineers.");
        assert_eq!(
            facts[0]
                .source
                .as_ref()
                .map(|(_, written)| written.as_str()),
            Some("Mentored two new engin eers.")
        );
        let note = facts[0]
            .review
            .as_ref()
            .and_then(|review| review.note.as_deref());
        assert!(
            note.is_some_and(|note| note.contains("\u{201c}Mentored two new engin eers.\u{201d}")),
            "{note:?}"
        );
        assert_eq!(
            (facts[1].text.as_str(), &facts[1].review),
            ("Cut p99 latency from 480 ms to 95 ms.", &None),
            "a fix that changes a number is ignored"
        );
        assert_eq!(
            (facts[2].text.as_str(), &facts[2].review),
            ("Wrote the team’s runbooks.", &None)
        );
    }

    #[test]
    fn a_quote_fixed_without_saying_is_found_and_cited() {
        let extraction = answer(
            r#"{"entries": [{"kind": "experience", "bullets": [
                "Mentored two new engineers.", "Mentored two new engineers and an intern."]}]}"#,
        );
        let facts = &extraction.entries[0].facts;
        assert_eq!(facts[0].text, "Mentored two new engineers.");
        assert_eq!(
            facts[0]
                .source
                .as_ref()
                .map(|(_, written)| written.as_str()),
            Some("Mentored two new engin eers.")
        );
        assert_eq!(
            facts[0].review.as_ref().map(|review| review.reason),
            Some(ReviewReason::Cleaned)
        );
        assert_eq!(
            (
                facts[1].source.as_ref(),
                facts[1].review.as_ref().map(|review| review.reason)
            ),
            (None, Some(ReviewReason::Unsure)),
            "words the text doesn't have aren't a fix"
        );
    }

    #[test]
    fn a_quote_not_in_the_text_is_kept_and_marked_unsure() {
        let extraction = answer(
            r#"{"summary": "Jordan Ellis is a backend engineer.",
                "contacts": ["Jordan Ellis", "[PHONE_9]"],
                "entries": [{"kind": "other", "section": " Talks ",
                             "bullets": ["Spoke at RustConf 2024."]}]}"#,
        );
        assert_eq!(
            extraction.profile.len(),
            1,
            "only real contacts are contacts"
        );
        let summary = &extraction.profile[0];
        assert_eq!(summary.text, "Alex Rahman is a backend engineer.");
        assert_eq!(summary.source, None);
        assert_eq!(
            summary.review.as_ref().map(|review| review.reason),
            Some(ReviewReason::Unsure)
        );
        assert_eq!(extraction.entries[0].section.as_deref(), Some("Talks"));
        assert!(
            extraction.entries[0].facts[0].review.is_some(),
            "a bullet as unsure as the summary"
        );
    }

    #[test]
    fn what_was_found_is_saved_with_its_sources() -> Result<(), Error> {
        let extraction = answer(
            r#"{"summary": "Backend engineer who likes quiet on-call weeks.",
                "entries": [
                  {"kind": "experience", "title": "Senior Engineer",
                   "bullets": ["Designed the event pipeline for shipment updates."]},
                  {"kind": "other", "section": "Talks", "bullets": ["Spoke at RustConf 2024."]}]}"#,
        );
        let mut writer = Writer::open_in_memory()?;
        writer.execute(|write| {
            let document = super::super::commands::import_document(write, None, TEXT)?;
            save(write, document, &extraction)
        })?;
        // One row per fact: its kind, text, entry's kind and section, and
        // the document's text it cites.
        let saved: Vec<String> = writer
            .kron()
            .prepare(
                "SELECT concat_ws(' | ', facts.kind, facts.text, entries.kind,
                                  custom_sections.name, fact_sources.original_text)
                 FROM facts
                 LEFT JOIN entries ON entries.id = facts.entry_id
                 LEFT JOIN custom_sections ON custom_sections.id = entries.custom_section_id
                 LEFT JOIN fact_sources ON fact_sources.fact_id = facts.id
                 ORDER BY facts.id",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        assert_eq!(
            saved,
            [
                "summary | Backend engineer who likes quiet on-call weeks. | \
                 Backend engineer who likes quiet on-call weeks.",
                "title | Senior Engineer | experience | Senior Engineer",
                "bullet | Designed the event pipeline for shipment updates. | experience | \
                 Designed the event pipe-\nline for shipment updates.",
                "bullet | Spoke at RustConf 2024. | custom | Talks",
            ]
        );
        Ok(())
    }

    #[test]
    fn a_document_is_unread_until_its_facts_are_found() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let (document, _) = writer.execute(|write| {
            let document = super::super::commands::import_document(write, None, TEXT)?;
            super::super::commands::add_name(write, "Alex Rahman", Some(document))?;
            Ok(document)
        })?;
        assert_eq!(
            unread(writer.kron())?,
            [(document, None)],
            "the name confirmed is no fact found"
        );
        let extraction =
            answer(r#"{"entries": [{"kind": "experience", "title": "Senior Engineer"}]}"#);
        writer.execute(|write| save(write, document, &extraction))?;
        assert_eq!(unread(writer.kron())?, []);
        Ok(())
    }

    fn found(kind: FactKind, text: &str) -> FoundFact {
        FoundFact {
            kind,
            text: text.to_owned(),
            source: Some((0..text.len(), text.to_owned())),
            review: None,
        }
    }

    fn job(dates: &str, bullets: &[&str]) -> FoundEntry {
        let mut facts = vec![
            found(FactKind::Title, "Senior Engineer"),
            found(FactKind::Organization, "Northwind"),
            found(FactKind::Date, dates),
        ];
        facts.extend(bullets.iter().map(|bullet| found(FactKind::Bullet, bullet)));
        FoundEntry {
            kind: EntryKind::Experience,
            section: None,
            facts,
        }
    }

    fn languages(skills: &[&str]) -> FoundEntry {
        let mut facts = vec![found(FactKind::SkillGroup, "Languages")];
        facts.extend(skills.iter().map(|skill| found(FactKind::Skill, skill)));
        FoundEntry {
            kind: EntryKind::Skills,
            section: None,
            facts,
        }
    }

    #[test]
    fn a_fact_found_again_gains_a_source_instead_of_a_copy() -> Result<(), Error> {
        let summary = "Backend engineer who likes quiet on-call weeks.";
        let pipeline = "Designed the event pipeline for shipment updates.";
        let first = Extraction {
            profile: vec![found(FactKind::Summary, summary)],
            entries: vec![
                job(
                    "Mar 2021 – Present",
                    &[pipeline, "Cut p99 latency from 480 ms to 95 ms."],
                ),
                languages(&["Java"]),
            ],
        };
        // The CV updated: the job has ended, a bullet is reworded and one
        // added, a skill learned. Another job with the same title there,
        // years before, stays its own entry.
        let second = Extraction {
            profile: vec![found(
                FactKind::Summary,
                "Backend engineer who likes quiet on call weeks",
            )],
            entries: vec![
                job(
                    "Mar 2021 - Jun 2024",
                    &[
                        "designed  the event pipeline for shipment updates",
                        "Cut p99 latency from 480 ms to 95 ms with a cache.",
                        "Mentored two engineers.",
                    ],
                ),
                job("Jan 2015 – Dec 2016", &[]),
                languages(&["Java", "Go"]),
            ],
        };

        let mut writer = Writer::open_in_memory()?;
        writer.execute(|write| {
            let document = super::super::commands::import_document(write, None, TEXT)?;
            save(write, document, &first)?;
            // The user merged the pipeline bullet into a note of theirs.
            let note =
                super::super::commands::add_note(write, "Built the shipment event pipeline.")?;
            let bullet: Uuid = write
                .prepare("SELECT id FROM facts WHERE text = ?1")?
                .query_row([pipeline], |row| row.get(0))?;
            super::super::commands::merge_facts(write, note[0], &[super::super::FactId(bullet)])
        })?;
        let (saved, _) = writer.execute(|write| {
            let document = super::super::commands::import_document(write, None, TEXT)?;
            save(write, document, &second)
        })?;
        assert_eq!(
            saved,
            Saved {
                added: 7,
                there: 6,
                to_review: 2
            }
        );

        // Each live fact: its kind, text, review, how many sources it has,
        // and which entry (by order) it's in.
        let facts: Vec<String> = writer
            .kron()
            .prepare(
                "SELECT concat_ws(' | ', facts.kind, facts.text, facts.review,
                    (SELECT count(*) FROM fact_sources WHERE fact_id = facts.id),
                    (SELECT count(*) FROM entries AS earlier
                     WHERE earlier.id <= facts.entry_id AND earlier.kind = entries.kind))
                 FROM facts LEFT JOIN entries ON entries.id = facts.entry_id
                 WHERE facts.merged_into IS NULL AND facts.kind != 'name'
                 ORDER BY facts.id",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        assert_eq!(
            facts,
            [
                "summary | Backend engineer who likes quiet on-call weeks. | 2 | 0",
                "title | Senior Engineer | 2 | 1",
                "organization | Northwind | 2 | 1",
                "date | Mar 2021 – Present | 1 | 1",
                "bullet | Cut p99 latency from 480 ms to 95 ms. | 1 | 1",
                "skill_group | Languages | 2 | 1",
                "skill | Java | 2 | 1",
                // Found again, the bullet merged into it is a second source.
                "bullet | Built the shipment event pipeline. | 2 | 1",
                "date | Mar 2021 - Jun 2024 | duplicate | 1 | 1",
                "bullet | Cut p99 latency from 480 ms to 95 ms with a cache. | duplicate | 1 | 1",
                "bullet | Mentored two engineers. | 1 | 1",
                "title | Senior Engineer | 1 | 2",
                "organization | Northwind | 1 | 2",
                "date | Jan 2015 – Dec 2016 | 1 | 2",
                "skill | Go | 1 | 1",
            ]
        );
        let note: String = writer.kron().query_row(
            "SELECT review_note FROM facts WHERE text = 'Mar 2021 - Jun 2024'",
            [],
            |row| row.get(0),
        )?;
        assert!(note.contains("Mar 2021 – Present"), "{note}");
        Ok(())
    }

    /// Sends the test CVs to the provider set up in the
    /// development data, with its key from the keyring, and scores the facts
    /// found against what each CV says (`*.facts`). It costs money and needs
    /// the network, so it runs only when asked:
    /// `cargo test live_extraction -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_extraction() -> Result<(), Box<dyn std::error::Error>> {
        let device = dirs::data_dir()
            .ok_or("no data directory")?
            .join("kron-dev/device.sqlite");
        let device = rusqlite::Connection::open_with_flags(
            device,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let provider = crate::llm::assigned(&device, crate::llm::Purpose::ExtractFacts)
            .map_err(|error| format!("{error:?}"))?
            .ok_or("no provider set up in the development data")?;
        let key = crate::llm::key(provider.id)?.ok_or("no key saved for the provider")?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let stand_in = crate::database::Writer::open_in_memory()
            .and_then(|writer| crate::model::settings::stand_in_name(writer.kron()))
            .map_err(|error| error.to_string())?;
        let mut total_cost = 0.;
        let mut recalls = Vec::new();
        for (name, pdf, expected) in [
            (
                "junior",
                &include_bytes!("extraction/junior.pdf")[..],
                include_str!("extraction/junior.facts"),
            ),
            (
                "senior",
                include_bytes!("extraction/senior.pdf"),
                include_str!("extraction/senior.facts"),
            ),
            (
                "columns",
                include_bytes!("extraction/columns.pdf"),
                include_str!("extraction/columns.facts"),
            ),
            (
                "linkedin",
                include_bytes!("extraction/linkedin.pdf"),
                include_str!("extraction/linkedin.facts"),
            ),
        ] {
            let reading = crate::import::read(crate::import::Format::Pdf, pdf.to_vec())
                .map_err(|error| format!("{error:?}"))?;
            let guessed = crate::import::name::guess(&reading);
            let names: Vec<&str> = guessed.as_deref().into_iter().collect();
            let redaction = Redaction::new(
                &reading.text,
                &redaction::find(&reading.text, &names, &[]),
                &stand_in,
            );
            let started = std::time::Instant::now();
            let reply = runtime
                .block_on(crate::llm::call(
                    &provider,
                    key.clone(),
                    request(&redaction.text),
                ))
                .map_err(|error| error.to_string())?;
            let elapsed = started.elapsed();
            let extraction = Extraction::locate(
                Answer::parse(&reply.response.text())?,
                &reading.text,
                &redaction,
            );
            total_cost += reply.cost.unwrap_or(0.);

            let plain = |text: &str| -> String {
                text.chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect()
            };
            let found: Vec<(FactKind, String, &FoundFact)> = extraction
                .profile
                .iter()
                .chain(extraction.entries.iter().flat_map(|entry| &entry.facts))
                .map(|fact| (fact.kind, plain(&fact.text), fact))
                .collect();
            let expected: Vec<(FactKind, String, &str)> = expected
                .lines()
                .filter_map(|line| {
                    let (kind, text) = line.split_once('\t')?;
                    Some((FactKind::from_stored(kind)?, plain(text), text))
                })
                .collect();
            let missed: Vec<&(FactKind, String, &str)> = expected
                .iter()
                .filter(|(kind, text, _)| {
                    !found
                        .iter()
                        .any(|(found_kind, found_text, _)| found_kind == kind && found_text == text)
                })
                .collect();
            let extra: Vec<&(FactKind, String, &FoundFact)> = found
                .iter()
                .filter(|(kind, text, _)| {
                    !expected.iter().any(|(expected_kind, expected_text, _)| {
                        expected_kind == kind && expected_text == text
                    })
                })
                .collect();
            let recall = 1. - missed.len() as f64 / expected.len() as f64;
            recalls.push(recall);
            let usage = &reply.response.usage;
            println!(
                "== {name}: {} facts, {} expected, recall {:.0}%, {} unsure, {} cleaned; {:?} in, {:?} out, ${:.5}, {:.1} s",
                found.len(),
                expected.len(),
                recall * 100.,
                found
                    .iter()
                    .filter(|(.., fact)| fact
                        .review
                        .as_ref()
                        .is_some_and(|review| review.reason == ReviewReason::Unsure))
                    .count(),
                found
                    .iter()
                    .filter(|(.., fact)| fact
                        .review
                        .as_ref()
                        .is_some_and(|review| review.reason == ReviewReason::Cleaned))
                    .count(),
                usage.input_tokens,
                usage.output_tokens,
                reply.cost.unwrap_or(0.),
                elapsed.as_secs_f64(),
            );
            for (kind, _, text) in &missed {
                println!("  missed {kind:?}: {text}");
            }
            for (kind, _, fact) in &extra {
                println!(
                    "  extra  {kind:?}: {} {:?}",
                    fact.text,
                    fact.review.as_ref().map(|review| review.reason)
                );
            }
            // Each is something the reader got wrong, or the CV did.
            for (kind, _, fact) in &found {
                if let Some((_, written)) = &fact.source
                    && *kind != FactKind::Contact
                    && ![clean(written, false), clean(written, true)].contains(&fact.text)
                {
                    println!(
                        "  fixed  {kind:?}: {:?} → {:?}",
                        clean(written, false),
                        fact.text
                    );
                }
            }
        }
        println!("== total ${total_cost:.5}");
        assert!(
            recalls.iter().all(|recall| *recall >= 0.9),
            "recall {recalls:?}"
        );
        Ok(())
    }
}
