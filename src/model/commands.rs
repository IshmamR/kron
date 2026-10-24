// What views run to change facts, notes and applications: each is a command
// for `Database::write`. The library is reloaded from the database after
// each one, until views query it themselves (storage.md, step 4).

use std::ops::Range;

use rusqlite::OptionalExtension;
use uuid::Uuid;

use super::{ClaimId, EventId, FactId, JobEvent, JobEventKind, JobId};
use crate::database::schema::{
    CustomSections, DocumentTexts, Documents, Entries, Events, FactSources, Facts, custom_sections,
    document_texts, documents, entries, events, fact_sources, facts, generations,
    unsupported_claims,
};
use crate::database::{Error, Write};
use crate::typeset;

/// The custom section notes' entries, and other entries without a section of
/// their own, go in.
pub const OTHER_SECTION: &str = "Other";

/// Saves what the user wrote as a note and makes a fact of each line, without
/// bullet markers. Lines written together are grouped as one entry. The user
/// wrote them, so they need no review.
pub fn add_note(write: &mut Write, text: &str) -> Result<Vec<FactId>, Error> {
    let lines = note_lines(text);
    if lines.is_empty() {
        return Ok(Vec::new());
    }
    let document = Uuid::now_v7();
    let now = write.now();
    write.insert(Documents, document, &[documents::ADDED_AT.value(&now)])?;
    write.insert(
        DocumentTexts,
        document,
        &[document_texts::TEXT.value(&text)],
    )?;
    let entry = if lines.len() > 1 {
        let entry = Uuid::now_v7();
        let section = other_section(write)?;
        write.insert(
            Entries,
            entry,
            &[
                entries::KIND.value(&"custom"),
                entries::CUSTOM_SECTION_ID.value(&Some(section)),
            ],
        )?;
        Some(entry)
    } else {
        None
    };
    let mut added = Vec::new();
    for (span, line) in lines {
        let fact = Uuid::now_v7();
        write.insert(
            Facts,
            fact,
            &[
                facts::KIND.value(&"bullet"),
                facts::TEXT.value(&line),
                facts::EXTRACTED.value(&line),
                facts::ENTRY_ID.value(&entry),
            ],
        )?;
        write.insert(
            FactSources,
            Uuid::now_v7(),
            &[
                fact_sources::FACT_ID.value(&fact),
                fact_sources::DOCUMENT_ID.value(&document),
                fact_sources::SPAN_START.value(&(span.start as i64)),
                fact_sources::SPAN_END.value(&(span.end as i64)),
                fact_sources::ORIGINAL_TEXT.value(&line),
            ],
        )?;
        added.push(FactId(fact));
    }
    Ok(added)
}

/// Makes the claim a fact, as a note, and points the claim at it. Nothing
/// once it has been added.
pub fn add_claim_as_fact(write: &mut Write, claim: ClaimId) -> Result<Vec<FactId>, Error> {
    let (text, added_as): (String, Option<Uuid>) = write
        .prepare("SELECT text, added_as FROM unsupported_claims WHERE id = ?1")?
        .query_row([claim.0], |row| Ok((row.get(0)?, row.get(1)?)))?;
    if added_as.is_some() {
        return Ok(Vec::new());
    }
    let added = add_note(write, &text)?;
    if let Some(first) = added.first() {
        write.set(unsupported_claims::ADDED_AS, claim.0, &Some(first.0))?;
    }
    Ok(added)
}

/// Hidden facts are never used in generated output.
pub fn set_hidden(write: &mut Write, fact: FactId, hidden: bool) -> Result<(), Error> {
    write.set(facts::HIDDEN, fact.0, &hidden)
}

/// Says the fact is right as it is: it no longer needs review.
pub fn confirm_fact(write: &mut Write, fact: FactId) -> Result<(), Error> {
    write.update(
        Facts,
        fact.0,
        &[
            facts::REVIEW.value(&None::<&str>),
            facts::REVIEW_NOTE.value(&None::<&str>),
        ],
    )
}

/// Rewording a fact is reviewing it. Empty text changes nothing.
pub fn edit_fact(write: &mut Write, fact: FactId, text: &str) -> Result<(), Error> {
    let text = text.trim();
    let current: String = write
        .prepare("SELECT text FROM facts WHERE id = ?1")?
        .query_row([fact.0], |row| row.get(0))?;
    if text.is_empty() || text == current {
        return Ok(());
    }
    write.update(
        Facts,
        fact.0,
        &[
            facts::TEXT.value(&text),
            facts::REVIEW.value(&None::<&str>),
            facts::REVIEW_NOTE.value(&None::<&str>),
        ],
    )
}

