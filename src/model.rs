mod application;
pub mod sample;

use std::collections::HashSet;
use std::ops::Range;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use application::{
    InterviewKind, JobEvent, JobEventKind, Nudge, Stage, StepKind,
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

/// Everything the app knows about the user. Held in memory for now.
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

    fn fact_mut(&mut self, id: FactId) -> Option<&mut Fact> {
        self.facts.iter_mut().find(|fact| fact.id == id)
    }

    pub fn entry(&self, id: EntryId) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn job(&self, id: JobId) -> Option<&Job> {
        self.jobs.iter().find(|job| job.id == id)
    }

    pub fn job_mut(&mut self, id: JobId) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|job| job.id == id)
    }

    /// Saves what the user wrote as a note and extracts facts from it: one
    /// per line, without bullet markers. Lines written together are grouped
    /// as one entry. Facts start unreviewed.
    pub fn add_note(&mut self, text: &str) -> Vec<FactId> {
        let document = DocumentId::new();
        let mut facts = Vec::new();
        let mut line_start = 0;
        for line in text.split_inclusive('\n') {
            let trimmed = line
                .trim()
                .trim_start_matches(['-', '•', '*'])
                .trim_start();
            if let Some(offset) = line.find(trimmed).filter(|_| !trimmed.is_empty()) {
                let start = line_start + offset;
                facts.push(Fact::new(
                    FactKind::Bullet,
                    trimmed,
                    FactSource {
                        document,
                        span: start..start + trimmed.len(),
                        original_text: trimmed.to_string(),
                    },
                ));
            }
            line_start += line.len();
        }
        if facts.is_empty() {
            return Vec::new();
        }
        self.documents.push(SourceDocument {
            id: document,
            kind: DocumentKind::Note,
            text: text.to_string(),
            added_at: Utc::now(),
        });
        if facts.len() > 1 {
            let entry = EntryId::new();
            self.entries.push(Entry {
                id: entry,
                kind: EntryKind::Other,
            });
            for fact in &mut facts {
                fact.entry = Some(entry);
            }
        }
        let ids = facts.iter().map(|fact| fact.id).collect();
        self.facts.extend(facts);
        ids
    }

    /// Turns the job's `index`th unsupported claim into a note, and so a fact
    /// the user reviews before it's used.
    pub fn add_unsupported_claim_as_fact(&mut self, job: JobId, index: usize) -> Vec<FactId> {
        let Some(resume) = self.job(job).and_then(Job::current_resume) else {
            return Vec::new();
        };
        let Some(claim) = resume
            .unsupported_claims
            .get(index)
            .filter(|claim| claim.added_as.is_none())
        else {
            return Vec::new();
        };
        let text = claim.text.clone();
        let added = self.add_note(&text);
        if let Some(claim) = self
            .job_mut(job)
            .and_then(|job| job.resumes.last_mut())
            .and_then(|resume| resume.unsupported_claims.get_mut(index))
        {
            claim.added_as = added.first().copied();
        }
        added
    }

    pub fn set_fact_status(&mut self, id: FactId, status: FactStatus) {
        let Some(fact) = self.fact_mut(id) else {
            return;
        };
        // Confirming says the text came out right.
        if status == FactStatus::Confirmed {
            fact.extraction = Extraction::Clean;
        }
        fact.status = status;
    }

    /// Rewording a fact is reviewing it, so it becomes confirmed, and its
    /// text is no longer as extracted, so no longer flagged.
    pub fn edit_fact(&mut self, id: FactId, text: &str) {
        let text = text.trim();
        let Some(fact) = self.fact_mut(id) else {
            return;
        };
        if text.is_empty() || text == fact.text {
            return;
        }
        fact.text = text.to_string();
        fact.status = FactStatus::Confirmed;
        fact.extraction = Extraction::Clean;
    }

    pub fn revert_fact(&mut self, id: FactId) {
        if let Some(fact) = self.fact_mut(id).filter(|fact| fact.is_edited()) {
            fact.text = fact.extracted.clone();
        }
    }

    /// Folds `others` into `keep`, which keeps its wording and gains their
    /// sources. Everything that cited them cites `keep` instead.
    pub fn merge_facts(&mut self, keep: FactId, others: &[FactId]) {
        let others: Vec<FactId> = others.iter().copied().filter(|&id| id != keep).collect();
        if others.is_empty() || self.fact(keep).is_none() {
            return;
        }
        let (merged, facts): (Vec<Fact>, Vec<Fact>) = std::mem::take(&mut self.facts)
            .into_iter()
            .partition(|fact| others.contains(&fact.id));
        self.facts = facts;
        let Some(fact) = self.fact_mut(keep) else {
            return;
        };
        for other in merged {
            fact.sources.extend(other.sources);
            fact.entry = fact.entry.or(other.entry);
        }
        // Merging is reviewing.
        fact.status = FactStatus::Confirmed;
        fact.extraction = Extraction::Clean;

        let replace = |id: &mut FactId| {
            if others.contains(id) {
                *id = keep;
            }
        };
        for job in &mut self.jobs {
            for fact_override in &mut job.overrides {
                replace(&mut fact_override.fact);
            }
            for resume in &mut job.resumes {
                for version in &mut resume.basis {
                    replace(&mut version.fact);
                }
                for line in resume.lines_mut() {
                    line.cites.iter_mut().for_each(replace);
                    let mut seen = Vec::new();
                    line.cites.retain(|&id| {
                        let first = !seen.contains(&id);
                        seen.push(id);
                        first
                    });
                }
                for claim in &mut resume.unsupported_claims {
                    if let UnsupportedReason::UsesHiddenFact(id) = &mut claim.reason {
                        replace(id);
                    }
                }
            }
        }
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

    /// Facts that probably say the same thing as `id`: most of the shorter
    /// one's words appear in the other.
    pub fn possible_duplicates(&self, id: FactId) -> Vec<FactId> {
        let Some(fact) = self.fact(id) else {
            return Vec::new();
        };
        let words = significant_words(&fact.text);
        self.facts
            .iter()
            .filter(|other| other.id != id && other.kind == fact.kind)
            .filter(|other| {
                let other_words = significant_words(&other.text);
                let shared = words.intersection(&other_words).count();
                let shorter = words.len().min(other_words.len());
                shorter > 0 && shared as f32 / shorter as f32 >= DUPLICATE_OVERLAP
            })
            .map(|other| other.id)
            .collect()
    }

    /// How the fact differs from the version the résumé was generated from,
    /// if it does.
    pub fn fact_change(&self, resume: &TailoredResume, id: FactId) -> Option<FactChange> {
        let fact = self.fact(id)?;
        let hidden = fact.status == FactStatus::Hidden;
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

    /// Lays the job's current résumé out with another template. Its content
    /// stays as it is.
    pub fn set_template(&mut self, job: JobId, template: &str) {
        if let Some(resume) = self.job_mut(job).and_then(|job| job.resumes.last_mut()) {
            resume.template = template.to_string();
        }
    }

    pub fn unreviewed_count(&self) -> usize {
        self.facts
            .iter()
            .filter(|fact| fact.status == FactStatus::Unreviewed)
            .count()
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
    pub extraction: Extraction,
    pub status: FactStatus,
}

impl Fact {
    pub fn new(kind: FactKind, text: &str, source: FactSource) -> Self {
        Self {
            id: FactId::new(),
            kind,
            text: text.to_string(),
            extracted: text.to_string(),
            sources: vec![source],
            entry: None,
            extraction: Extraction::Clean,
            status: FactStatus::Unreviewed,
        }
    }

    pub fn is_edited(&self) -> bool {
        self.text != self.extracted
    }
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Extraction {
    Clean,
    /// The text didn't copy cleanly from the source and needs a look.
    Flagged {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FactStatus {
    Unreviewed,
    Confirmed,
    /// Never used in generated output.
    Hidden,
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

impl Line {
    pub fn is_edited(&self) -> bool {
        self.text != self.generated
    }
}

/// A claim the generator wanted to make but couldn't back with a fact, so it
/// was left out. The user can add it as a fact and regenerate.
///
/// Not called a "refusal": in a job app that reads as a rejection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnsupportedClaim {
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

    fn fact_with_text(library: &Library, start: &str) -> FactId {
        match library.facts.iter().find(|fact| fact.text.starts_with(start)) {
            Some(fact) => fact.id,
            None => panic!("no fact starts with {start:?}"),
        }
    }

    #[test]
    fn a_note_becomes_one_fact_per_line_grouped_together() {
        let mut library = Library::default();
        let added = library.add_note("- Organized Code Fest 2019.\n\n  • Ran a team of 15.\n");
        assert_eq!(added.len(), 2);
        let facts: Vec<&Fact> = added.iter().filter_map(|&id| library.fact(id)).collect();
        assert_eq!(facts[0].text, "Organized Code Fest 2019.");
        assert_eq!(facts[1].text, "Ran a team of 15.");
        assert!(facts[0].entry.is_some() && facts[0].entry == facts[1].entry);
        let document = &library.documents[0];
        for fact in facts {
            assert_eq!(fact.status, FactStatus::Unreviewed);
            assert_eq!(&document.text[fact.sources[0].span.clone()], fact.text);
        }
    }

    #[test]
    fn a_one_line_note_needs_no_entry() {
        let mut library = Library::default();
        let added = library.add_note("Gave a guest lecture.");
        assert_eq!(library.fact(added[0]).and_then(|fact| fact.entry), None);
        assert!(library.add_note("  \n").is_empty());
    }

    #[test]
    fn the_sample_linkedin_bullet_is_a_possible_duplicate() {
        let library = sample();
        let migration = fact_with_text(&library, "Led the migration");
        let linkedin = fact_with_text(&library, "Migrated 12 backend");
        assert_eq!(library.possible_duplicates(migration), vec![linkedin]);
        let latency = fact_with_text(&library, "Cut p99 latency");
        assert!(library.possible_duplicates(latency).is_empty());
    }

    #[test]
    fn merging_moves_sources_and_citations_to_the_kept_fact() {
        let mut library = sample();
        let keep = fact_with_text(&library, "Migrated 12 backend");
        let merged = fact_with_text(&library, "Led the migration");
        let uses = library.fact_uses(merged).len();
        assert!(uses > 0);

        library.merge_facts(keep, &[keep, merged]);

        assert!(library.fact(merged).is_none());
        let Some(fact) = library.fact(keep) else {
            panic!("the kept fact is gone");
        };
        assert_eq!(fact.sources.len(), 2);
        assert_eq!(fact.status, FactStatus::Confirmed);
        assert_eq!(library.fact_uses(keep).len(), uses);
    }

    #[test]
    fn editing_or_hiding_a_fact_outdates_the_resumes_that_use_it() {
        let mut library = sample();
        let ledgerly = |library: &Library| {
            library
                .jobs
                .iter()
                .find(|job| job.company == "Ledgerly")
                .and_then(Job::current_resume)
                .map(|resume| library.changed_facts(resume))
        };
        assert_eq!(ledgerly(&library), Some(Vec::new()));

        let latency = fact_with_text(&library, "Cut p99 latency");
        library.set_fact_status(latency, FactStatus::Confirmed);
        assert_eq!(ledgerly(&library), Some(Vec::new()), "confirming changes nothing");

        library.edit_fact(latency, "Cut p99 latency of the routing API by 80%.");
        assert_eq!(ledgerly(&library), Some(vec![latency]));
        library.revert_fact(latency);
        assert_eq!(library.fact(latency).map(Fact::is_edited), Some(false));
        assert_eq!(ledgerly(&library), Some(Vec::new()), "reverting undoes the change");

        library.set_fact_status(latency, FactStatus::Hidden);
        assert_eq!(ledgerly(&library), Some(vec![latency]));
        library.set_fact_status(latency, FactStatus::Confirmed);
        assert_eq!(ledgerly(&library), Some(Vec::new()), "showing it again undoes hiding");
    }

    #[test]
    fn showing_a_hidden_fact_a_resume_needed_outdates_it() {
        let mut library = sample();
        let property_tests = fact_with_text(&library, "Introduced property-based tests");
        library.set_fact_status(property_tests, FactStatus::Confirmed);
        let Some(resume) = library
            .jobs
            .iter()
            .find(|job| job.company == "Ledgerly")
            .and_then(Job::current_resume)
        else {
            panic!("no Ledgerly résumé");
        };
        assert_eq!(
            library.fact_change(resume, property_tests),
            Some(FactChange::Shown)
        );
    }

    #[test]
    fn adding_a_claim_as_a_fact_keeps_the_claim_to_point_at_it() {
        let mut library = sample();
        let Some(job) = library.jobs.first().map(|job| job.id) else {
            panic!("no jobs");
        };
        let added = library.add_unsupported_claim_as_fact(job, 0);
        assert_eq!(added.len(), 1);
        let claim = library
            .job(job)
            .and_then(Job::current_resume)
            .and_then(|resume| resume.unsupported_claims.first());
        assert_eq!(claim.and_then(|claim| claim.added_as), added.first().copied());
        assert!(library.add_unsupported_claim_as_fact(job, 0).is_empty(), "only once");
    }
}
