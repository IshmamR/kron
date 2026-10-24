// Loads the sample library in `assets/sample/` for building the UI before
// import and generation exist. The data file's header explains its format.
// Development only: release builds start empty.

use std::collections::HashMap;

use chrono::{DateTime, Duration, DurationRound, Local, TimeZone, Utc};
use serde::Deserialize;
use uuid::{NoContext, Timestamp, Uuid};

use crate::database::schema::{
    Blocks, DocumentTexts, Documents, Entries, Events, FactOverrides, FactSources,
    FactVersions, Facts, Generations, JobDescriptions, Jobs, LineCitations, Lines, Sections,
    UnsupportedClaims, blocks, document_texts, documents, entries, events,
    fact_overrides, fact_sources, fact_versions, facts, generations, job_descriptions, jobs,
    line_citations, lines, sections, unsupported_claims,
};
use crate::database::{self, Write};
use crate::typeset;

use crate::model::commands;
use crate::model::{
    Block, ClaimId, CoverLetter, DocumentId, DocumentKind, Entry, EntryBlock, EntryId, EntryKind,
    Fact, FactId, FactKind, FactSource, FactVersion, Inclusion, Review, ReviewReason,
    InterviewKind, Job, JobEvent, JobEventKind, JobId, Library, Line, LineId, ResumeId, Section,
    SectionKind, SourceDocument, TailoredResume, UnsupportedClaim, UnsupportedReason,
};

const LIBRARY: &str = include_str!("../../assets/sample/library.ron");
/// Documents too long to write inline, by `text_file`.
const TEXT_FILES: [(&str, &str); 2] = [
    ("cv.txt", include_str!("../../assets/sample/cv.txt")),
    ("linkedin.txt", include_str!("../../assets/sample/linkedin.txt")),
];
const DEFAULT_DOCUMENT: &str = "cv";
/// What the sample's imported documents are saved as: their text, since the
/// sample has no PDFs.
const DOCUMENT_MEDIA_TYPE: &str = "text/plain; charset=utf-8";
/// The paper the bundled templates set.
const PAPER: &str = "letter";
/// The custom section the sample's `Other` entries go in.
const CUSTOM_SECTION: &str = "Other";

pub fn library() -> Result<Library, String> {
    let data: SampleData =
        ron::from_str(LIBRARY).map_err(|error| format!("sample library: {error}"))?;
    let mut loader = Loader {
        library: Library::default(),
        documents: HashMap::new(),
        facts: HashMap::new(),
        entries: HashMap::new(),
    };
    for document in data.documents {
        loader.add_document(document)?;
    }
    for fact in data.facts {
        loader.add_fact(fact, None, None, None, 0)?;
    }
    for entry in data.entries {
        loader.add_entry(entry)?;
    }
    for job in data.jobs {
        let job = loader.job(job)?;
        loader.library.jobs.push(job);
    }
    Ok(loader.library)
}

/// Writes the sample library unless the database has documents or jobs
/// already, so a first run has something on every screen.
pub fn write_if_empty(write: &mut Write) -> Result<(), database::Error> {
    let has_data: bool = write
        .prepare("SELECT EXISTS (SELECT 1 FROM documents) OR EXISTS (SELECT 1 FROM jobs)")?
        .query_row([], |row| row.get(0))?;
    if has_data {
        Ok(())
    } else {
        write_library(write)
    }
}

/// Writes the sample library into the database. Its templates must be
/// installed (`typeset::install_templates`).
pub fn write_library(write: &mut Write) -> Result<(), database::Error> {
    let library = library().map_err(database::Error::Misuse)?;
    for document in &library.documents {
        write_document(write, document)?;
    }
    for entry in &library.entries {
        let section = match entry.kind {
            EntryKind::Other => Some(commands::other_section(write)?),
            _ => None,
        };
        write.insert(
            Entries,
            entry.id.0,
            &[
                entries::KIND.value(&entry.kind.stored()),
                entries::CUSTOM_SECTION_ID.value(&section),
            ],
        )?;
    }
    for fact in &library.facts {
        write_fact(write, fact)?;
    }
    for (position, job) in library.jobs.iter().enumerate() {
        write_job(write, job, position)?;
    }
    Ok(())
}

struct Loader {
    library: Library,
    /// Each document's id, by key.
    documents: HashMap<String, DocumentId>,
    /// Each fact's id and extracted text, by key. Résumés are generated from
    /// the extracted text: edits in the sample came after generation.
    facts: HashMap<String, (FactId, String)>,
    /// The facts that head each entry, by key.
    entries: HashMap<String, EntryFacts>,
}

