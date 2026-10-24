// Builds the `Library` from the database, for views that still read it. Goes
// away with `Library` (storage.md, step 5).
//
// Merged facts are left out: their sources, citations and overrides count
// for the fact they were merged into, as when merging removed them.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, Row};
use uuid::Uuid;

use super::{
    Block, ClaimId, CoverLetter, DocumentId, DocumentKind, Entry, EntryBlock, EntryId, EntryKind,
    Fact, FactId, FactKind, FactOverride, FactSource, FactVersion, Inclusion, InterviewKind, Job,
    JobEvent, JobEventKind, JobId, Library, Line, LineId, ResumeId, Review, ReviewReason, Section,
    SectionKind, SourceDocument, TailoredResume, UnsupportedClaim, UnsupportedReason,
};
use crate::database::Error;
use crate::typeset;

/// Longer merge chains than this are a loop, which merging never makes.
const MERGE_DEPTH: usize = 64;

impl Library {
    pub fn load(connection: &Connection) -> Result<Self, Error> {
        let mut library = Library {
            documents: documents(connection)?,
            entries: entries(connection)?,
            ..Library::default()
        };

        let (facts, merged_into) = facts(connection)?;
        let target = |id: Uuid| -> FactId {
            let mut id = id;
            for _ in 0..MERGE_DEPTH {
                match merged_into.get(&id) {
                    Some(&into) => id = into,
                    None => break,
                }
            }
            FactId(id)
        };
        library.facts = facts;
        let by_id: HashMap<FactId, usize> = library
            .facts
            .iter()
            .enumerate()
            .map(|(index, fact)| (fact.id, index))
            .collect();
        for (fact, source) in query(
            connection,
            "SELECT fact_id, document_id, span_start, span_end, original_text
             FROM fact_sources WHERE deleted_at IS NULL ORDER BY id",
            |row| {
                Ok((
                    row.get::<_, Uuid>(0)?,
                    FactSource {
                        document: DocumentId(row.get(1)?),
                        span: offset(row.get(2)?)..offset(row.get(3)?),
                        original_text: row.get(4)?,
                    },
                ))
            },
        )? {
            if let Some(&index) = by_id.get(&target(fact)) {
                library.facts[index].sources.push(source);
            }
        }

        library.jobs = jobs(connection, &target)?;
        Ok(library)
    }
}

fn documents(connection: &Connection) -> Result<Vec<SourceDocument>, Error> {
    query(
        connection,
        "SELECT documents.id, file_name, added_at, text
         FROM documents JOIN document_texts ON document_texts.document_id = documents.id
         WHERE deleted_at IS NULL ORDER BY added_at, documents.id",
        |row| {
            Ok(SourceDocument {
                id: DocumentId(row.get(0)?),
                kind: match row.get::<_, Option<String>>(1)? {
                    Some(file_name) => DocumentKind::File { file_name },
                    None => DocumentKind::Note,
                },
                added_at: time(row.get(2)?),
                text: row.get(3)?,
            })
        },
    )
}

fn entries(connection: &Connection) -> Result<Vec<Entry>, Error> {
    query(
        connection,
        "SELECT id, kind FROM entries WHERE deleted_at IS NULL ORDER BY id",
        |row| {
            let kind: String = row.get(1)?;
            Ok(Entry {
                id: EntryId(row.get(0)?),
                // `profile` has no kind in the model yet.
                kind: EntryKind::from_stored(&kind).unwrap_or(EntryKind::Other),
            })
        },
    )
}

/// The facts not merged into another, and where each merged one went.
fn facts(connection: &Connection) -> Result<(Vec<Fact>, HashMap<Uuid, Uuid>), Error> {
    let rows = query(
        connection,
        "SELECT id, kind, text, extracted, entry_id, hidden, review, review_note, merged_into
         FROM facts WHERE deleted_at IS NULL ORDER BY id",
        |row| {
            let kind: String = row.get(1)?;
            let review: Option<String> = row.get(6)?;
            Ok((
                Fact {
                    id: FactId(row.get(0)?),
                    kind: stored(FactKind::from_stored(&kind), "facts.kind", &kind)?,
                    text: row.get(2)?,
                    extracted: row.get(3)?,
                    sources: Vec::new(),
                    entry: row.get::<_, Option<Uuid>>(4)?.map(EntryId),
                    hidden: row.get(5)?,
                    review: match review {
                        Some(reason) => Some(Review {
                            reason: stored(
                                ReviewReason::from_stored(&reason),
                                "facts.review",
                                &reason,
                            )?,
                            note: row.get(7)?,
                        }),
                        None => None,
                    },
                },
                row.get::<_, Option<Uuid>>(8)?,
            ))
        },
    )?;
    let mut facts = Vec::new();
    let mut merged_into = HashMap::new();
    for (fact, into) in rows {
        match into {
            Some(into) => {
                merged_into.insert(fact.id.0, into);
            }
            None => facts.push(fact),
        }
    }
    Ok((facts, merged_into))
}