/// Puts the text back as extracted.
pub fn revert_fact(write: &mut Write, fact: FactId) -> Result<(), Error> {
    let (text, extracted): (String, String) = write
        .prepare("SELECT text, extracted FROM facts WHERE id = ?1")?
        .query_row([fact.0], |row| Ok((row.get(0)?, row.get(1)?)))?;
    if text == extracted {
        return Ok(());
    }
    write.set(facts::TEXT, fact.0, &extracted)
}

/// A merge, with what it changed on the kept fact, to undo it.
#[derive(Clone, Debug)]
pub struct Merge {
    pub keep: FactId,
    pub others: Vec<FactId>,
    review: Option<String>,
    review_note: Option<String>,
    entry: Option<Uuid>,
}

/// Folds `others` into `keep`, which keeps its wording and gains their
/// sources and citations: each of them points at it (`merged_into`), and
/// nothing is deleted. Merging is reviewing, so `keep` needs no review after.
pub fn merge_facts(write: &mut Write, keep: FactId, others: &[FactId]) -> Result<Merge, Error> {
    let others: Vec<FactId> = others.iter().copied().filter(|&id| id != keep).collect();
    let (review, review_note, entry): (Option<String>, Option<String>, Option<Uuid>) = write
        .prepare("SELECT review, review_note, entry_id FROM facts WHERE id = ?1")?
        .query_row([keep.0], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let mut adopted = entry;
    for other in &others {
        write.set(facts::MERGED_INTO, other.0, &Some(keep.0))?;
        if adopted.is_none() {
            adopted = write
                .prepare("SELECT entry_id FROM facts WHERE id = ?1")?
                .query_row([other.0], |row| row.get(0))?;
        }
    }
    write.update(
        Facts,
        keep.0,
        &[
            facts::REVIEW.value(&None::<&str>),
            facts::REVIEW_NOTE.value(&None::<&str>),
            facts::ENTRY_ID.value(&adopted),
        ],
    )?;
    Ok(Merge {
        keep,
        others,
        review,
        review_note,
        entry,
    })
}

pub fn undo_merge(write: &mut Write, merge: &Merge) -> Result<(), Error> {
    for other in &merge.others {
        write.clear(facts::MERGED_INTO, other.0)?;
    }
    write.update(
        Facts,
        merge.keep.0,
        &[
            facts::REVIEW.value(&merge.review),
            facts::REVIEW_NOTE.value(&merge.review_note),
            facts::ENTRY_ID.value(&merge.entry),
        ],
    )
}

/// Lays the job's current résumé out with the template called `key`. Its
/// content stays as it is.
pub fn set_template(write: &mut Write, job: JobId, key: &str) -> Result<(), Error> {
    let generation: Option<Uuid> = write
        .prepare(
            "SELECT generation_id FROM current_generations WHERE job_id = ?1 AND kind = 'resume'",
        )?
        .query_row([job.0], |row| row.get(0))
        .optional()?;
    let Some(generation) = generation else {
        return Ok(());
    };
    let template = typeset::template(key);
    write.set(
        generations::TEMPLATE_VERSION_ID,
        generation,
        &Some(template.version_id),
    )
}

/// Adds the event, or replaces the one with its id.
pub fn save_event(write: &mut Write, job: JobId, event: &JobEvent) -> Result<(), Error> {
    let (kind, resume, interview_kind, interview_with) = match &event.kind {
        JobEventKind::Applied { resume } => ("applied", resume.map(|resume| resume.0), None, None),
        JobEventKind::RecruiterReplied => ("recruiter_replied", None, None, None),
        JobEventKind::Interview { kind, with } => {
            ("interview", None, Some(kind.stored()), Some(with.as_str()))
        }
        JobEventKind::Offer => ("offer", None, None, None),
        JobEventKind::Rejected => ("rejected", None, None, None),
        JobEventKind::Withdrawn => ("withdrawn", None, None, None),
        JobEventKind::NoResponse => ("no_response", None, None, None),
        JobEventKind::Note => ("note", None, None, None),
    };
    let at = event.at.timestamp_millis();
    let values = [
        events::AT.value(&at),
        events::KIND.value(&kind),
        events::RESUME_ID.value(&resume),
        events::INTERVIEW_KIND.value(&interview_kind),
        events::INTERVIEW_WITH.value(&interview_with),
        events::NOTE.value(&event.note),
    ];
    let exists = write
        .prepare("SELECT 1 FROM events WHERE id = ?1")?
        .exists([event.id.0])?;
    if exists {
        return write.update(Events, event.id.0, &values);
    }
    let mut values = Vec::from(values);
    values.push(events::JOB_ID.value(&job.0));
    write.insert(Events, event.id.0, &values)
}

pub fn delete_event(write: &mut Write, event: EventId) -> Result<(), Error> {
    write.delete(Events, event.0)
}

pub fn restore_event(write: &mut Write, event: EventId) -> Result<(), Error> {
    write.clear(events::DELETED_AT, event.0)
}

/// The custom section for entries without one of their own, made the first
/// time it's needed.
pub fn other_section(write: &mut Write) -> Result<Uuid, Error> {
    let existing: Option<Uuid> = write
        .prepare(
            "SELECT id FROM custom_sections WHERE name = ?1 AND deleted_at IS NULL
             ORDER BY id LIMIT 1",
        )?
        .query_row([OTHER_SECTION], |row| row.get(0))
        .optional()?;
    if let Some(section) = existing {
        return Ok(section);
    }
    let section = Uuid::now_v7();
    let now = write.now();
    write.insert(
        CustomSections,
        section,
        &[
            custom_sections::NAME.value(&OTHER_SECTION),
            custom_sections::ADDED_AT.value(&now),
        ],
    )?;
    Ok(section)
}

/// Each non-empty line's text, without a bullet marker, and its byte range
/// in `text`.
fn note_lines(text: &str) -> Vec<(Range<usize>, &str)> {
    let mut lines = Vec::new();
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim().trim_start_matches(['-', '•', '*']).trim_start();
        if let Some(offset) = line.find(trimmed).filter(|_| !trimmed.is_empty()) {
            let start = line_start + offset;
            lines.push((start..start + trimmed.len(), trimmed));
        }
        line_start += line.len();
    }
    lines
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use rusqlite::{Connection, Params};

    use super::*;
    use crate::database::Writer;
    use crate::model::{
        Application, FactChange, InterviewKind, LineId, ResumeId, Section, citations,
        fact_changes, sample,
    };

    fn empty() -> Result<Writer, Error> {
        Writer::open_in_memory()
    }

    fn with_sample() -> Result<Writer, Error> {
        Ok(sample::written()?.0)
    }

    fn run<R>(
        writer: &mut Writer,
        command: impl FnOnce(&mut Write) -> Result<R, Error>,
    ) -> Result<R, Error> {
        Ok(writer.execute(command)?.0)
    }

    fn count(connection: &Connection, sql: &str, parameters: impl Params) -> Result<usize, Error> {
        let count: i64 = connection.query_row(sql, parameters, |row| row.get(0))?;
        usize::try_from(count).map_err(|error| Error::Misuse(error.to_string()))
    }

    fn fact_with_text(connection: &Connection, start: &str) -> Result<FactId, Error> {
        Ok(FactId(connection.query_row(
            "SELECT id FROM facts WHERE text LIKE ?1 || '%' AND merged_into IS NULL
             ORDER BY id LIMIT 1",
            [start],
            |row| row.get(0),
        )?))
    }

    fn job_at(connection: &Connection, company: &str) -> Result<JobId, Error> {
        Ok(JobId(connection.query_row(
            "SELECT id FROM jobs WHERE company = ?1",
            [company],
            |row| row.get(0),
        )?))
    }

    fn current_resume(connection: &Connection, job: JobId) -> Result<Option<ResumeId>, Error> {
        Ok(connection
            .query_row(
                "SELECT generation_id FROM current_generations
                 WHERE job_id = ?1 AND kind = 'resume'",
                [job.0],
                |row| row.get(0),
            )
            .optional()?
            .map(ResumeId))
    }

    /// The lines of each job's current résumé that cite the fact, following
    /// merges.
    fn uses(connection: &Connection, fact: FactId) -> Result<Vec<LineId>, Error> {
        let resumes: Vec<ResumeId> = connection
            .prepare("SELECT generation_id FROM current_generations WHERE kind = 'resume'")?
            .query_map([], |row| Ok(ResumeId(row.get(0)?)))?
            .collect::<Result<_, _>>()?;
        let mut lines = Vec::new();
        for resume in resumes {
            for (line, cited) in citations(connection, resume)? {
                if cited == fact {
                    lines.push(line);
                }
            }
        }
        lines.sort();
        Ok(lines)
    }

    fn sources(connection: &Connection, fact: FactId) -> Result<usize, Error> {
        count(
            connection,
            "SELECT count(*) FROM fact_sources JOIN facts ON facts.id = fact_sources.fact_id
             WHERE (facts.id = ?1 OR facts.merged_into = ?1) AND fact_sources.deleted_at IS NULL",
            [fact.0],
        )
    }

    fn changed(connection: &Connection, company: &str) -> Result<Vec<(FactId, FactChange)>, Error> {
        match current_resume(connection, job_at(connection, company)?)? {
            Some(resume) => fact_changes(connection, resume),
            None => Ok(Vec::new()),
        }
    }

    #[test]
    fn the_sample_reads_back_as_it_was_written() -> Result<(), Error> {
        let (writer, written) = sample::written()?;
        let connection = writer.kron();
        let rows = |table: &str| count(connection, &format!("SELECT count(*) FROM {table}"), []);
        assert_eq!(rows("documents")?, written.documents.len());
        assert_eq!(rows("entries")?, written.entries.len());
        assert_eq!(rows("facts")?, written.facts.len());
        assert_eq!(
            rows("fact_sources")?,
            written.facts.iter().map(|fact| fact.sources.len()).sum::<usize>()
        );
        assert_eq!(rows("jobs")?, written.jobs.len());
        for job in &written.jobs {
            let of_job = |sql: &str| count(connection, sql, [job.id.0]);
            assert_eq!(
                of_job(
                    "SELECT count(*) FROM events
                     WHERE job_id = ?1 AND kind != 'wishlisted' AND deleted_at IS NULL"
                )?,
                job.events.len(),
                "{}",
                job.company
            );
            assert_eq!(
                of_job("SELECT count(*) FROM generations WHERE job_id = ?1 AND kind = 'resume'")?,
                job.resumes.len(),
                "{}",
                job.company
            );
            assert_eq!(
                of_job("SELECT count(*) FROM fact_overrides WHERE job_id = ?1")?,
                job.overrides.len(),
                "{}",
                job.company
            );
            assert_eq!(
                of_job(
                    "SELECT count(*) FROM generations
                     JOIN sections ON sections.generation_id = generations.id
                     JOIN blocks ON blocks.section_id = sections.id
                     WHERE generations.job_id = ?1 AND generations.kind = 'cover_letter'"
                )?,
                job.cover_letter
                    .as_ref()
                    .map_or(0, |letter| letter.paragraphs.len()),
                "{}",
                job.company
            );
            for resume in &job.resumes {
                let of_resume = |sql: &str| count(connection, sql, [resume.id.0]);
                let sections = Section::read_all(connection, resume.id)?;
                let lines = sections
                    .iter()
                    .flat_map(|section| &section.blocks)
                    .map(|block| block.lines().len())
                    .sum::<usize>();
                assert_eq!(lines, resume.lines().count());
                let version: Option<Uuid> = connection.query_row(
                    "SELECT template_version_id FROM generations WHERE id = ?1",
                    [resume.id.0],
                    |row| row.get(0),
                )?;
                assert_eq!(typeset::template_of_version(version).key, resume.template);
                assert_eq!(
                    of_resume("SELECT count(*) FROM fact_versions WHERE generation_id = ?1")?,
                    resume.basis.len()
                );
                assert_eq!(
                    of_resume("SELECT count(*) FROM unsupported_claims WHERE generation_id = ?1")?,
                    resume.unsupported_claims.len()
                );
            }
        }
        Ok(())
    }

    #[test]
    fn a_note_becomes_one_fact_per_line_grouped_together() -> Result<(), Error> {
        let mut writer = empty()?;
        let added = run(&mut writer, |write| {
            add_note(
                write,
                "- Organized Code Fest 2019.\n\n  • Ran a team of 15.\n",
            )
        })?;
        assert_eq!(added.len(), 2);
        let mut facts = Vec::new();
        for fact in &added {
            let read: (String, Option<String>, Option<Uuid>, String, i64, i64) =
                writer.kron().query_row(
                    "SELECT facts.text, facts.review, facts.entry_id, document_texts.text,
                            fact_sources.span_start, fact_sources.span_end
                     FROM facts
                     JOIN fact_sources ON fact_sources.fact_id = facts.id
                     JOIN document_texts ON document_texts.document_id = fact_sources.document_id
                     WHERE facts.id = ?1",
                    [fact.0],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )?;
            facts.push(read);
        }
        assert_eq!(facts[0].0, "Organized Code Fest 2019.");
        assert_eq!(facts[1].0, "Ran a team of 15.");
        assert!(facts[0].2.is_some() && facts[0].2 == facts[1].2);
        for (text, review, _, document, start, end) in &facts {
            assert_eq!(*review, None, "the user wrote it");
            let span = usize::try_from(*start).unwrap_or_default()
                ..usize::try_from(*end).unwrap_or_default();
            assert_eq!(document.get(span), Some(text.as_str()));
        }
        Ok(())
    }

    #[test]
    fn a_one_line_note_needs_no_entry() -> Result<(), Error> {
        let mut writer = empty()?;
        let added = run(&mut writer, |write| {
            add_note(write, "Gave a guest lecture.")
        })?;
        let entry: Option<Uuid> = writer.kron().query_row(
            "SELECT entry_id FROM facts WHERE id = ?1",
            [added[0].0],
            |row| row.get(0),
        )?;
        assert_eq!(entry, None);
        assert!(run(&mut writer, |write| add_note(write, "  \n"))?.is_empty());
        Ok(())
    }

    #[test]
    fn merging_moves_sources_and_citations_and_undoing_puts_them_back() -> Result<(), Error> {
        let mut writer = with_sample()?;
        let keep = fact_with_text(writer.kron(), "Migrated 12 backend")?;
        let merged = fact_with_text(writer.kron(), "Led the migration")?;
        let kept_uses = uses(writer.kron(), keep)?;
        let merged_uses = uses(writer.kron(), merged)?;
        assert!(!merged_uses.is_empty());
        let mut both: Vec<LineId> = kept_uses.iter().chain(&merged_uses).copied().collect();
        both.sort();
        both.dedup();

        let merge = run(&mut writer, |write| {
            merge_facts(write, keep, &[keep, merged])
        })?;
        let (merged_into, review): (Option<Uuid>, Option<String>) = writer.kron().query_row(
            "SELECT merged.merged_into, kept.review FROM facts AS merged, facts AS kept
             WHERE merged.id = ?1 AND kept.id = ?2",
            [merged.0, keep.0],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(merged_into, Some(keep.0));
        assert_eq!(review, None);
        assert_eq!(sources(writer.kron(), keep)?, 2);
        assert_eq!(uses(writer.kron(), keep)?, both);
        assert!(uses(writer.kron(), merged)?.is_empty());

        run(&mut writer, |write| undo_merge(write, &merge))?;
        assert_eq!(sources(writer.kron(), keep)?, 1);
        assert_eq!(uses(writer.kron(), keep)?, kept_uses);
        assert_eq!(uses(writer.kron(), merged)?, merged_uses);
        Ok(())
    }

    #[test]
    fn editing_or_hiding_a_fact_outdates_the_resumes_that_use_it() -> Result<(), Error> {
        let mut writer = with_sample()?;
        assert_eq!(changed(writer.kron(), "Ledgerly")?, Vec::new());

        let latency = fact_with_text(writer.kron(), "Cut p99 latency")?;
        run(&mut writer, |write| confirm_fact(write, latency))?;
        assert_eq!(changed(writer.kron(), "Ledgerly")?, Vec::new(), "confirming changes nothing");

        run(&mut writer, |write| {
            edit_fact(write, latency, "Cut p99 latency of the routing API by 80%.")
        })?;
        assert_eq!(
            changed(writer.kron(), "Ledgerly")?,
            vec![(latency, FactChange::Reworded)]
        );
        run(&mut writer, |write| revert_fact(write, latency))?;
        assert_eq!(
            count(
                writer.kron(),
                "SELECT count(*) FROM facts WHERE id = ?1 AND text = extracted",
                [latency.0]
            )?,
            1
        );
        assert_eq!(
            changed(writer.kron(), "Ledgerly")?,
            Vec::new(),
            "reverting undoes the change"
        );

        run(&mut writer, |write| set_hidden(write, latency, true))?;
        assert_eq!(
            changed(writer.kron(), "Ledgerly")?,
            vec![(latency, FactChange::Hidden)]
        );
        run(&mut writer, |write| set_hidden(write, latency, false))?;
        assert_eq!(
            changed(writer.kron(), "Ledgerly")?,
            Vec::new(),
            "showing it again undoes hiding"
        );
        Ok(())
    }

    #[test]
    fn showing_a_hidden_fact_a_resume_needed_outdates_it() -> Result<(), Error> {
        let mut writer = with_sample()?;
        let property_tests = fact_with_text(writer.kron(), "Introduced property-based tests")?;
        run(&mut writer, |write| {
            set_hidden(write, property_tests, false)
        })?;
        assert_eq!(
            changed(writer.kron(), "Ledgerly")?,
            vec![(property_tests, FactChange::Shown)]
        );
        Ok(())
    }

    #[test]
    fn adding_a_claim_as_a_fact_keeps_the_claim_to_point_at_it() -> Result<(), Error> {
        let mut writer = with_sample()?;
        let claim = ClaimId(writer.kron().query_row(
            "SELECT unsupported_claims.id FROM unsupported_claims
             JOIN current_generations AS current
               ON current.generation_id = unsupported_claims.generation_id
             WHERE unsupported_claims.added_as IS NULL
             ORDER BY unsupported_claims.id LIMIT 1",
            [],
            |row| row.get(0),
        )?);
        let added = run(&mut writer, |write| add_claim_as_fact(write, claim))?;
        assert_eq!(added.len(), 1);
        let added_as: Option<Uuid> = writer.kron().query_row(
            "SELECT added_as FROM unsupported_claims WHERE id = ?1",
            [claim.0],
            |row| row.get(0),
        )?;
        assert_eq!(added_as.map(FactId), added.first().copied());
        assert!(
            run(&mut writer, |write| add_claim_as_fact(write, claim))?.is_empty(),
            "only once"
        );
        Ok(())
    }

    #[test]
    fn an_event_can_change_kind_be_deleted_and_come_back() -> Result<(), Error> {
        let mut writer = with_sample()?;
        let job = job_at(writer.kron(), "Ledgerly")?;
        let mut event = JobEvent::new(
            Utc::now(),
            JobEventKind::Interview {
                kind: InterviewKind::Phone,
                with: "A recruiter".into(),
            },
        );
        run(&mut writer, |write| save_event(write, job, &event))?;
        let events = |writer: &Writer| -> Result<Vec<JobEvent>, Error> {
            Ok(Application::read(writer.kron(), job)?
                .map(|application| application.events)
                .unwrap_or_default())
        };
        let before = events(&writer)?.len();

        // The interview's columns go in the same statement as the kind, or
        // the CHECKs tying them together refuse it.
        event.kind = JobEventKind::Note;
        event.note = Some("Moved to next week.".into());
        run(&mut writer, |write| save_event(write, job, &event))?;
        let saved = events(&writer)?
            .into_iter()
            .find(|each| each.id == event.id)
            .map(|each| each.kind);
        assert_eq!(saved, Some(JobEventKind::Note));
        assert_eq!(events(&writer)?.len(), before);

        run(&mut writer, |write| delete_event(write, event.id))?;
        assert_eq!(events(&writer)?.len(), before - 1);
        run(&mut writer, |write| restore_event(write, event.id))?;
        assert_eq!(events(&writer)?.len(), before);
        Ok(())
    }

    #[test]
    fn a_template_switch_lays_out_the_current_resume() -> Result<(), Error> {
        let mut writer = with_sample()?;
        let job = job_at(writer.kron(), "Ledgerly")?;
        run(&mut writer, |write| set_template(write, job, "sidebar"))?;
        let version: Option<Uuid> = writer.kron().query_row(
            "SELECT template_version_id FROM current_generations
             WHERE job_id = ?1 AND kind = 'resume'",
            [job.0],
            |row| row.get(0),
        )?;
        assert_eq!(typeset::template_of_version(version).key, "sidebar");
        Ok(())
    }
}