struct EntryFacts {
    title: FactId,
    organization: Option<FactId>,
    dates: Option<FactId>,
    location: Option<FactId>,
    skills: Vec<FactId>,
}

impl Loader {
    fn add_document(&mut self, document: SampleDocument) -> Result<(), String> {
        let text = match (document.text, document.text_file) {
            (Some(text), None) => text,
            (None, Some(file)) => TEXT_FILES
                .iter()
                .find(|(name, _)| *name == file)
                // A checkout with Windows line endings would otherwise hide
                // every fact that spans a line break.
                .map(|(_, text)| text.replace("\r\n", "\n"))
                .ok_or_else(|| format!("no text file is called {file:?}"))?,
            _ => {
                return Err(format!(
                    "document {:?} needs one of `text` and `text_file`",
                    document.key
                ));
            }
        };
        let id = DocumentId::new();
        self.library.documents.push(SourceDocument {
            id,
            kind: match document.kind {
                SampleDocumentKind::File(file_name) => DocumentKind::File { file_name },
                SampleDocumentKind::Note => DocumentKind::Note,
            },
            text,
            added_at: Utc::now() - Duration::days(document.added_days_ago),
        });
        self.documents.insert(document.key, id);
        Ok(())
    }

    /// Where `text` first is in the document called `key`, from byte `after`
    /// on.
    fn source(&self, key: &str, text: &str, after: usize) -> Result<FactSource, String> {
        let document = self
            .documents
            .get(key)
            .and_then(|&id| self.library.document(id))
            .ok_or_else(|| format!("no document is called {key:?}"))?;
        let start = document
            .text
            .get(after..)
            .and_then(|rest| rest.find(text))
            .map(|offset| after + offset)
            .ok_or_else(|| format!("{text:?} isn't in document {key:?}"))?;
        Ok(FactSource {
            document: document.id,
            span: start..start + text.len(),
            original_text: text.to_string(),
        })
    }

    /// `from` and `status` default to the entry's, when the fact is in one.
    /// It's looked for in its document from byte `after` on, so a fact like
    /// "Dhaka, Bangladesh" is found where its own entry says it.
    fn add_fact(
        &mut self,
        fact: SampleFact,
        entry: Option<EntryId>,
        from: Option<&str>,
        status: Option<SampleStatus>,
        after: usize,
    ) -> Result<FactId, String> {
        let after = if fact.from.is_some() { 0 } else { after };
        let from = fact.from.as_deref().or(from).unwrap_or(DEFAULT_DOCUMENT);
        let original_text = fact.source.unwrap_or_else(|| fact.text.clone());
        let mut sources = vec![self.source(from, &original_text, after)?];
        for key in &fact.also_in {
            sources.push(self.source(key, &fact.text, 0)?);
        }
        let id = FactId::new();
        self.library.facts.push(Fact {
            id,
            kind: fact.kind,
            text: fact.edited.unwrap_or_else(|| fact.text.clone()),
            extracted: fact.text.clone(),
            sources,
            entry,
            hidden: fact.status.or(status) == Some(SampleStatus::Hidden),
            // Every flag in the sample is text the extraction had to clean up.
            review: fact.flag.map(|note| Review {
                reason: ReviewReason::Cleaned,
                note: Some(note),
            }),
        });
        if self
            .facts
            .insert(fact.key.clone(), (id, fact.text))
            .is_some()
        {
            return Err(format!("two facts are called {:?}", fact.key));
        }
        Ok(id)
    }

    fn add_entry(&mut self, entry: SampleEntry) -> Result<(), String> {
        let id = EntryId::new();
        self.library.entries.push(Entry {
            id,
            kind: entry.kind,
        });
        let from = entry.from.as_deref();
        let status = entry.status;

        let title_kind = match entry.kind {
            EntryKind::Skills => FactKind::SkillGroup,
            _ => FactKind::Title,
        };
        let title = self.add_fact(
            SampleFact::new(&entry.key, title_kind, &entry.title),
            Some(id),
            from,
            status,
            0,
        )?;
        let after = self
            .library
            .fact(title)
            .and_then(|fact| fact.sources.first())
            .map_or(0, |source| source.span.start);
        let mut add = |kind: FactKind, text: Option<String>| {
            text.map(|text| self.add_fact(SampleFact::new(&entry.key, kind, &text), Some(id), from, status, after))
                .transpose()
        };
        let organization = add(FactKind::Organization, entry.organization)?;
        let location = add(FactKind::Location, entry.location)?;
        let dates = add(FactKind::Date, entry.dates)?;
        let skills = entry
            .skills
            .into_iter()
            .map(|skill| {
                self.add_fact(SampleFact::new(&entry.key, FactKind::Skill, &skill), Some(id), from, status, after)
            })
            .collect::<Result<_, String>>()?;
        for bullet in entry.bullets {
            self.add_fact(bullet, Some(id), from, status, after)?;
        }
        self.entries.insert(
            entry.key,
            EntryFacts {
                title,
                organization,
                dates,
                location,
                skills,
            },
        );
        Ok(())
    }

