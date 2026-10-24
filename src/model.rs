mod application;
pub mod commands;
#[cfg(any(debug_assertions, test))]
pub mod sample;
mod section;
mod stored;

use std::collections::HashSet;
use std::ops::Range;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use section::{citations, fact_changes};
pub use application::{
    Application, InterviewKind, JobEvent, JobEventKind, Nudge, Stage, StepKind,
};

/// Share of the shorter fact's words the longer must contain to be offered as
/// a duplicate.
const DUPLICATE_OVERLAP: f32 = 0.6;

macro_rules! id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }
    };
}

id!(DocumentId);
id!(FactId);
id!(EntryId);
id!(JobId);
id!(ResumeId);
id!(LineId);
id!(EventId);
id!(ClaimId);

/// Everything the app knows about the user, as loaded from the database.
/// Reloaded after every change until views query the database themselves
/// (storage.md, step 4).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    pub documents: Vec<SourceDocument>,
    pub entries: Vec<Entry>,
    pub facts: Vec<Fact>,
    pub jobs: Vec<Job>,
}

impl Library {
    pub fn document(&self, id: DocumentId) -> Option<&SourceDocument> {
        self.documents.iter().find(|document| document.id == id)
    }

    pub fn fact(&self, id: FactId) -> Option<&Fact> {
        self.facts.iter().find(|fact| fact.id == id)
    }

    /// The lines of each job's current résumé that cite the fact.
    pub fn fact_uses(&self, id: FactId) -> Vec<(&Job, &Line)> {
        self.jobs
            .iter()
            .filter_map(|job| job.current_resume().map(|resume| (job, resume)))
            .flat_map(|(job, resume)| {
                resume
                    .lines()
                    .filter(move |line| line.cites.contains(&id))
                    .map(move |line| (job, line))
            })
            .collect()
    }

    /// How the fact differs from the version the résumé was generated from,
    /// if it does.
    pub fn fact_change(&self, resume: &TailoredResume, id: FactId) -> Option<FactChange> {
        let fact = self.fact(id)?;
        let hidden = fact.hidden;
        resume
            .basis
            .iter()
            .filter(|version| version.fact == id)
            .find_map(|version| match (version.hidden, hidden) {
                (false, true) => Some(FactChange::Hidden),
                (true, false) => Some(FactChange::Shown),
                _ if version.text != fact.text => Some(FactChange::Reworded),
                _ => None,
            })
    }

    /// Facts that changed since the résumé was generated from them, so it may
    /// no longer match them.
    pub fn changed_facts(&self, resume: &TailoredResume) -> Vec<FactId> {
        let mut changed: Vec<FactId> = resume
            .basis
            .iter()
            .map(|version| version.fact)
            .filter(|&id| self.fact_change(resume, id).is_some())
            .collect();
        changed.sort();
        changed.dedup();
        changed
    }

