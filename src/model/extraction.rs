// Finds a document's facts with a model. The model is sent the text with the
// user's name and contact details hidden (`redaction`) and quotes each fact;
// each quote is looked up in the text sent, never trusting the model to say
// where it is, so a fact says what the document says and cites where. A
// quote that isn't there word for word is kept and marked unsure: the model
// may have read it right and copied it wrong. Where the document breaks a
// word across lines, the fact is the word joined, marked for a look.

use std::ops::Range;

use rig_core::completion::CompletionRequest;
use rig_core::completion::options::{GenerationOptions, Reasoning};
use serde::Deserialize;
use uuid::Uuid;

use super::commands::custom_section;
use super::redaction::{Identifier, Redaction};
use super::{DocumentId, EntryKind, FactKind, Review, ReviewReason};
use crate::database::schema::{Entries, FactSources, Facts, entries, fact_sources, facts};
use crate::database::{Error, Write};

/// What the model is told. The answer's shape is spelled out, since DeepSeek
/// takes "a JSON object" rather than a schema.
const PREAMBLE: &str = r#"You read a CV, or other text about a person's work, and list what it says as exact quotes.

Answer with one JSON object:
{
  "summary": the profile or summary paragraph, or null,
  "contacts": the placeholders, like "[EMAIL_1]", that are this person's own email, phone, links and profiles,
  "entries": [
    {
      "kind": "experience", "education", "project", "certification", "skills" or "other",
      "section": for "other", the heading it is under, as written; otherwise null,
      "title": the job title, degree, project or certificate name, or a skills line's label (like "Languages"), or null,
      "organization": the employer, school or issuer, or null,
      "location": or null,
      "dates": as written, like "Mar 2021 – Present", or null,
      "bullets": [each bullet point or sentence about it],
      "skills": [each skill or technology listed for it]
    }
  ]
}

Rules:
- Copy every string exactly as the text has it: the same words, spelling, case and punctuation. Never rephrase, correct, shorten, or join text from different places.
- Leave out bullet markers (•, -, *) at the start of a point.
- A bullet point is one quote, even when it wraps onto the next line: quote all of it, with the line break as a space.
- Placeholders in square brackets, like [EMAIL_1], stand for hidden details. Copy them as they are. Never guess what they hide.
- Each job, degree, project and certification is its own entry, in the order the text has them. Two roles at one employer are two entries.
- A skills line like "Languages: Go, Java" is an entry of kind "skills" with the title "Languages" and the skills "Go" and "Java".
- Skills or technologies named with a project or job go in that entry's skills.
- Talks, awards, publications, volunteering, spoken languages and anything else go in entries of kind "other", each item a bullet.
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
    summary: Option<String>,
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
    title: Option<String>,
    #[serde(default)]
    organization: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    dates: Option<String>,
    #[serde(default)]
    bullets: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
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
        let mut fact = |kind: FactKind, quote: &str| -> Option<FoundFact> {
            let quote = quote.trim().trim_start_matches(BULLETS).trim();
            if quote.is_empty() {
                return None;
            }
            Some(match quotes.find(quote) {
                Some((redacted, joined)) => {
                    let span = redaction.original(redacted);
                    let written = &original[span.clone()];
                    let broken = written
                        .match_indices('-')
                        .any(|(index, _)| broken_word(&written[index..]).is_some());
                    FoundFact {
                        kind,
                        text: clean(written, joined),
                        source: Some((span, written.to_owned())),
                        review: broken.then(|| Review {
                            reason: ReviewReason::Cleaned,
                            note: Some(
                                "A word was split across two lines in the document. Check it's \
                                 joined right."
                                    .to_owned(),
                            ),
                        }),
                    }
                }
                None => FoundFact {
                    kind,
                    text: restore(quote, original, redaction),
                    source: None,
                    review: Some(Review {
                        reason: ReviewReason::Unsure,
                        note: Some(
                            "This isn't in the document word for word. Check it says what the \
                             document does."
                                .to_owned(),
                        ),
                    }),
                },
            })
        };

        let mut extraction = Extraction::default();
        extraction.profile.extend(
            answer
                .summary
                .as_deref()
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
                .filter_map(|(kind, quote)| fact(kind, quote.as_deref()?))
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
                if !extraction.profile.iter().any(|fact| fact.text == written) {
                    extraction.profile.push(FoundFact {
                        kind: FactKind::Contact,
                        text: written.to_owned(),
                        source: Some((replacement.original.clone(), written.to_owned())),
                        review: None,
                    });
                }
            }
        }
        extraction
    }

    pub fn count(&self) -> usize {
        self.profile.len()
            + self
                .entries
                .iter()
                .map(|entry| entry.facts.len())
                .sum::<usize>()
    }

    /// How many facts are marked for a look.
    pub fn to_review(&self) -> usize {
        self.profile
            .iter()
            .chain(self.entries.iter().flat_map(|entry| &entry.facts))
            .filter(|fact| fact.review.is_some())
            .count()
    }
}