    fn job(&self, job: SampleJob) -> Result<Job, String> {
        let mut resumes = job
            .resumes
            .into_iter()
            .map(|resume| self.resume(resume))
            .collect::<Result<Vec<_>, String>>()?;
        resumes.sort_by_key(|resume| resume.generated_at);
        let events = job
            .events
            .into_iter()
            .map(|event| event.into_event(&resumes))
            .collect();
        Ok(Job {
            id: JobId::new(),
            title: job.title,
            company: job.company,
            description: job.description,
            url: job.url,
            created_at: Utc::now() - Duration::days(job.created_days_ago),
            overrides: Vec::new(),
            resumes,
            cover_letter: None,
            events,
        })
    }

    fn resume(&self, resume: SampleResume) -> Result<TailoredResume, String> {
        let sections: Vec<Section> = resume
            .sections
            .into_iter()
            .map(|section| self.section(section))
            .collect::<Result<_, String>>()?;
        let unsupported_claims: Vec<UnsupportedClaim> = resume
            .unsupported
            .into_iter()
            .map(|claim| {
                Ok(UnsupportedClaim {
                    id: ClaimId::new(),
                    text: claim.text,
                    reason: match claim.reason {
                        SampleReason::InventedFigure(figure) => {
                            UnsupportedReason::InventedFigure(figure)
                        }
                        SampleReason::InventedYear(year) => UnsupportedReason::InventedYear(year),
                        SampleReason::NoSupportingFact => UnsupportedReason::NoSupportingFact,
                        SampleReason::UsesHiddenFact(key) => {
                            UnsupportedReason::UsesHiddenFact(self.fact(&key)?.0)
                        }
                    },
                    added_as: None,
                })
            })
            .collect::<Result<_, String>>()?;

        // Generated from the facts as extracted, before any edit or hiding in
        // the sample, except the hidden facts its claims needed.
        let cited = sections
            .iter()
            .flat_map(|section| &section.blocks)
            .flat_map(Block::lines)
            .flat_map(|line| line.cites.iter().map(|&fact| (fact, false)));
        let needed_hidden = unsupported_claims
            .iter()
            .filter_map(|claim| match claim.reason {
                UnsupportedReason::UsesHiddenFact(fact) => Some((fact, true)),
                _ => None,
            });
        let mut basis: Vec<FactVersion> = Vec::new();
        for (fact, hidden) in cited.chain(needed_hidden) {
            if basis.iter().any(|version| version.fact == fact) {
                continue;
            }
            let text = self
                .library
                .fact(fact)
                .map(|fact| fact.extracted.clone())
                .unwrap_or_default();
            basis.push(FactVersion { fact, text, hidden });
        }

        Ok(TailoredResume {
            id: ResumeId::new(),
            // On the hour, so the times shown don't change between launches.
            generated_at: (Utc::now() - Duration::hours(resume.generated_hours_ago))
                .duration_trunc(Duration::hours(1))
                .unwrap_or_else(|_| Utc::now()),
            template: resume.template,
            sections,
            unsupported_claims,
            basis,
        })
    }

    fn section(&self, section: SampleSection) -> Result<Section, String> {
        let (kind, title, blocks) = match section {
            SampleSection::Header { contacts } => {
                let contacts = contacts
                    .iter()
                    .map(|key| Ok(self.fact_line(self.fact(key)?.0)))
                    .collect::<Result<_, String>>()?;
                let name = self.fact_line(self.fact("name")?.0);
                (SectionKind::Header, "", vec![Block::Header { name, contacts }])
            }
            SampleSection::Summary(line) => (
                SectionKind::Summary,
                "Summary",
                vec![Block::Text(self.line(line)?)],
            ),
            SampleSection::Experience(entries) => {
                (SectionKind::Experience, "Experience", self.entry_blocks(entries)?)
            }
            SampleSection::Projects(entries) => {
                (SectionKind::Projects, "Projects", self.entry_blocks(entries)?)
            }
            SampleSection::Education(entries) => {
                (SectionKind::Education, "Education", self.entry_blocks(entries)?)
            }
            SampleSection::Skills(groups) => {
                let blocks = groups
                    .into_iter()
                    .map(|group| self.skills_block(group))
                    .collect::<Result<_, String>>()?;
                (SectionKind::Skills, "Skills", blocks)
            }
            SampleSection::List { title, items } => {
                let blocks = items
                    .into_iter()
                    .map(|line| Ok(Block::Item(self.line(line)?)))
                    .collect::<Result<_, String>>()?;
                return Ok(Section {
                    kind: SectionKind::Other,
                    title,
                    blocks,
                });
            }
        };
        Ok(Section {
            kind,
            title: title.to_string(),
            blocks,
        })
    }