    /// "Senior Software Engineer, Northwind Logistics", from the entry's title
    /// and organization facts.
    pub fn entry_label(&self, id: EntryId) -> Option<String> {
        let text_of = |kind: FactKind| {
            self.facts
                .iter()
                .find(|fact| fact.entry == Some(id) && fact.kind == kind)
                .map(|fact| fact.text.as_str())
        };
        let title = text_of(FactKind::Title).or_else(|| text_of(FactKind::SkillGroup));
        match (title, text_of(FactKind::Organization)) {
            (Some(title), Some(organization)) => Some(format!("{title}, {organization}")),
            (Some(label), None) | (None, Some(label)) => Some(label.to_string()),
            (None, None) => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmSettings {
    pub provider: LlmProvider,
    pub model: String,
}

/// The API key is kept in the OS keyring, not here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LlmProvider {
    Anthropic,
    OpenAi,
    Ollama { url: String },
    OpenAiCompatible { url: String },
}

/// Where facts come from: an imported file, or something the user wrote.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceDocument {
    pub id: DocumentId,
    pub kind: DocumentKind,
    /// Plain text; fact spans index into it.
    pub text: String,
    pub added_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocumentKind {
    File { file_name: String },
    Note,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fact {
    pub id: FactId,
    pub kind: FactKind,
    /// The fact as it's used now.
    pub text: String,
    /// The fact as extracted, kept to revert to.
    pub extracted: String,
    /// Where it was found. More than one once duplicates are merged.
    pub sources: Vec<FactSource>,
    /// The role, project or degree it belongs to, if any.
    pub entry: Option<EntryId>,
    /// Never used in generated output.
    pub hidden: bool,
    /// `None`: nothing to check.
    pub review: Option<Review>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactSource {
    pub document: DocumentId,
    /// Byte range in `SourceDocument::text`.
    pub span: Range<usize>,
    /// The verbatim text, which extraction may have cleaned up.
    pub original_text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FactKind {
    Name,
    Bullet,
    Title,
    Organization,
    Date,
    Skill,
    Contact,
    Summary,
    Location,
    /// The name of a group of skills, like "Languages".
    SkillGroup,
}

/// Why a person should look at a fact. Review is the exception: verbatim
/// facts and the user's own notes have none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub reason: ReviewReason,
    /// The details, like "hyphenated across a line break".
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewReason {
    /// The text didn't copy cleanly from the source and was cleaned up.
    Cleaned,
    /// The extraction wasn't sure it read the fact right.
    Unsure,
    /// It may belong to another entry, or be split or joined wrongly.
    Structure,
    /// It may say the same as another fact.
    Duplicate,
}

impl Review {
    /// The note, or else what the reason means.
    pub fn description(&self) -> &str {
        match (&self.note, self.reason) {
            (Some(note), _) => note,
            (None, ReviewReason::Cleaned) => "The text was cleaned up from the source.",
            (None, ReviewReason::Unsure) => "The extraction wasn't sure about it.",
            (None, ReviewReason::Structure) => "It may belong somewhere else.",
            (None, ReviewReason::Duplicate) => "It may repeat another fact.",
        }
    }
}

/// A role, degree or project that groups facts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: EntryId,
    pub kind: EntryKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    Experience,
    Education,
    Project,
    Certification,
    /// A group of skills, titled like "Languages".
    Skills,
    Other,
}

/// One application.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    /// Short summarized title, shown as the grid caption.
    pub title: String,
    pub company: String,
    pub description: String,
    pub url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub overrides: Vec<FactOverride>,
    /// Every generation is kept; the last one is the current version.
    pub resumes: Vec<TailoredResume>,
    pub cover_letter: Option<CoverLetter>,
    pub events: Vec<JobEvent>,
}

impl Job {
    pub fn current_resume(&self) -> Option<&TailoredResume> {
        self.resumes.last()
    }

    pub fn resume(&self, id: ResumeId) -> Option<&TailoredResume> {
        self.resumes.iter().find(|resume| resume.id == id)
    }
}

/// A change to a fact that applies to one job only.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactOverride {
    pub fact: FactId,
    pub inclusion: Option<Inclusion>,
    pub text: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Inclusion {
    Include,
    Exclude,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TailoredResume {
    pub id: ResumeId,
    pub generated_at: DateTime<Utc>,
    /// Key of the template it's laid out with, one of `typeset::TEMPLATES`.
    /// Kept with the résumé, so a version that was sent keeps its look.
    #[serde(default)]
    pub template: String,
    pub sections: Vec<Section>,
    pub unsupported_claims: Vec<UnsupportedClaim>,
    /// The facts as they were when it was generated, to tell when they've
    /// changed since.
    pub basis: Vec<FactVersion>,
}

impl TailoredResume {
    /// Every line, in reading order.
    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.sections
            .iter()
            .flat_map(|section| &section.blocks)
            .flat_map(Block::lines)
    }

    fn lines_mut(&mut self) -> impl Iterator<Item = &mut Line> {
        self.sections
            .iter_mut()
            .flat_map(|section| &mut section.blocks)
            .flat_map(Block::lines_mut)
    }

    pub fn line(&self, id: LineId) -> Option<&Line> {
        self.lines().find(|line| line.id == id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactVersion {
    pub fact: FactId,
    pub text: String,
    pub hidden: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactChange {
    Reworded,
    Hidden,
    /// Shown again after being hidden.
    Shown,
}

/// A part of the résumé. Templates lay sections out by kind: an education
/// entry's school and degree may sit where a job's title and employer would.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Section {
    pub kind: SectionKind,
    /// Empty for the header.
    pub title: String,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SectionKind {
    Header,
    Summary,
    Experience,
    Education,
    Projects,
    Skills,
    Other,
}

/// What templates lay out: the résumé's content with its structure, so the
/// same lines can be set in any design.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Block {
    Header { name: Line, contacts: Vec<Line> },
    /// A role, degree or project.
    Entry(Box<EntryBlock>),
    /// "Languages: Rust, Go, Python".
    Skills { label: Option<Line>, items: Line },
    /// A paragraph, like a summary.
    Text(Line),
    /// An item of a list outside any entry, like a talk.
    Item(Line),
}

impl Block {
    /// The block's lines in reading order.
    pub fn lines(&self) -> Vec<&Line> {
        match self {
            Block::Header { name, contacts } => std::iter::once(name).chain(contacts).collect(),
            Block::Entry(entry) => entry.lines().collect(),
            Block::Skills { label, items } => label.iter().chain([items]).collect(),
            Block::Text(line) | Block::Item(line) => vec![line],
        }
    }

    fn lines_mut(&mut self) -> Vec<&mut Line> {
        match self {
            Block::Header { name, contacts } => std::iter::once(name).chain(contacts).collect(),
            Block::Entry(entry) => [
                Some(&mut entry.title),
                entry.stack.as_mut(),
                entry.dates.as_mut(),
                entry.organization.as_mut(),
                entry.location.as_mut(),
            ]
            .into_iter()
            .flatten()
            .chain(&mut entry.bullets)
            .collect(),
            Block::Skills { label, items } => label.iter_mut().chain([items]).collect(),
            Block::Text(line) | Block::Item(line) => vec![line],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntryBlock {
    /// A job title, a degree or a project's name.
    pub title: Line,
    /// The employer or school.
    pub organization: Option<Line>,
    pub dates: Option<Line>,
    pub location: Option<Line>,
    /// What a project was built with: "Rust, Tokio".
    pub stack: Option<Line>,
    pub bullets: Vec<Line>,
}

impl EntryBlock {
    /// In the order of `Block::lines_mut`.
    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        [
            Some(&self.title),
            self.stack.as_ref(),
            self.dates.as_ref(),
            self.organization.as_ref(),
            self.location.as_ref(),
        ]
        .into_iter()
        .flatten()
        .chain(&self.bullets)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoverLetter {
    pub generated_at: DateTime<Utc>,
    pub paragraphs: Vec<Vec<Line>>,
    pub unsupported_claims: Vec<UnsupportedClaim>,
}

/// A generated line. `cites` is never empty.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Line {
    /// Stays the same through edits and reordering, so selections, the page's
    /// click map and undo history can refer to the line.
    pub id: LineId,
    /// What the generator wrote, kept to revert to and compare against.
    pub generated: String,
    /// What's on the page: the generated text, or the user's edit of it.
    pub text: String,
    pub cites: Vec<FactId>,
}

/// A claim the generator wanted to make but couldn't back with a fact, so it
/// was left out. The user can add it as a fact and regenerate.
///
/// Not called a "refusal": in a job app that reads as a rejection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnsupportedClaim {
    pub id: ClaimId,
    pub text: String,
    pub reason: UnsupportedReason,
    /// The fact the user added from it, if they did.
    #[serde(default)]
    pub added_as: Option<FactId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnsupportedReason {
    InventedFigure(String),
    InventedYear(u16),
    NoSupportingFact,
    UsesHiddenFact(FactId),
}

/// Tells whether another fact probably says the same thing as `text`: most
/// of the shorter one's words appear in the other.
pub fn possible_duplicate(text: &str) -> impl Fn(&str) -> bool {
    let words = significant_words(text);
    move |other| {
        let other_words = significant_words(other);
        let shared = words.intersection(&other_words).count();
        let shorter = words.len().min(other_words.len());
        shorter > 0 && shared as f32 / shorter as f32 >= DUPLICATE_OVERLAP
    }
}

fn significant_words(text: &str) -> HashSet<String> {
    const COMMON: [&str; 8] = ["the", "and", "for", "from", "with", "that", "into", "their"];
    text.split(|character: char| !character.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|word| {
            (word.len() >= 3 || word.chars().any(|character| character.is_ascii_digit()))
                && !COMMON.contains(&word.as_str())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Library {
        match sample::library() {
            Ok(library) => library,
            Err(error) => panic!("{error}"),
        }
    }

    fn fact_text<'a>(library: &'a Library, start: &str) -> &'a str {
        match library.facts.iter().find(|fact| fact.text.starts_with(start)) {
            Some(fact) => &fact.text,
            None => panic!("no fact starts with {start:?}"),
        }
    }

    #[test]
    fn the_sample_linkedin_bullet_is_a_possible_duplicate() {
        let library = sample();
        let migration = fact_text(&library, "Led the migration");
        let linkedin = fact_text(&library, "Migrated 12 backend");
        let latency = fact_text(&library, "Cut p99 latency");
        assert!(possible_duplicate(migration)(linkedin));
        assert!(possible_duplicate(linkedin)(migration));
        assert!(!possible_duplicate(latency)(linkedin));
        assert!(!possible_duplicate(latency)(migration));
    }
}