fn jobs(connection: &Connection, target: &dyn Fn(Uuid) -> FactId) -> Result<Vec<Job>, Error> {
    let mut jobs = query(
        connection,
        "SELECT jobs.id, title, company, url, created_at, text
         FROM jobs JOIN job_descriptions ON job_descriptions.job_id = jobs.id
         WHERE deleted_at IS NULL ORDER BY board_sort_key, jobs.id",
        |row| {
            Ok(Job {
                id: JobId(row.get(0)?),
                title: row.get(1)?,
                company: row.get(2)?,
                url: row.get(3)?,
                created_at: time(row.get(4)?),
                description: row.get(5)?,
                overrides: Vec::new(),
                resumes: Vec::new(),
                cover_letter: None,
                events: Vec::new(),
            })
        },
    )?;
    let index: HashMap<Uuid, usize> = jobs
        .iter()
        .enumerate()
        .map(|(index, job)| (job.id.0, index))
        .collect();

    for (job, fact_override) in query(
        connection,
        "SELECT job_id, fact_id, inclusion, text FROM fact_overrides
         WHERE deleted_at IS NULL ORDER BY job_id, fact_id",
        |row| {
            let inclusion: Option<String> = row.get(2)?;
            Ok((
                row.get::<_, Uuid>(0)?,
                FactOverride {
                    fact: target(row.get(1)?),
                    inclusion: match inclusion {
                        Some(inclusion) => Some(stored(
                            Inclusion::from_stored(&inclusion),
                            "fact_overrides.inclusion",
                            &inclusion,
                        )?),
                        None => None,
                    },
                    text: row.get(3)?,
                },
            ))
        },
    )? {
        if let Some(&job) = index.get(&job) {
            jobs[job].overrides.push(fact_override);
        }
    }

    for (job, generation) in generations(connection, target)? {
        let Some(&job) = index.get(&job) else {
            continue;
        };
        match generation {
            Generation::Resume(resume) => jobs[job].resumes.push(resume),
            Generation::CoverLetter(letter) => jobs[job].cover_letter = Some(letter),
        }
    }

    // The steps the model doesn't have yet (wishlisted, tailoring, ready)
    // are left out: without them a job hasn't been applied to.
    for (job, event) in query(
        connection,
        "SELECT id, job_id, at, kind, resume_id, interview_kind, interview_with, note
         FROM events
         WHERE deleted_at IS NULL AND kind NOT IN ('wishlisted', 'tailoring', 'ready')
         ORDER BY id",
        |row| {
            let kind: String = row.get(3)?;
            let kind = match kind.as_str() {
                "applied" => JobEventKind::Applied {
                    resume: row.get::<_, Option<Uuid>>(4)?.map(ResumeId),
                },
                "recruiter_replied" => JobEventKind::RecruiterReplied,
                "interview" => {
                    let interview: String = row.get(5)?;
                    JobEventKind::Interview {
                        kind: stored(
                            InterviewKind::from_stored(&interview),
                            "events.interview_kind",
                            &interview,
                        )?,
                        with: row.get(6)?,
                    }
                }
                "offer" => JobEventKind::Offer,
                "rejected" => JobEventKind::Rejected,
                "withdrawn" => JobEventKind::Withdrawn,
                "no_response" => JobEventKind::NoResponse,
                "note" => JobEventKind::Note,
                other => return Err(unknown("events.kind", other)),
            };
            Ok((
                row.get::<_, Uuid>(1)?,
                JobEvent {
                    id: super::EventId(row.get(0)?),
                    at: time(row.get(2)?),
                    kind,
                    note: row.get(7)?,
                },
            ))
        },
    )? {
        if let Some(&job) = index.get(&job) {
            jobs[job].events.push(event);
        }
    }
    Ok(jobs)
}

/// A line and its role in its block, like "title" or "bullet".
type RoledLine = (String, Line);

enum Generation {
    Resume(TailoredResume),
    CoverLetter(CoverLetter),
}