    /// Each entry with all of its heading facts, and the bullets chosen.
    fn entry_blocks(&self, entries: Vec<SampleEntryBlock>) -> Result<Vec<Block>, String> {
        entries
            .into_iter()
            .map(|block| {
                let facts = self.entry(&block.entry)?;
                let stack = (!facts.skills.is_empty()).then(|| self.joined_line(&facts.skills));
                Ok(Block::Entry(Box::new(EntryBlock {
                    title: self.fact_line(facts.title),
                    organization: facts.organization.map(|id| self.fact_line(id)),
                    dates: facts.dates.map(|id| self.fact_line(id)),
                    location: facts.location.map(|id| self.fact_line(id)),
                    stack,
                    bullets: block
                        .bullets
                        .into_iter()
                        .map(|line| self.line(line))
                        .collect::<Result<_, String>>()?,
                })))
            })
            .collect()
    }

    fn skills_block(&self, group: SampleSkills) -> Result<Block, String> {
        let facts = self.entry(&group.group)?;
        let skills: Vec<FactId> = if group.only.is_empty() {
            facts.skills.clone()
        } else {
            group
                .only
                .iter()
                .map(|skill| {
                    facts
                        .skills
                        .iter()
                        .copied()
                        .find(|&id| self.library.fact(id).is_some_and(|fact| fact.extracted == *skill))
                        .ok_or_else(|| format!("{:?} has no skill {skill:?}", group.group))
                })
                .collect::<Result<_, String>>()?
        };
        Ok(Block::Skills {
            label: Some(self.fact_line(facts.title)),
            items: self.joined_line(&skills),
        })
    }

    fn entry(&self, key: &str) -> Result<&EntryFacts, String> {
        self.entries
            .get(key)
            .ok_or_else(|| format!("no entry is called {key:?}"))
    }

    /// A line that's the fact's text as extracted, citing it.
    fn fact_line(&self, id: FactId) -> Line {
        self.joined_line(&[id])
    }

    /// "Rust, Tokio": the facts' texts in a list, citing them.
    fn joined_line(&self, ids: &[FactId]) -> Line {
        let text = ids
            .iter()
            .filter_map(|&id| self.library.fact(id))
            .map(|fact| fact.extracted.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Line {
            id: LineId::new(),
            generated: text.clone(),
            text,
            cites: ids.to_vec(),
        }
    }

    fn line(&self, line: SampleLine) -> Result<Line, String> {
        let (generated, text, cites) = match line {
            SampleLine::Fact(key) => {
                let (id, text) = self.fact(&key)?;
                (text.clone(), text.clone(), vec![*id])
            }
            SampleLine::Reworded { text, cites } => {
                let cites = cites
                    .iter()
                    .map(|key| Ok(self.fact(key)?.0))
                    .collect::<Result<_, String>>()?;
                (text.clone(), text, cites)
            }
            SampleLine::Edited { fact, text } => {
                let (id, generated) = self.fact(&fact)?;
                (generated.clone(), text, vec![*id])
            }
        };
        Ok(Line {
            id: LineId::new(),
            generated,
            text,
            cites,
        })
    }

    fn fact(&self, key: &str) -> Result<&(FactId, String), String> {
        self.facts
            .get(key)
            .ok_or_else(|| format!("no fact is called {key:?}"))
    }
}

#[derive(Deserialize)]
struct SampleData {
    documents: Vec<SampleDocument>,
    facts: Vec<SampleFact>,
    entries: Vec<SampleEntry>,
    jobs: Vec<SampleJob>,
}

#[derive(Deserialize)]
struct SampleDocument {
    key: String,
    kind: SampleDocumentKind,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    text_file: Option<String>,
    added_days_ago: i64,
}

#[derive(Deserialize)]
enum SampleDocumentKind {
    File(String),
    Note,
}

#[derive(Deserialize)]
struct SampleFact {
    #[serde(default)]
    key: String,
    #[serde(default = "bullet")]
    kind: FactKind,
    text: String,
    #[serde(default)]
    from: Option<String>,
    /// The text as it appears in the document, when extraction didn't copy
    /// it cleanly. Defaults to `text`.
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    also_in: Vec<String>,
    #[serde(default)]
    edited: Option<String>,
    #[serde(default)]
    status: Option<SampleStatus>,
    /// Why the extraction was flagged for a look.
    #[serde(default)]
    flag: Option<String>,
}

impl SampleFact {
    /// A fact the data file gives no key, like an entry's title or dates.
    fn new(entry: &str, kind: FactKind, text: &str) -> Self {
        Self {
            key: format!("{entry} {kind:?}: {text}"),
            kind,
            text: text.into(),
            from: None,
            source: None,
            also_in: Vec::new(),
            edited: None,
            status: None,
            flag: None,
        }
    }
}

#[derive(Deserialize)]
struct SampleEntry {
    key: String,
    kind: EntryKind,
    title: String,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    status: Option<SampleStatus>,
    #[serde(default)]
    organization: Option<String>,
    #[serde(default)]
    dates: Option<String>,
    #[serde(default)]
    location: Option<String>,
    /// A project's stack, or a skill group's skills.
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    bullets: Vec<SampleFact>,
}

#[derive(Deserialize)]
struct SampleJob {
    title: String,
    company: String,
    description: String,
    #[serde(default)]
    url: Option<String>,
    created_days_ago: i64,
    #[serde(default)]
    events: Vec<SampleEvent>,
    /// Every version generated; the latest is the current one.
    #[serde(default)]
    resumes: Vec<SampleResume>,
}

#[derive(Deserialize)]
struct SampleEvent {
    /// Negative for events to come.
    days_ago: i64,
    /// Local hour of the day; otherwise the current time.
    #[serde(default)]
    hour: Option<u32>,
    kind: SampleEventKind,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize)]
