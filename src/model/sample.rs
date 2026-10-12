// Loads the sample library in `assets/sample/` for building the UI before
// import and generation exist. The data file's header explains its format.

use std::collections::HashMap;

use chrono::{Duration, Utc};
use serde::Deserialize;

use crate::model::{
    DocumentId, DocumentKind, Entry, EntryId, EntryKind, Extraction, Fact, FactId, FactKind,
    FactSource, FactStatus, FactVersion, Job, JobEvent, JobEventKind, JobId, Library, Line, LineId, ResumeId,
    Section, SourceDocument, TailoredResume, UnsupportedClaim, UnsupportedReason,
};

const LIBRARY: &str = include_str!("../../assets/sample/library.ron");
/// Documents too long to write inline, by `text_file`.
const TEXT_FILES: [(&str, &str); 2] = [
    ("cv.txt", include_str!("../../assets/sample/cv.txt")),
    ("linkedin.txt", include_str!("../../assets/sample/linkedin.txt")),
];
const DEFAULT_DOCUMENT: &str = "cv";

pub fn library() -> Result<Library, String> {
    let data: SampleData =
        ron::from_str(LIBRARY).map_err(|error| format!("sample library: {error}"))?;
    let mut loader = Loader {
        library: Library::default(),
        documents: HashMap::new(),
        facts: HashMap::new(),
        headings: HashMap::new(),
    };
    for document in data.documents {
        loader.add_document(document)?;
    }
    for fact in data.facts {
        loader.add_fact(fact, None, None, None)?;
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

struct Loader {
    library: Library,
    /// Each document's id, by key.
    documents: HashMap<String, DocumentId>,
    /// Each fact's id and extracted text, by key. Résumés are generated from
    /// the extracted text: edits in the sample came after generation.
    facts: HashMap<String, (FactId, String)>,
    /// Each entry's heading line and the facts it cites, by key.
    headings: HashMap<String, (String, Vec<FactId>)>,
}

impl Loader {
    fn add_document(&mut self, document: SampleDocument) -> Result<(), String> {
        let text = match (document.text, document.text_file) {
            (Some(text), None) => text,
            (None, Some(file)) => TEXT_FILES
                .iter()
                .find(|(name, _)| *name == file)
                .map(|(_, text)| text.to_string())
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

    /// Where `text` is in the document called `key`.
    fn source(&self, key: &str, text: &str) -> Result<FactSource, String> {
        let document = self
            .documents
            .get(key)
            .and_then(|&id| self.library.document(id))
            .ok_or_else(|| format!("no document is called {key:?}"))?;
        let start = document
            .text
            .find(text)
            .ok_or_else(|| format!("{text:?} isn't in document {key:?}"))?;
        Ok(FactSource {
            document: document.id,
            span: start..start + text.len(),
            original_text: text.to_string(),
        })
    }

    /// `from` and `status` default to the entry's, when the fact is in one.
    fn add_fact(
        &mut self,
        fact: SampleFact,
        entry: Option<EntryId>,
        from: Option<&str>,
        status: Option<FactStatus>,
    ) -> Result<FactId, String> {
        let from = fact.from.as_deref().or(from).unwrap_or(DEFAULT_DOCUMENT);
        let original_text = fact.source.unwrap_or_else(|| fact.text.clone());
        let mut sources = vec![self.source(from, &original_text)?];
        for key in &fact.also_in {
            sources.push(self.source(key, &fact.text)?);
        }
        let id = FactId::new();
        self.library.facts.push(Fact {
            id,
            kind: fact.kind,
            text: fact.edited.unwrap_or_else(|| fact.text.clone()),
            extracted: fact.text.clone(),
            sources,
            entry,
            extraction: match fact.flag {
                Some(reason) => Extraction::Flagged { reason },
                None => Extraction::Clean,
            },
            status: fact.status.or(status).unwrap_or(FactStatus::Confirmed),
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

        let mut heading = entry.title.clone();
        let mut cites = vec![self.add_fact(
            SampleFact::new(FactKind::Title, &entry.title),
            Some(id),
            from,
            status,
        )?];
        if let Some(organization) = entry.organization {
            heading.push_str(&format!(", {organization}"));
            cites.push(self.add_fact(
                SampleFact::new(FactKind::Organization, &organization),
                Some(id),
                from,
                status,
            )?);
        }
        if let Some(dates) = entry.dates {
            heading.push_str(&format!(" · {dates}"));
            cites.push(self.add_fact(
                SampleFact::new(FactKind::Date, &dates),
                Some(id),
                from,
                status,
            )?);
        }
        self.headings.insert(entry.key, (heading, cites));

        for bullet in entry.bullets {
            self.add_fact(bullet, Some(id), from, status)?;
        }
        Ok(())
    }

    fn job(&self, job: SampleJob) -> Result<Job, String> {
        let resumes = match job.resume {
            Some(resume) => vec![self.resume(resume)?],
            None => Vec::new(),
        };
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
            events: job
                .events
                .into_iter()
                .map(SampleEvent::into_event)
                .collect(),
        })
    }

    fn resume(&self, resume: SampleResume) -> Result<TailoredResume, String> {
        let sections: Vec<Section> = resume
            .sections
            .into_iter()
            .map(|section| {
                Ok(Section {
                    title: section.title,
                    lines: section
                        .lines
                        .into_iter()
                        .map(|line| self.line(line))
                        .collect::<Result<_, String>>()?,
                })
            })
            .collect::<Result<_, String>>()?;
        let unsupported_claims: Vec<UnsupportedClaim> = resume
            .unsupported
            .into_iter()
            .map(|claim| {
                Ok(UnsupportedClaim {
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
            .flat_map(|section| &section.lines)
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
            generated_at: Utc::now() - Duration::hours(resume.generated_hours_ago),
            sections,
            unsupported_claims,
            basis,
        })
    }

    fn line(&self, line: SampleLine) -> Result<Line, String> {
        let (generated, text, cites) = match line {
            SampleLine::Fact(key) => {
                let (id, text) = self.fact(&key)?;
                (text.clone(), text.clone(), vec![*id])
            }
            SampleLine::Heading(key) => {
                let (heading, cites) = self
                    .headings
                    .get(&key)
                    .ok_or_else(|| format!("no entry is called {key:?}"))?;
                (heading.clone(), heading.clone(), cites.clone())
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
    status: Option<FactStatus>,
    /// Why the extraction was flagged for a look.
    #[serde(default)]
    flag: Option<String>,
}

impl SampleFact {
    /// A fact without a key, for an entry's title, organization and dates.
    fn new(kind: FactKind, text: &str) -> Self {
        Self {
            key: format!("{kind:?}: {text}"),
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
    status: Option<FactStatus>,
    #[serde(default)]
    organization: Option<String>,
    #[serde(default)]
    dates: Option<String>,
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
    #[serde(default)]
    resume: Option<SampleResume>,
}

#[derive(Deserialize)]
struct SampleEvent {
    days_ago: i64,
    kind: SampleEventKind,
}

#[derive(Deserialize)]
enum SampleEventKind {
    Applied,
    RecruiterReplied,
    InterviewScheduled { in_days: i64 },
    Rejected,
    Offer,
    Withdrawn,
    Note(String),
}

impl SampleEvent {
    fn into_event(self) -> JobEvent {
        let kind = match self.kind {
            SampleEventKind::Applied => JobEventKind::Applied,
            SampleEventKind::RecruiterReplied => JobEventKind::RecruiterReplied,
            SampleEventKind::InterviewScheduled { in_days } => JobEventKind::InterviewScheduled {
                at: Utc::now() + Duration::days(in_days),
            },
            SampleEventKind::Rejected => JobEventKind::Rejected,
            SampleEventKind::Offer => JobEventKind::Offer,
            SampleEventKind::Withdrawn => JobEventKind::Withdrawn,
            SampleEventKind::Note(note) => JobEventKind::Note(note),
        };
        JobEvent {
            at: Utc::now() - Duration::days(self.days_ago),
            kind,
        }
    }
}

#[derive(Deserialize)]
struct SampleResume {
    generated_hours_ago: i64,
    sections: Vec<SampleSection>,
    #[serde(default)]
    unsupported: Vec<SampleClaim>,
}

#[derive(Deserialize)]
struct SampleSection {
    #[serde(default)]
    title: String,
    lines: Vec<SampleLine>,
}

#[derive(Deserialize)]
enum SampleLine {
    Fact(String),
    Heading(String),
    Reworded { text: String, cites: Vec<String> },
    Edited { fact: String, text: String },
}

#[derive(Deserialize)]
struct SampleClaim {
    text: String,
    reason: SampleReason,
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

#[cfg(test)]
mod tests {
    #[test]
    fn sample_library_loads() {
        if let Err(error) = super::library() {
            panic!("{error}");
        }
    }
}
