// Reading one generation's sections, blocks and lines: what a template lays
// out, and what the job view shows.

use std::collections::HashMap;

use rusqlite::Connection;
use uuid::Uuid;

use super::{Block, EntryBlock, FactChange, FactId, Line, LineId, ResumeId, Section, SectionKind};
use crate::database::Error;

/// A line and its role in its block, like "title" or "bullet".
pub type RoledLine = (String, Line);

impl Section {
    /// The generation's sections in order, each with its blocks and lines.
    /// A block missing a line it needs is left out.
    pub fn read_all(connection: &Connection, generation: ResumeId) -> Result<Vec<Section>, Error> {
        type Row = (
            Uuid,
            SectionKind,
            String,
            Option<Uuid>,
            Option<String>,
            Option<Uuid>,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<Row> = connection
            .prepare_cached(
                "SELECT sections.id, sections.kind, sections.title, blocks.id, blocks.kind,
                        lines.id, lines.role, lines.generated, lines.text
                 FROM sections
                 LEFT JOIN blocks ON blocks.section_id = sections.id AND blocks.deleted_at IS NULL
                 LEFT JOIN lines ON lines.block_id = blocks.id AND lines.deleted_at IS NULL
                 WHERE sections.generation_id = ?1 AND sections.deleted_at IS NULL
                 ORDER BY sections.sort_key, sections.id, blocks.sort_key, blocks.id,
                          lines.sort_key, lines.id",
            )?
            .query_map([generation.0], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        let mut cites: HashMap<LineId, Vec<FactId>> = HashMap::new();
        for (line, fact) in citations(connection, generation)? {
            cites.entry(line).or_default().push(fact);
        }

        let mut sections: Vec<(Uuid, Section)> = Vec::new();
        // The block being gathered: its section, id, kind and lines so far.
        let mut block: Option<(Uuid, Uuid, String, Vec<RoledLine>)> = None;
        let finish = |block: Option<(Uuid, Uuid, String, Vec<RoledLine>)>,
                      sections: &mut Vec<(Uuid, Section)>| {
            let Some((section, _, kind, lines)) = block else {
                return;
            };
            if let Some(built) = Block::from_lines(&kind, lines)
                && let Some((_, section)) = sections.iter_mut().find(|(id, _)| *id == section)
            {
                section.blocks.push(built);
            }
        };
        for (section, kind, title, block_id, block_kind, line, role, generated, text) in rows {
            if sections.last().is_none_or(|(id, _)| *id != section) {
                finish(block.take(), &mut sections);
                sections.push((
                    section,
                    Section {
                        kind,
                        title,
                        blocks: Vec::new(),
                    },
                ));
            }
            let (Some(block_id), Some(block_kind)) = (block_id, block_kind) else {
                continue;
            };
            if block.as_ref().is_none_or(|(_, id, _, _)| *id != block_id) {
                finish(block.take(), &mut sections);
                block = Some((section, block_id, block_kind, Vec::new()));
            }
            if let (Some(line), Some(role), Some(generated), Some(text), Some(block)) =
                (line, role, generated, text, block.as_mut())
            {
                let id = LineId(line);
                block.3.push((
                    role,
                    Line {
                        id,
                        generated,
                        text,
                        cites: cites.remove(&id).unwrap_or_default(),
                    },
                ));
            }
        }
        finish(block.take(), &mut sections);
        Ok(sections.into_iter().map(|(_, section)| section).collect())
    }
}

impl Block {
    /// The block its lines make, by their roles; `None`, with a log, when a
    /// line it needs is missing.
    pub fn from_lines(kind: &str, lines: Vec<RoledLine>) -> Option<Block> {
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
}

/// Each line of the generation and the facts it cites, in order. A fact
/// merged into another since counts as the one at the end of its merges, and
/// two that ended in the same one count once.
pub fn citations(
    connection: &Connection,
    generation: ResumeId,
) -> Result<Vec<(LineId, FactId)>, Error> {
    let rows: Vec<(LineId, FactId)> = connection
        .prepare_cached(
            "WITH RECURSIVE
                 cited(line_id, fact_id, sort_key) AS (
                     SELECT line_citations.line_id, line_citations.fact_id,
                            line_citations.sort_key
                     FROM sections
                     JOIN blocks ON blocks.section_id = sections.id
                     JOIN lines ON lines.block_id = blocks.id
                     JOIN line_citations ON line_citations.line_id = lines.id
                     WHERE sections.generation_id = ?1
                 ),
                 chains(fact_id, root_id) AS (
                     SELECT fact_id, fact_id FROM cited
                     UNION
                     SELECT chains.fact_id, facts.merged_into
                     FROM chains JOIN facts ON facts.id = chains.root_id
                     WHERE facts.merged_into IS NOT NULL
                 )
             SELECT cited.line_id, chains.root_id
             FROM cited
             JOIN chains ON chains.fact_id = cited.fact_id
             JOIN facts AS roots ON roots.id = chains.root_id AND roots.merged_into IS NULL
             ORDER BY cited.line_id, cited.sort_key",
        )?
        .query_map([generation.0], |row| {
            Ok((LineId(row.get(0)?), FactId(row.get(1)?)))
        })?
        .collect::<Result<_, _>>()?;
    let mut citations: Vec<(LineId, FactId)> = Vec::with_capacity(rows.len());
    for citation in rows {
        if !citations.contains(&citation) {
            citations.push(citation);
        }
    }
    Ok(citations)
}

/// How each fact the résumé was generated from differs from the version it
/// was generated from, if it does. A merged fact counts as the one it was
/// merged into.
pub fn fact_changes(
    connection: &Connection,
    generation: ResumeId,
) -> Result<Vec<(FactId, FactChange)>, Error> {
    let versions: Vec<(FactId, bool, bool, bool)> = connection
        .prepare_cached(
            "WITH RECURSIVE chains(fact_id, root_id) AS (
                 SELECT fact_id, fact_id FROM fact_versions WHERE generation_id = ?1
                 UNION
                 SELECT chains.fact_id, facts.merged_into
                 FROM chains JOIN facts ON facts.id = chains.root_id
                 WHERE facts.merged_into IS NOT NULL
             )
             SELECT roots.id, fact_versions.hidden, roots.hidden,
                    fact_versions.text != roots.text
             FROM fact_versions
             JOIN chains ON chains.fact_id = fact_versions.fact_id
             JOIN facts AS roots ON roots.id = chains.root_id
                                AND roots.merged_into IS NULL AND roots.deleted_at IS NULL
             WHERE fact_versions.generation_id = ?1
             ORDER BY fact_versions.fact_id",
        )?
        .query_map([generation.0], |row| {
            Ok((FactId(row.get(0)?), row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<_, _>>()?;
    let mut changes: Vec<(FactId, FactChange)> = Vec::new();
    for (fact, was_hidden, hidden, reworded) in versions {
        let change = match (was_hidden, hidden) {
            (false, true) => FactChange::Hidden,
            (true, false) => FactChange::Shown,
            _ if reworded => FactChange::Reworded,
            _ => continue,
        };
        if !changes.iter().any(|(changed, _)| *changed == fact) {
            changes.push((fact, change));
        }
    }
    Ok(changes)
}

fn first(lines: Vec<Line>) -> Option<Line> {
    lines.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FactId, commands, sample};

    fn resumes(connection: &Connection) -> Result<Vec<ResumeId>, Error> {
        Ok(connection
            .prepare("SELECT id FROM generations WHERE kind = 'resume' ORDER BY id")?
            .query_map([], |row| Ok(ResumeId(row.get(0)?)))?
            .collect::<Result<_, _>>()?)
    }

    fn lines(sections: &[Section]) -> Vec<&Line> {
        sections
            .iter()
            .flat_map(|section| &section.blocks)
            .flat_map(Block::lines)
            .collect()
    }

    /// The facts the line's row cites, in their order.
    fn cited(connection: &Connection, line: LineId) -> Result<Vec<FactId>, Error> {
        Ok(connection
            .prepare("SELECT fact_id FROM line_citations WHERE line_id = ?1 ORDER BY sort_key")?
            .query_map([line.0], |row| Ok(FactId(row.get(0)?)))?
            .collect::<Result<_, _>>()?)
    }

    #[test]
    fn every_sample_resume_reads_back_every_line_with_its_citations() -> Result<(), Error> {
        let writer = sample::written()?;
        let connection = writer.kron();
        let resumes = resumes(connection)?;
        assert_eq!(resumes.len(), 9);
        for resume in resumes {
            let sections = Section::read_all(connection, resume)?;
            let rows: i64 = connection.query_row(
                "SELECT count(*) FROM lines
                 JOIN blocks ON blocks.id = lines.block_id
                 JOIN sections ON sections.id = blocks.section_id
                 WHERE sections.generation_id = ?1",
                [resume.0],
                |row| row.get(0),
            )?;
            let lines = lines(&sections);
            assert_eq!(lines.len() as i64, rows, "no line is lost to its block");
            for line in lines {
                assert!(!line.cites.is_empty(), "{:?} cites nothing", line.text);
                assert_eq!(line.cites, cited(connection, line.id)?, "{:?}", line.text);
            }
        }
        Ok(())
    }

    #[test]
    fn a_sample_resume_reads_back_its_sections_in_order() -> Result<(), Error> {
        let writer = sample::written()?;
        let connection = writer.kron();
        let job = sample::job(connection, "Atlas Freight")?;
        let resume = ResumeId(connection.query_row(
            "SELECT id FROM generations WHERE job_id = ?1 AND kind = 'resume'",
            [job.0],
            |row| row.get(0),
        )?);
        let sections: Vec<(SectionKind, String, usize)> = Section::read_all(connection, resume)?
            .into_iter()
            .map(|section| (section.kind, section.title, section.blocks.len()))
            .collect();
        assert_eq!(
            sections,
            [
                (SectionKind::Header, String::new(), 1),
                (SectionKind::Summary, "Summary".into(), 1),
                (SectionKind::Experience, "Experience".into(), 4),
                (SectionKind::Projects, "Projects".into(), 2),
                (SectionKind::Other, "Talks".into(), 2),
                (SectionKind::Education, "Education".into(), 1),
                (SectionKind::Skills, "Skills".into(), 3),
            ]
        );
        Ok(())
    }

    #[test]
    fn citations_follow_merges_to_the_kept_fact() -> Result<(), Error> {
        let mut writer = sample::written()?;
        let migration = sample::fact(writer.kron(), "Led the migration")?;
        let linkedin = sample::fact(writer.kron(), "Migrated 12 backend")?;
        let latency = sample::fact(writer.kron(), "Cut p99 latency")?;
        let citing_merged: Vec<LineId> = writer
            .kron()
            .prepare("SELECT line_id FROM line_citations WHERE fact_id IN (?1, ?2)")?
            .query_map([migration.0, linkedin.0], |row| Ok(LineId(row.get(0)?)))?
            .collect::<Result<_, _>>()?;
        assert!(!citing_merged.is_empty());

        writer.execute(|write| commands::merge_facts(write, linkedin, &[migration]))?;
        writer.execute(|write| commands::merge_facts(write, latency, &[linkedin]))?;
        for resume in resumes(writer.kron())? {
            for line in lines(&Section::read_all(writer.kron(), resume)?) {
                assert!(
                    !line.cites.contains(&migration) && !line.cites.contains(&linkedin),
                    "{:?} still cites a merged fact",
                    line.text
                );
                let mut unique = line.cites.clone();
                unique.sort();
                unique.dedup();
                assert_eq!(
                    unique.len(),
                    line.cites.len(),
                    "{:?} cites a fact twice",
                    line.text
                );
                if citing_merged.contains(&line.id) {
                    assert!(line.cites.contains(&latency), "{:?}", line.text);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn a_missing_resume_has_no_sections() -> Result<(), Error> {
        let writer = sample::written()?;
        let sections = Section::read_all(writer.kron(), ResumeId::new())?;
        assert!(sections.is_empty());
        Ok(())
    }
}