enum SampleEventKind {
    Applied,
    RecruiterReplied,
    Interview {
        kind: InterviewKind,
        #[serde(default)]
        with: String,
    },
    Offer,
    Rejected,
    Withdrawn,
    NoResponse,
    Note,
}

impl SampleEvent {
    /// An application sends the latest résumé generated before it.
    fn into_event(self, resumes: &[TailoredResume]) -> JobEvent {
        let at = sample_time(self.days_ago, self.hour);
        let kind = match self.kind {
            SampleEventKind::Applied => JobEventKind::Applied {
                resume: resumes
                    .iter()
                    .rev()
                    .find(|resume| resume.generated_at <= at)
                    .map(|resume| resume.id),
            },
            SampleEventKind::RecruiterReplied => JobEventKind::RecruiterReplied,
            SampleEventKind::Interview { kind, with } => JobEventKind::Interview { kind, with },
            SampleEventKind::Offer => JobEventKind::Offer,
            SampleEventKind::Rejected => JobEventKind::Rejected,
            SampleEventKind::Withdrawn => JobEventKind::Withdrawn,
            SampleEventKind::NoResponse => JobEventKind::NoResponse,
            SampleEventKind::Note => JobEventKind::Note,
        };
        JobEvent {
            note: self.note,
            ..JobEvent::new(at, kind)
        }
    }
}

/// `days_ago` days from now, at `hour` local time if given.
fn sample_time(days_ago: i64, hour: Option<u32>) -> DateTime<Utc> {
    let at = Utc::now() - Duration::days(days_ago);
    hour.and_then(|hour| {
        let day = at.with_timezone(&Local).date_naive();
        Local
            .from_local_datetime(&day.and_hms_opt(hour, 0, 0)?)
            .single()
    })
    .map_or(at, |local| local.with_timezone(&Utc))
}

#[derive(Deserialize)]
struct SampleResume {
    generated_hours_ago: i64,
    #[serde(default = "default_template")]
    template: String,
    sections: Vec<SampleSection>,
    #[serde(default)]
    unsupported: Vec<SampleClaim>,
}

#[derive(Deserialize)]
enum SampleSection {
    /// The name, then the contacts, by fact key.
    Header { contacts: Vec<String> },
    Summary(SampleLine),
    Experience(Vec<SampleEntryBlock>),
    Projects(Vec<SampleEntryBlock>),
    Education(Vec<SampleEntryBlock>),
    Skills(Vec<SampleSkills>),
    List { title: String, items: Vec<SampleLine> },
}

#[derive(Deserialize)]
struct SampleEntryBlock {
    entry: String,
    #[serde(default)]
    bullets: Vec<SampleLine>,
}

#[derive(Deserialize)]
struct SampleSkills {
    /// The skill group's entry key.
    group: String,
    /// Which of its skills, by text; all of them when empty.
    #[serde(default)]
    only: Vec<String>,
}