/// Saves what was found in `document`: its entries, their facts and where
/// each was found.
pub fn save(write: &mut Write, document: DocumentId, extraction: &Extraction) -> Result<(), Error> {
    for fact in &extraction.profile {
        save_fact(write, document, None, fact)?;
    }
    for entry in &extraction.entries {
        let id = Uuid::now_v7();
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
        write.insert(
            Entries,
            id,
            &[
                entries::KIND.value(&entry.kind.stored()),
                entries::CUSTOM_SECTION_ID.value(&section),
            ],
        )?;
        for fact in &entry.facts {
            save_fact(write, document, Some(id), fact)?;
        }
    }
    Ok(())
}

fn save_fact(
    write: &mut Write,
    document: DocumentId,
    entry: Option<Uuid>,
    fact: &FoundFact,
) -> Result<(), Error> {
    let id = Uuid::now_v7();
    write.insert(
        Facts,
        id,
        &[
            facts::KIND.value(&fact.kind.stored()),
            facts::TEXT.value(&fact.text),
            facts::EXTRACTED.value(&fact.text),
            facts::ENTRY_ID.value(&entry),
            facts::REVIEW.value(&fact.review.as_ref().map(|review| review.reason.stored())),
            facts::REVIEW_NOTE.value(
                &fact
                    .review
                    .as_ref()
                    .and_then(|review| review.note.as_deref()),
            ),
        ],
    )?;
    if let Some((span, written)) = &fact.source {
        write.insert(
            FactSources,
            Uuid::now_v7(),
            &[
                fact_sources::FACT_ID.value(&id),
                fact_sources::DOCUMENT_ID.value(&document.0),
                fact_sources::SPAN_START.value(&(span.start as i64)),
                fact_sources::SPAN_END.value(&(span.end as i64)),
                fact_sources::ORIGINAL_TEXT.value(written),
            ],
        )?;
    }
    Ok(())
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

/// `quote` with the placeholders in it put back, for a fact that couldn't
/// be found: each as the first thing it replaced.
fn restore(quote: &str, original: &str, redaction: &Redaction) -> String {
    let mut restored = quote.to_owned();
    for replacement in &redaction.replacements {
        if restored.contains(&replacement.placeholder) {
            restored = restored.replace(
                &replacement.placeholder,
                &original[replacement.original.clone()],
            );
        }
    }
    restored
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Writer;
    use crate::model::redaction;

    const TEXT: &str = "Alex Rahman\n\
        alex@example.com · github.com/alexr\n\
        Backend engineer who likes quiet on-call weeks.\n\
        Senior Engineer, Northwind (Berlin) Mar 2021 – Present\n\
        • Designed the event pipe-\n\
        line for shipment updates.\n\
        • Cut p99 latency from 480 ms to\n95 ms.\n\
        Engineer, Northwind (Berlin) 2018 – 2021\n\
        • Built a real-\ntime tracker in Java.\n\
        • Wrote the team’s runbooks.\n\
        Languages: Java, JavaScript, Go\n\
        Databases: PostgreSQL, SQL\n";

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
             "contacts": ["[EMAIL_1]", "[LINK_1]"],
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
        assert_eq!(extraction.to_review(), 2);
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

    /// Sends the junior and senior test CVs to the provider set up in the
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
        }
        println!("== total ${total_cost:.5}");
        assert!(
            recalls.iter().all(|recall| *recall >= 0.9),
            "recall {recalls:?}"
        );
        Ok(())
    }
}
