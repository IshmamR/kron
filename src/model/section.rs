// Reading one generation's sections, blocks and lines: what a template lays
// out, and what the job view shows.

use std::collections::HashMap;

use rusqlite::Connection;
use uuid::Uuid;

use super::{Block, EntryBlock, FactId, Line, LineId, ResumeId, Section, SectionKind};
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
        .query_map([generation.0], |row| Ok((LineId(row.get(0)?), FactId(row.get(1)?))))?
        .collect::<Result<_, _>>()?;
    let mut citations: Vec<(LineId, FactId)> = Vec::with_capacity(rows.len());
    for citation in rows {
        if !citations.contains(&citation) {
            citations.push(citation);
        }
    }
    Ok(citations)
}

fn first(lines: Vec<Line>) -> Option<Line> {
    lines.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{Write, Writer};
    use crate::model::{FactId, Library, commands, sample};
    use crate::typeset::{self, resume_markup};

    fn sample_writer() -> Result<Writer, Error> {
        let mut writer = Writer::open_in_memory()?;
        writer.execute(|write: &mut Write| {
            typeset::install_templates(write)?;
            sample::write_library(write)
        })?;
        Ok(writer)
    }

    /// Every résumé reads back with the markup and citations `Library::load`
    /// gives it, so it typesets the same.
    fn assert_resumes_match_the_library(connection: &Connection) -> Result<(), Error> {
        let library = Library::load(connection)?;
        let mut checked = 0;
        for resume in library.jobs.iter().flat_map(|job| &job.resumes) {
            let sections = Section::read_all(connection, resume.id)?;
            let template = typeset::template(&resume.template);
            assert_eq!(
                resume_markup(template, &sections),
                resume_markup(template, &resume.sections)
            );
            let cites = |sections: &[Section]| -> Vec<(LineId, Vec<FactId>)> {
                sections
                    .iter()
                    .flat_map(|section| &section.blocks)
                    .flat_map(Block::lines)
                    .map(|line| (line.id, line.cites.clone()))
                    .collect()
            };
            assert_eq!(cites(&sections), cites(&resume.sections));
            checked += 1;
        }
        assert!(checked > 0);
        Ok(())
    }

    #[test]
    fn every_sample_resume_reads_back_as_the_library_reads_it() -> Result<(), Error> {
        assert_resumes_match_the_library(sample_writer()?.kron())
    }

    #[test]
    fn citations_follow_merges_as_the_library_does() -> Result<(), Error> {
        let mut writer = sample_writer()?;
        let fact = |writer: &Writer, start: &str| -> Result<FactId, Error> {
            Ok(FactId(writer.kron().query_row(
                "SELECT id FROM facts WHERE text LIKE ?1 || '%'",
                [start],
                |row| row.get(0),
            )?))
        };
        let migration = fact(&writer, "Led the migration")?;
        let linkedin = fact(&writer, "Migrated 12 backend")?;
        let latency = fact(&writer, "Cut p99 latency")?;
        writer.execute(|write| commands::merge_facts(write, linkedin, &[migration]))?;
        writer.execute(|write| commands::merge_facts(write, latency, &[linkedin]))?;
        assert_resumes_match_the_library(writer.kron())
    }

    #[test]
    fn a_missing_resume_has_no_sections() -> Result<(), Error> {
        let writer = sample_writer()?;
        let sections = Section::read_all(writer.kron(), ResumeId::new())?;
        assert!(sections.is_empty());
        Ok(())
    }
}