#[derive(Deserialize)]
enum SampleLine {
    Fact(String),
    Reworded { text: String, cites: Vec<String> },
    Edited { fact: String, text: String },
}

#[derive(Deserialize)]
struct SampleClaim {
    text: String,
    reason: SampleReason,
}

/// Only hidden still means something: facts need review only when flagged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
enum SampleStatus {
    Unreviewed,
    Confirmed,
    Hidden,
}

#[derive(Deserialize)]
enum SampleReason {
    InventedFigure(String),
    InventedYear(u16),
    NoSupportingFact,
    UsesHiddenFact(String),
}

fn bullet() -> FactKind {
    FactKind::Bullet
}

fn default_template() -> String {
    crate::typeset::DEFAULT_TEMPLATE.to_string()
}

fn write_document(write: &mut Write, document: &SourceDocument) -> Result<(), database::Error> {
    let (file_name, file_hash) = match &document.kind {
        DocumentKind::File { file_name } => (
            Some(file_name.as_str()),
            Some(write.add_file(document.text.as_bytes(), DOCUMENT_MEDIA_TYPE)?),
        ),
        DocumentKind::Note => (None, None),
    };
    write.insert(
        Documents,
        document.id.0,
        &[
            documents::FILE_NAME.value(&file_name),
            documents::FILE_HASH.value(&file_hash),
            documents::ADDED_AT.value(&document.added_at.timestamp_millis()),
        ],
    )?;
    write.insert(
        DocumentTexts,
        document.id.0,
        &[document_texts::TEXT.value(&document.text)],
    )
}

fn write_fact(write: &mut Write, fact: &Fact) -> Result<(), database::Error> {
    let (review, review_note) = match &fact.review {
        Some(review) => (Some(review.reason.stored()), review.note.as_deref()),
        None => (None, None),
    };
    write.insert(
        Facts,
        fact.id.0,
        &[
            facts::KIND.value(&fact.kind.stored()),
            facts::TEXT.value(&fact.text),
            facts::EXTRACTED.value(&fact.extracted),
            facts::ENTRY_ID.value(&fact.entry.map(|entry| entry.0)),
            facts::HIDDEN.value(&fact.hidden),
            facts::REVIEW.value(&review),
            facts::REVIEW_NOTE.value(&review_note),
        ],
    )?;
    for source in &fact.sources {
        write.insert(
            FactSources,
            Uuid::now_v7(),
            &[
                fact_sources::FACT_ID.value(&fact.id.0),
                fact_sources::DOCUMENT_ID.value(&source.document.0),
                fact_sources::SPAN_START.value(&(source.span.start as i64)),
                fact_sources::SPAN_END.value(&(source.span.end as i64)),
                fact_sources::ORIGINAL_TEXT.value(&source.original_text),
            ],
        )?;
    }
    Ok(())
}

fn write_job(write: &mut Write, job: &Job, position: usize) -> Result<(), database::Error> {
    let created_at = job.created_at.timestamp_millis();
    write.insert(
        Jobs,
        job.id.0,
        &[
            jobs::TITLE.value(&job.title),
            jobs::COMPANY.value(&job.company),
            jobs::URL.value(&job.url),
            jobs::CREATED_AT.value(&created_at),
            jobs::BOARD_SORT_KEY.value(&sort_key(position)?),
        ],
    )?;
    write.insert(
        JobDescriptions,
        job.id.0,
        &[job_descriptions::TEXT.value(&job.description)],
    )?;
    for override_ in &job.overrides {
        let inclusion = override_.inclusion.map(Inclusion::stored);
        write.insert(
            FactOverrides,
            (job.id.0, override_.fact.0),
            &[
                fact_overrides::INCLUSION.value(&inclusion),
                fact_overrides::TEXT.value(&override_.text),
            ],
        )?;
    }
    for resume in &job.resumes {
        write_resume(write, job.id, resume)?;
    }
    if let Some(letter) = &job.cover_letter {
        write_cover_letter(write, job.id, letter)?;
    }

    // Adding a job logs `wishlisted`; the model leaves it out. Its id is from
    // when the job was added, so it sorts before anything logged later.
    let added = Timestamp::from_unix(
        NoContext,
        job.created_at.timestamp().max(0) as u64,
        job.created_at.timestamp_subsec_nanos(),
    );
    write.insert(
        Events,
        Uuid::new_v7(added),
        &[
            events::JOB_ID.value(&job.id.0),
            events::AT.value(&created_at),
            events::KIND.value(&"wishlisted"),
        ],
    )?;
    for event in &job.events {
        commands::save_event(write, job.id, event)?;
    }
    Ok(())
}