/// Each job's generations, oldest first, with their sections, lines and
/// claims.
fn generations(
    connection: &Connection,
    target: &dyn Fn(Uuid) -> FactId,
) -> Result<Vec<(Uuid, Generation)>, Error> {
    let mut citations: HashMap<Uuid, Vec<FactId>> = HashMap::new();
    for (line, fact) in query(
        connection,
        "SELECT line_id, fact_id FROM line_citations ORDER BY line_id, sort_key",
        |row| Ok((row.get::<_, Uuid>(0)?, row.get::<_, Uuid>(1)?)),
    )? {
        let cites = citations.entry(line).or_default();
        let fact = target(fact);
        // Two facts it cites may have been merged into one.
        if !cites.contains(&fact) {
            cites.push(fact);
        }
    }

    // Each block's lines with their roles, in order.
    let mut lines: HashMap<Uuid, Vec<RoledLine>> = HashMap::new();
    for (block, role, line) in query(
        connection,
        "SELECT id, block_id, role, generated, text FROM lines
         WHERE deleted_at IS NULL ORDER BY block_id, sort_key",
        |row| {
            Ok((
                row.get::<_, Uuid>(1)?,
                row.get::<_, String>(2)?,
                (row.get::<_, Uuid>(0)?, row.get(3)?, row.get(4)?),
            ))
        },
    )? {
        let (id, generated, text) = line;
        lines.entry(block).or_default().push((
            role,
            Line {
                id: LineId(id),
                generated,
                text,
                cites: citations.remove(&id).unwrap_or_default(),
            },
        ));
    }

    // Each section's blocks: their kind and lines.
    let mut blocks: HashMap<Uuid, Vec<(String, Vec<RoledLine>)>> = HashMap::new();
    for (section, id, kind) in query(
        connection,
        "SELECT id, section_id, kind FROM blocks WHERE deleted_at IS NULL
         ORDER BY section_id, sort_key",
        |row| {
            Ok((
                row.get::<_, Uuid>(1)?,
                row.get::<_, Uuid>(0)?,
                row.get::<_, String>(2)?,
            ))
        },
    )? {
        blocks
            .entry(section)
            .or_default()
            .push((kind, lines.remove(&id).unwrap_or_default()));
    }

    let mut sections: HashMap<Uuid, Vec<(String, String, Uuid)>> = HashMap::new();
    for (generation, kind, title, id) in query(
        connection,
        "SELECT id, generation_id, kind, title FROM sections WHERE deleted_at IS NULL
         ORDER BY generation_id, sort_key",
        |row| {
            Ok((
                row.get::<_, Uuid>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Uuid>(0)?,
            ))
        },
    )? {
        sections
            .entry(generation)
            .or_default()
            .push((kind, title, id));
    }

    let mut basis: HashMap<Uuid, Vec<FactVersion>> = HashMap::new();
    for (generation, version) in query(
        connection,
        "SELECT generation_id, fact_id, text, hidden FROM fact_versions
         ORDER BY generation_id, fact_id",
        |row| {
            Ok((
                row.get::<_, Uuid>(0)?,
                FactVersion {
                    fact: target(row.get(1)?),
                    text: row.get(2)?,
                    hidden: row.get(3)?,
                },
            ))
        },
    )? {
        basis.entry(generation).or_default().push(version);
    }

    let mut claims: HashMap<Uuid, Vec<UnsupportedClaim>> = HashMap::new();
    for (generation, claim) in query(
        connection,
        "SELECT id, generation_id, text, reason, figure, year, hidden_fact_id, added_as
         FROM unsupported_claims WHERE deleted_at IS NULL ORDER BY generation_id, sort_key",
        |row| {
            let reason: String = row.get(3)?;
            let reason = match reason.as_str() {
                "invented_figure" => UnsupportedReason::InventedFigure(row.get(4)?),
                "invented_year" => UnsupportedReason::InventedYear(row.get(5)?),
                "no_supporting_fact" => UnsupportedReason::NoSupportingFact,
                "uses_hidden_fact" => UnsupportedReason::UsesHiddenFact(target(row.get(6)?)),
                other => return Err(unknown("unsupported_claims.reason", other)),
            };
            Ok((
                row.get::<_, Uuid>(1)?,
                UnsupportedClaim {
                    id: ClaimId(row.get(0)?),
                    text: row.get(2)?,
                    reason,
                    added_as: row.get::<_, Option<Uuid>>(7)?.map(target),
                },
            ))
        },
    )? {
        claims.entry(generation).or_default().push(claim);
    }

    let rows = query(
        connection,
        "SELECT id, job_id, kind, generated_at, template_version_id FROM generations
         WHERE deleted_at IS NULL ORDER BY generated_at, id",
        |row| {
            Ok((
                row.get::<_, Uuid>(0)?,
                row.get::<_, Uuid>(1)?,
                row.get::<_, String>(2)?,
                time(row.get(3)?),
                row.get::<_, Option<Uuid>>(4)?,
            ))
        },
    )?;
    let mut generations = Vec::new();
    for (id, job, kind, generated_at, template_version) in rows {
        let mut generation_sections = Vec::new();
        for (section_kind, title, section) in sections.remove(&id).unwrap_or_default() {
            let section_blocks = blocks.remove(&section).unwrap_or_default();
            generation_sections.push((section_kind, title, section_blocks));
        }
        let unsupported_claims = claims.remove(&id).unwrap_or_default();
        let generation = match kind.as_str() {
            "resume" => {
                let mut sections = Vec::new();
                for (kind, title, section_blocks) in generation_sections {
                    sections.push(Section {
                        kind: stored(SectionKind::from_stored(&kind), "sections.kind", &kind)?,
                        title,
                        blocks: section_blocks
                            .into_iter()
                            .filter_map(|(kind, lines)| block(&kind, lines))
                            .collect(),
                    });
                }
                let template = typeset::TEMPLATES
                    .iter()
                    .find(|template| Some(template.version_id) == template_version)
                    .unwrap_or(typeset::template(typeset::DEFAULT_TEMPLATE));
                Generation::Resume(TailoredResume {
                    id: ResumeId(id),
                    generated_at,
                    template: template.key.to_string(),
                    sections,
                    unsupported_claims,
                    basis: basis.remove(&id).unwrap_or_default(),
                })
            }
            "cover_letter" => Generation::CoverLetter(CoverLetter {
                generated_at,
                paragraphs: generation_sections
                    .into_iter()
                    .flat_map(|(_, _, section_blocks)| section_blocks)
                    .map(|(_, lines)| lines.into_iter().map(|(_, line)| line).collect())
                    .collect(),
                unsupported_claims,
            }),
            other => return Err(unknown("generations.kind", other).into()),
        };
        generations.push((job, generation));
    }
    Ok(generations)
}

