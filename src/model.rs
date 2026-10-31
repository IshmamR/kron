mod application;
pub mod commands;
pub mod extraction;
pub mod redaction;
#[cfg(any(debug_assertions, test))]
pub mod sample;
mod section;
pub mod settings;
mod stored;
pub mod verifier;

use std::collections::HashSet;

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
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

#[derive(Clone, Debug)]
pub struct LlmSettings {
    pub provider: LlmProvider,
    pub model: String,
}

/// The API key is kept in the OS keyring, not here.
#[derive(Clone, Debug)]
pub enum LlmProvider {
    Anthropic,
    OpenAi,
    Ollama { url: String },
    OpenAiCompatible { url: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub reason: ReviewReason,
    /// The details, like "hyphenated across a line break".
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Experience,
    Education,
    Project,
    Certification,
    /// A group of skills, titled like "Languages".
    Skills,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inclusion {
    Include,
    Exclude,
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
#[derive(Clone, Debug)]
pub struct Section {
    pub kind: SectionKind,
    /// Empty for the header.
    pub title: String,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug)]
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
}

#[derive(Clone, Debug)]
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

/// A generated line. `cites` is never empty.
#[derive(Clone, Debug)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
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

    use crate::database::Error;

    fn fact_text(connection: &rusqlite::Connection, start: &str) -> Result<String, Error> {
        let fact = sample::fact(connection, start)?;
        Ok(connection.query_row("SELECT text FROM facts WHERE id = ?1", [fact.0], |row| row.get(0))?)
    }

    #[test]
    fn the_sample_linkedin_bullet_is_a_possible_duplicate() -> Result<(), Error> {
        let writer = sample::written()?;
        let migration = fact_text(writer.kron(), "Led the migration")?;
        let linkedin = fact_text(writer.kron(), "Migrated 12 backend")?;
        let latency = fact_text(writer.kron(), "Cut p99 latency")?;
        assert!(possible_duplicate(&migration)(&linkedin));
        assert!(possible_duplicate(&linkedin)(&migration));
        assert!(!possible_duplicate(&latency)(&linkedin));
        assert!(!possible_duplicate(&latency)(&migration));
        Ok(())
    }
}