fn write_resume(
    write: &mut Write,
    job: JobId,
    resume: &TailoredResume,
) -> Result<(), database::Error> {
    let template = typeset::template(&resume.template);
    write.insert(
        Generations,
        resume.id.0,
        &[
            generations::JOB_ID.value(&job.0),
            generations::KIND.value(&"resume"),
            generations::GENERATED_AT.value(&resume.generated_at.timestamp_millis()),
            generations::TEMPLATE_VERSION_ID.value(&Some(template.version_id)),
            generations::PAPER.value(&Some(PAPER)),
        ],
    )?;
    for (position, section) in resume.sections.iter().enumerate() {
        let kind = section.kind.stored();
        let section_id = write_section(write, resume.id.0, position, kind, &section.title)?;
        for (position, block) in section.blocks.iter().enumerate() {
            let (kind, lines): (_, Vec<(&str, &Line)>) = match block {
                Block::Header { name, contacts } => (
                    "header",
                    std::iter::once(("name", name))
                        .chain(contacts.iter().map(|line| ("contact", line)))
                        .collect(),
                ),
                Block::Entry(entry) => (
                    "entry",
                    [
                        Some(("title", &entry.title)),
                        entry
                            .organization
                            .as_ref()
                            .map(|line| ("organization", line)),
                        entry.dates.as_ref().map(|line| ("dates", line)),
                        entry.location.as_ref().map(|line| ("location", line)),
                        entry.stack.as_ref().map(|line| ("stack", line)),
                    ]
                    .into_iter()
                    .flatten()
                    .chain(entry.bullets.iter().map(|line| ("bullet", line)))
                    .collect(),
                ),
                Block::Skills { label, items } => (
                    "skills",
                    label
                        .iter()
                        .map(|line| ("label", line))
                        .chain([("items", items)])
                        .collect(),
                ),
                Block::Text(line) => ("text", vec![("text", line)]),
                Block::Item(line) => ("item", vec![("item", line)]),
            };
            write_block(write, section_id, position, kind, &lines)?;
        }
    }
    for version in &resume.basis {
        write.insert(
            FactVersions,
            (resume.id.0, version.fact.0),
            &[
                fact_versions::TEXT.value(&version.text),
                fact_versions::HIDDEN.value(&version.hidden),
            ],
        )?;
    }
    write_claims(write, resume.id.0, &resume.unsupported_claims)
}

/// Plain text, without a template: a body of paragraphs.
fn write_cover_letter(
    write: &mut Write,
    job: JobId,
    letter: &CoverLetter,
) -> Result<(), database::Error> {
    let id = Uuid::now_v7();
    write.insert(
        Generations,
        id,
        &[
            generations::JOB_ID.value(&job.0),
            generations::KIND.value(&"cover_letter"),
            generations::GENERATED_AT.value(&letter.generated_at.timestamp_millis()),
        ],
    )?;
    let body = write_section(write, id, 0, "body", "")?;
    for (position, paragraph) in letter.paragraphs.iter().enumerate() {
        let lines: Vec<_> = paragraph.iter().map(|line| ("sentence", line)).collect();
        write_block(write, body, position, "paragraph", &lines)?;
    }
    write_claims(write, id, &letter.unsupported_claims)
}

fn write_section(
    write: &mut Write,
    generation: Uuid,
    position: usize,
    kind: &str,
    title: &str,
) -> Result<Uuid, database::Error> {
    let id = Uuid::now_v7();
    write.insert(
        Sections,
        id,
        &[
            sections::GENERATION_ID.value(&generation),
            sections::SORT_KEY.value(&sort_key(position)?),
            sections::KIND.value(&kind),
            sections::TITLE.value(&title),
        ],
    )?;
    Ok(id)
}

/// The block and its lines, each with its role.
fn write_block(
    write: &mut Write,
    section: Uuid,
    position: usize,
    kind: &str,
    lines: &[(&str, &Line)],
) -> Result<(), database::Error> {
    let block = Uuid::now_v7();
    write.insert(
        Blocks,
        block,
        &[
            blocks::SECTION_ID.value(&section),
            blocks::SORT_KEY.value(&sort_key(position)?),
            blocks::KIND.value(&kind),
        ],
    )?;
    for (position, (role, line)) in lines.iter().enumerate() {
        write.insert(
            Lines,
            line.id.0,
            &[
                lines::BLOCK_ID.value(&block),
                lines::ROLE.value(role),
                lines::SORT_KEY.value(&sort_key(position)?),
                lines::GENERATED.value(&line.generated),
                lines::TEXT.value(&line.text),
            ],
        )?;
        let mut cited = Vec::new();
        for fact in &line.cites {
            // A fact is cited once per line: it's the key.
            if cited.contains(fact) {
                continue;
            }
            write.insert(
                LineCitations,
                (line.id.0, fact.0),
                &[line_citations::SORT_KEY.value(&sort_key(cited.len())?)],
            )?;
            cited.push(*fact);
        }
    }
    Ok(())
}