/// The block its lines make, by their roles; `None`, with a log, when a line
/// it needs is missing.
fn block(kind: &str, lines: Vec<RoledLine>) -> Option<Block> {
    let mut by_role: HashMap<String, Vec<Line>> = HashMap::new();
    for (role, line) in lines {
        by_role.entry(role).or_default().push(line);
    }
    let mut all = |role: &str| by_role.remove(role).unwrap_or_default();
    let block = match kind {
        "header" => first(all("name")).map(|name| Block::Header {
            name,
            contacts: all("contact"),
        }),
        "entry" => first(all("title")).map(|title| {
            Block::Entry(Box::new(EntryBlock {
                title,
                organization: first(all("organization")),
                dates: first(all("dates")),
                location: first(all("location")),
                stack: first(all("stack")),
                bullets: all("bullet"),
            }))
        }),
        "skills" => first(all("items")).map(|items| Block::Skills {
            label: first(all("label")),
            items,
        }),
        "text" => first(all("text")).map(Block::Text),
        "item" => first(all("item")).map(Block::Item),
        _ => None,
    };
    if block.is_none() {
        eprintln!("a {kind} block without the lines it needs");
    }
    block
}

fn first(lines: Vec<Line>) -> Option<Line> {
    lines.into_iter().next()
}

fn query<T>(
    connection: &Connection,
    sql: &str,
    read: impl FnMut(&Row) -> rusqlite::Result<T>,
) -> Result<Vec<T>, Error> {
    Ok(connection
        .prepare_cached(sql)?
        .query_map([], read)?
        .collect::<Result<Vec<_>, _>>()?)
}

/// The value, or an error naming the column that held text no variant has,
/// which its CHECK stops.
fn stored<T>(value: Option<T>, column: &str, text: &str) -> rusqlite::Result<T> {
    value.ok_or_else(|| unknown(column, text))
}

fn unknown(column: &str, text: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(0, format!("{column}: {text}"), rusqlite::types::Type::Text)
}

fn time(milliseconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(milliseconds).unwrap_or_default()
}

/// Spans are checked non-negative by the schema.
fn offset(value: i64) -> usize {
    usize::try_from(value).unwrap_or_default()
}