fn write_claims(
    write: &mut Write,
    generation: Uuid,
    claims: &[UnsupportedClaim],
) -> Result<(), database::Error> {
    for (position, claim) in claims.iter().enumerate() {
        let (reason, figure, year, hidden_fact) = match &claim.reason {
            UnsupportedReason::InventedFigure(figure) => {
                ("invented_figure", Some(figure.as_str()), None, None)
            }
            UnsupportedReason::InventedYear(year) => ("invented_year", None, Some(*year), None),
            UnsupportedReason::NoSupportingFact => ("no_supporting_fact", None, None, None),
            UnsupportedReason::UsesHiddenFact(fact) => {
                ("uses_hidden_fact", None, None, Some(fact.0))
            }
        };
        write.insert(
            UnsupportedClaims,
            claim.id.0,
            &[
                unsupported_claims::GENERATION_ID.value(&generation),
                unsupported_claims::SORT_KEY.value(&sort_key(position)?),
                unsupported_claims::TEXT.value(&claim.text),
                unsupported_claims::REASON.value(&reason),
                unsupported_claims::FIGURE.value(&figure),
                unsupported_claims::YEAR.value(&year),
                unsupported_claims::HIDDEN_FACT_ID.value(&hidden_fact),
                unsupported_claims::ADDED_AS.value(&claim.added_as.map(|fact| fact.0)),
            ],
        )?;
    }
    Ok(())
}

/// The key for the `position`th item of a list, in the integer form of
/// fractional indexing ("a0" … "az", then "b00" …), so an item can later go
/// between two without renumbering the rest.
fn sort_key(position: usize) -> Result<String, database::Error> {
    const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let digit = |index: usize| DIGITS[index % 62] as char;
    match position {
        0..62 => Ok(format!("a{}", digit(position))),
        62..3844 => Ok(format!("b{}{}", digit(position / 62), digit(position))),
        _ => Err(database::Error::Misuse(format!(
            "no sort key for position {position}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::database::{Error, Writer};

    fn count(connection: &Connection, table: &str) -> Result<i64, Error> {
        Ok(
            connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })?,
        )
    }

    #[test]
    fn sample_library_loads() {
        if let Err(error) = super::library() {
            panic!("{error}");
        }
    }

    #[test]
    fn the_sample_fits_the_schema_once() -> Result<(), Error> {
        let library = library().map_err(Error::Misuse)?;
        let mut writer = Writer::open_in_memory()?;
        for _ in 0..2 {
            writer.execute(|write| {
                typeset::install_templates(write)?;
                write_if_empty(write)
            })?;
        }
        let kron = writer.kron();
        assert_eq!(count(kron, "documents")?, library.documents.len() as i64);
        assert_eq!(count(kron, "facts")?, library.facts.len() as i64);
        assert_eq!(count(kron, "jobs")?, library.jobs.len() as i64);
        let resumes: usize = library.jobs.iter().map(|job| job.resumes.len()).sum();
        assert_eq!(
            count(kron, "generations WHERE kind = 'resume'")?,
            resumes as i64
        );
        assert_eq!(
            count(kron, "template_versions")?,
            typeset::TEMPLATES.len() as i64
        );
        // Every job's stage comes from its events, starting with `wishlisted`.
        assert_eq!(count(kron, "latest_steps")?, library.jobs.len() as i64);
        let violations: i64 =
            kron.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        assert_eq!(violations, 0);
        Ok(())
    }

    #[test]
    fn erasing_lets_the_sample_be_written_again() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let write_sample = |write: &mut Write| {
            typeset::install_templates(write)?;
            write_library(write)
        };
        writer.execute(write_sample)?;
        let facts = count(writer.kron(), "facts")?;
        writer.execute(|write| {
            write.erase_everything()?;
            write_sample(write)
        })?;
        assert_eq!(count(writer.kron(), "facts")?, facts);
        assert_eq!(count(writer.kron(), "settings")?, 1);
        Ok(())
    }

    #[test]
    fn sort_keys_sort_in_order() -> Result<(), Error> {
        let keys = (0..200).map(sort_key).collect::<Result<Vec<_>, _>>()?;
        assert!(keys.is_sorted());
        Ok(())
    }
}
