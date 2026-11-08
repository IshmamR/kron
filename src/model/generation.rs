// Writes a résumé for a job from the user's facts. The model is sent the
// facts, numbered and grouped by entry, with the user's name and contact
// details hidden (`redaction`), and the job's description. It picks the
// entries, skills and items that fit the job, and writes the summary and each
// bullet, citing the facts it rests on: it may reword a fact for the job and
// combine a few. Kron writes everything else from the facts as they are: the
// header, each entry's title, organization, dates and location, the skills
// and the items. Every line the model wrote goes through the verifier, and
// one that fails is left out and listed as an unsupported claim.

use std::ops::Range;

use chrono::{Datelike, Local};
use rig_core::completion::CompletionRequest;
use rig_core::completion::options::{GenerationOptions, OnUnsupported, Reasoning};
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use uuid::Uuid;

use super::redaction::Redaction;
use super::verifier::{self, GivenFact};
use super::{
    Block, EntryBlock, EntryKind, FactId, FactKind, JobId, Line, LineId, ResumeId, Section,
    SectionKind, UnsupportedReason,
};
use crate::database::schema::{
    Blocks, FactVersions, Generations, LineCitations, Lines, Sections, UnsupportedClaims, blocks,
    fact_versions, generations, line_citations, lines, sections, unsupported_claims,
};
use crate::database::{Error, Write};
use crate::typeset::{self, Paper};

/// What the model is told. The answer's shape is spelled out, since that's
/// how every provider is asked for it: only DeepSeek is put in JSON mode
/// (`llm::call`), and it takes "a JSON object" rather than a schema.
const PREAMBLE: &str = r#"You write a résumé for one job from facts about a person. Each fact has an id, like F12. Facts are grouped in entries, like E3: a job, degree, project, certification, group of skills, or other items like talks.

Answer with one JSON object. Ids are strings in it, like "F12" and "E3":
{
  "summary": {"text": two or three sentences of what this person has done that matters most for the job, "facts": [the ids of the facts it rests on]}, or null,
  "entries": [
    {
      "entry": the id of a job, degree, project, certification or other entry to show,
      "bullets": [{"text": a bullet for this job, "facts": [the ids of the facts it rests on]}],
      "skills": [the ids of the entry's own skill facts to show with it, most relevant first]
    }
  ],
  "skills": [{"entry": the id of a group of skills, "skills": [the ids of its skills to show, most relevant first]}],
  "items": [the ids of facts in no entry, like talks or awards, worth showing for this job]
}

Rules:
- A bullet says only what its facts say. Reword it for this job: lead with what matters most to it, and use the job's words where they mean the same as the fact's.
- Never add a number, year, technology, tool, title, scope or result that the facts don't state, and never make one bigger. Keep every number as the facts give it.
- A bullet may combine two or three facts from its entry when they're about the same work. Cite every fact a bullet uses, and only facts from its own entry; the summary may cite any.
- Order each entry's bullets by how much they matter to this job. Leave out facts that don't help with it.
- Never write a bullet that only repeats an entry's title, organization, dates or location: they're shown already.
- Show every group of skills that helps with this job, its most relevant skills first. Leave out only skills that don't help.
- Show every job, unless there are many and the old ones don't help. Show degrees and certifications. Show other entries, like volunteering, when they help.
- Fit one page: at most four bullets for the latest job, three for the one before, one or two for older ones, and one or two for each project. Two pages only for more than ten years of work.
- Write as résumés do, without "I", "he", "she" or "they", and never the person's name or contact details: they're added.
- Say what the person did, never how well they fit the job or what the employer needs.
- Placeholders in square brackets, like [LINK_1], stand for hidden details. Copy them as they are.
- The person may give instructions for this résumé, like what to lead with. Follow them where these rules allow: they can change what's shown and in what order, never let a line say more than its facts."#;
/// Room for a two-page résumé as JSON. Ours, from extraction's: a two-page
/// CV's facts are about 4,000 tokens of answer.
const MAX_TOKENS: u64 = 16_000;
/// The section certifications are shown in: résumés have one, but it has no
/// kind of its own.
const CERTIFICATIONS: &str = "Certifications";
/// Marks the person's own instructions off from the facts in what's sent.
const INSTRUCTIONS_HEADING: &str = "\nTheir instructions for this résumé:\n";
/// What a list of skills is joined with on the page.
const SKILL_SEPARATOR: &str = ", ";

/// The facts a résumé can be made from, numbered as the model is given
/// them: hidden, merged and deleted facts are left out.
pub struct Material {
    facts: Vec<Given>,
    entries: Vec<GivenEntry>,
    /// For the header, which Kron writes: never sent.
    name: Option<Given>,
    contacts: Vec<Given>,
    /// The facts as the model is given them, before redaction.
    pub listing: String,
    /// This month, as `month` counts them, which "present" in a job's
    /// dates means.
    now: i32,
}

struct Given {
    id: FactId,
    kind: FactKind,
    text: String,
    entry: Option<usize>,
}

struct GivenEntry {
    kind: EntryKind,
    /// A custom section's name, for an entry of kind `Other`.
    section: Option<String>,
    facts: Vec<usize>,
}

/// The job a résumé is for, as the model is given it.
pub struct Job<'a> {
    pub title: &'a str,
    pub company: &'a str,
    pub description: &'a str,
}

/// The model's answer, as the preamble describes it.
#[derive(Debug, Default, Deserialize)]
pub struct Answer {
    #[serde(default)]
    summary: Option<Written>,
    #[serde(default)]
    entries: Vec<ChosenEntry>,
    #[serde(default)]
    skills: Vec<ChosenSkills>,
    #[serde(default)]
    items: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Written {
    #[serde(default)]
    text: String,
    #[serde(default)]
    facts: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChosenEntry {
    #[serde(default)]
    entry: String,
    #[serde(default)]
    bullets: Vec<Written>,
    #[serde(default)]
    skills: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChosenSkills {
    #[serde(default)]
    entry: String,
    #[serde(default)]
    skills: Vec<String>,
}

/// A résumé ready to save: its sections, and the claims left out of it.
#[derive(Debug)]
pub struct Draft {
    pub sections: Vec<Section>,
    pub claims: Vec<(String, UnsupportedReason)>,
    /// The facts it was made from, as they are now.
    facts: Vec<(FactId, String)>,
}

impl Material {
    pub fn read(connection: &Connection) -> Result<Self, Error> {
        let mut statement = connection.prepare_cached(
            "SELECT facts.id, facts.kind, facts.text, facts.entry_id, entries.kind,
                    custom_sections.name
             FROM facts
             LEFT JOIN entries ON entries.id = facts.entry_id
             LEFT JOIN custom_sections ON custom_sections.id = entries.custom_section_id
             WHERE facts.deleted_at IS NULL AND facts.merged_into IS NULL AND facts.hidden = 0
                 AND (facts.entry_id IS NULL OR entries.deleted_at IS NULL)
             ORDER BY facts.entry_id IS NOT NULL, facts.entry_id, facts.id",
        )?;
        let mut rows = statement.query([])?;
        let mut material = Self {
            facts: Vec::new(),
            entries: Vec::new(),
            name: None,
            contacts: Vec::new(),
            listing: String::new(),
            now: this_month(),
        };
        let mut entry_ids: Vec<Uuid> = Vec::new();
        while let Some(row) = rows.next()? {
            let kind: String = row.get(1)?;
            let Some(kind) = FactKind::from_stored(&kind) else {
                continue;
            };
            let entry_id: Option<Uuid> = row.get(3)?;
            let mut given = Given {
                id: FactId(row.get(0)?),
                kind,
                text: row.get(2)?,
                entry: None,
            };
            match kind {
                FactKind::Name => {
                    material.name.get_or_insert(given);
                    continue;
                }
                FactKind::Contact => {
                    material.contacts.push(given);
                    continue;
                }
                _ => {}
            }
            if let Some(entry_id) = entry_id {
                if entry_ids.last() != Some(&entry_id) {
                    let entry_kind: Option<String> = row.get(4)?;
                    entry_ids.push(entry_id);
                    material.entries.push(GivenEntry {
                        kind: entry_kind
                            .as_deref()
                            .and_then(EntryKind::from_stored)
                            .unwrap_or(EntryKind::Other),
                        section: row.get(5)?,
                        facts: Vec::new(),
                    });
                }
                let entry = material.entries.len() - 1;
                given.entry = Some(entry);
                material.entries[entry].facts.push(material.facts.len());
            }
            material.facts.push(given);
        }
        material.listing = material.list();
        Ok(material)
    }

    /// What's sent of the person: the listing, then their `instructions` for
    /// this résumé, if any. Redacted as one, so their name is hidden in both.
    pub fn sent(&self, instructions: Option<&str>) -> String {
        match instructions {
            Some(instructions) => format!(
                "{}{INSTRUCTIONS_HEADING}{}\n",
                self.listing,
                instructions.trim()
            ),
            None => self.listing.clone(),
        }
    }

    /// How many whole years the jobs' dates cover, a month two jobs share
    /// counted once, and the date facts that say so. A year with no month
    /// counts from its end when a job starts in it and to its start when one
    /// ends in it, so a guess never adds time.
    fn experience(&self) -> (u32, Vec<usize>) {
        let mut dates = Vec::new();
        let mut spans: Vec<Range<i32>> = Vec::new();
        for entry in self
            .entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::Experience)
        {
            for index in &entry.facts {
                let fact = &self.facts[*index];
                if fact.kind == FactKind::Date
                    && let Some(span) = span(&fact.text, self.now)
                {
                    dates.push(*index);
                    spans.push(span);
                }
            }
        }
        (years_covered(spans), dates)
    }

    /// Whether there's nothing to make a résumé from but a name and contacts.
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    /// The facts as the model is given them: those in no entry first, then
    /// each entry with its facts.
    fn list(&self) -> String {
        let mut listing = String::new();
        let line = |listing: &mut String, index: usize| {
            let fact = &self.facts[index];
            listing.push_str(&format!(
                "F{} {}: {}\n",
                index + 1,
                fact.kind.stored(),
                fact.text.split_whitespace().collect::<Vec<_>>().join(" ")
            ));
        };
        let loose: Vec<usize> = (0..self.facts.len())
            .filter(|index| self.facts[*index].entry.is_none())
            .collect();
        if !loose.is_empty() {
            listing.push_str("In no entry:\n");
            for index in loose {
                line(&mut listing, index);
            }
        }
        for (number, entry) in self.entries.iter().enumerate() {
            let kind = match entry.kind {
                EntryKind::Experience => "a job".to_owned(),
                EntryKind::Education => "a degree".to_owned(),
                EntryKind::Project => "a project".to_owned(),
                EntryKind::Certification => "a certification".to_owned(),
                EntryKind::Skills => "a group of skills".to_owned(),
                EntryKind::Other => match &entry.section {
                    Some(section) => format!("other items, under \"{section}\""),
                    None => "other items".to_owned(),
                },
            };
            listing.push_str(&format!("\nE{}, {kind}:\n", number + 1));
            for index in &entry.facts {
                line(&mut listing, *index);
            }
        }
        listing
    }

    /// The fact numbered `id`, like "F12", as the model refers to it.
    fn fact(&self, id: &str) -> Option<usize> {
        number(id, 'F').filter(|index| *index < self.facts.len())
    }

    fn entry(&self, id: &str) -> Option<usize> {
        number(id, 'E').filter(|index| *index < self.entries.len())
    }
}

/// The request asking the model for a résumé for `job` from `redacted`, what
/// the material sends (`Material::sent`) as redacted. It's asked not to
/// reason, where the model can stop: it's slower and costs more, and nothing
/// it reasons is checked.
pub fn request(redacted: &str, job: &Job) -> CompletionRequest {
    let prompt = format!(
        "The facts:\n{redacted}\nThe job: {} at {}\n\n{}",
        job.title, job.company, job.description
    );
    CompletionRequest::new(prompt)
        .preamble(PREAMBLE)
        .temperature(0.)
        .max_tokens(MAX_TOKENS)
        .options(
            GenerationOptions::new()
                .reasoning(Reasoning::Off)
                .on_unsupported(OnUnsupported::Ignore),
        )
}

impl Answer {
    /// Reads the model's answer, even wrapped in a Markdown code block, or
    /// with ids written bare, as `[F12, F14]`: DeepSeek's JSON mode let
    /// that through for a CV of 95 facts.
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        let json = match (text.find('{'), text.rfind('}')) {
            (Some(start), Some(end)) if start < end => &text[start..=end],
            _ => text,
        };
        serde_json::from_str(json)
            .or_else(|error| serde_json::from_str(&quoted_ids(json)).map_err(|_| error))
    }
}

impl Draft {
    /// The résumé `answer` describes, from `material`, what it sent
    /// (`sent`) redacted as `redaction`: each line the model wrote checked
    /// against the facts it cites.
    pub fn new(answer: &Answer, material: &Material, sent: &str, redaction: &Redaction) -> Self {
        let given: Vec<GivenFact> = material
            .facts
            .iter()
            .map(|fact| GivenFact {
                id: fact.id,
                text: &fact.text,
                hidden: false,
            })
            .collect();
        let mut claims = Vec::new();
        let (years, dates) = material.experience();
        // A line the model wrote, citing those of `facts` that `allowed`
        // lets it, if the verifier passes it. With `summary`, it may also
        // say how many years the person has worked, as the jobs' dates
        // count them, and cites those dates.
        let mut written =
            |written: &Written, allowed: &dyn Fn(usize) -> bool, summary: bool| -> Option<Line> {
                let text = redaction.restore(written.text.trim(), sent);
                if text.is_empty() {
                    return None;
                }
                let mut cites: Vec<FactId> = Vec::new();
                for index in written.facts.iter().filter_map(|id| material.fact(id)) {
                    let id = material.facts[index].id;
                    if allowed(index) && !cites.contains(&id) {
                        cites.push(id);
                    }
                }
                let Some(reason) = verifier::verify(&text, &cites, &given) else {
                    return Some(line(text, cites));
                };
                if summary
                    && let Some(rest) = without_years_worked(&text, years)
                    && verifier::verify(&rest, &cites, &given).is_none()
                {
                    for date in &dates {
                        let id = material.facts[*date].id;
                        if !cites.contains(&id) {
                            cites.push(id);
                        }
                    }
                    return Some(line(text, cites));
                }
                claims.push((text, reason));
                None
            };

        let mut sections = vec![Section {
            kind: SectionKind::Header,
            title: String::new(),
            blocks: vec![Block::Header {
                name: material.name.as_ref().map_or_else(
                    || line(String::new(), Vec::new()),
                    |name| line(name.text.clone(), vec![name.id]),
                ),
                contacts: material
                    .contacts
                    .iter()
                    .map(|contact| line(contact.text.clone(), vec![contact.id]))
                    .collect(),
            }],
        }];
        if let Some(summary) = answer
            .summary
            .as_ref()
            .and_then(|summary| written(summary, &|_| true, true))
        {
            sections.push(Section {
                kind: SectionKind::Summary,
                title: "Summary".to_owned(),
                blocks: vec![Block::Text(summary)],
            });
        }

        // Entries are shown in the order they were found, which is the
        // documents' own; the model only picks them.
        let mut chosen: Vec<(usize, &ChosenEntry)> = answer
            .entries
            .iter()
            .filter_map(|chosen| Some((material.entry(&chosen.entry)?, chosen)))
            .collect();
        chosen.sort_by_key(|(entry, _)| *entry);
        chosen.dedup_by_key(|(entry, _)| *entry);
        let mut entry_blocks = |kind: EntryKind| -> Vec<(Option<String>, Block)> {
            chosen
                .iter()
                .filter(|(entry, _)| material.entries[*entry].kind == kind)
                .filter_map(|(entry, chosen)| {
                    let in_entry = |index: usize| material.facts[index].entry == Some(*entry);
                    let bullets = chosen
                        .bullets
                        .iter()
                        .filter_map(|bullet| written(bullet, &in_entry, false))
                        .collect();
                    let block = entry_block(material, *entry, &chosen.skills, bullets)?;
                    Some((
                        material.entries[*entry].section.clone(),
                        Block::Entry(block),
                    ))
                })
                .collect()
        };
        let mut shown = Vec::new();
        for (kind, section, title) in [
            (EntryKind::Experience, SectionKind::Experience, "Experience"),
            (EntryKind::Project, SectionKind::Projects, "Projects"),
            (EntryKind::Education, SectionKind::Education, "Education"),
            (EntryKind::Certification, SectionKind::Other, CERTIFICATIONS),
        ] {
            let blocks: Vec<Block> = entry_blocks(kind)
                .into_iter()
                .map(|(_, block)| block)
                .collect();
            shown.push((section, title.to_owned(), blocks));
        }
        // Other entries under their section's name, as the documents had them.
        let others = entry_blocks(EntryKind::Other);
        for (name, block) in others {
            let title = name.unwrap_or_else(|| super::commands::OTHER_SECTION.to_owned());
            match shown
                .iter_mut()
                .find(|(kind, shown, _)| *kind == SectionKind::Other && *shown == title)
            {
                Some((_, _, blocks)) => blocks.push(block),
                None => shown.push((SectionKind::Other, title, vec![block])),
            }
        }
        sections.extend(
            shown
                .into_iter()
                .filter(|(_, _, blocks)| !blocks.is_empty())
                .map(|(kind, title, blocks)| Section {
                    kind,
                    title,
                    blocks,
                }),
        );

        let mut skills: Vec<(usize, &ChosenSkills)> = answer
            .skills
            .iter()
            .filter_map(|chosen| Some((material.entry(&chosen.entry)?, chosen)))
            .filter(|(entry, _)| material.entries[*entry].kind == EntryKind::Skills)
            .collect();
        skills.sort_by_key(|(entry, _)| *entry);
        skills.dedup_by_key(|(entry, _)| *entry);
        let blocks: Vec<Block> = skills
            .iter()
            .filter_map(|(entry, chosen)| {
                let items = skill_line(material, *entry, &chosen.skills)?;
                let label = material.entries[*entry]
                    .facts
                    .iter()
                    .map(|index| &material.facts[*index])
                    .find(|fact| fact.kind == FactKind::SkillGroup)
                    .map(|label| line(label.text.clone(), vec![label.id]));
                Some(Block::Skills { label, items })
            })
            .collect();
        if !blocks.is_empty() {
            sections.push(Section {
                kind: SectionKind::Skills,
                title: "Skills".to_owned(),
                blocks,
            });
        }

        // Facts in no entry, like talks, as the documents had them.
        let mut items: Vec<usize> = answer
            .items
            .iter()
            .filter_map(|id| material.fact(id))
            .filter(|index| {
                let fact = &material.facts[*index];
                fact.entry.is_none() && fact.kind != FactKind::Summary
            })
            .collect();
        items.sort_unstable();
        items.dedup();
        if !items.is_empty() {
            let blocks = items.into_iter().map(|index| {
                let fact = &material.facts[index];
                Block::Item(line(fact.text.clone(), vec![fact.id]))
            });
            let other = super::commands::OTHER_SECTION;
            match sections
                .iter_mut()
                .find(|section| section.kind == SectionKind::Other && section.title == other)
            {
                // Before its entries: after one, an item reads as its bullet.
                Some(section) => {
                    section.blocks.splice(0..0, blocks);
                }
                None => sections.push(Section {
                    kind: SectionKind::Other,
                    title: other.to_owned(),
                    blocks: blocks.collect(),
                }),
            }
        }
        // Skills before the sections the user named, as most résumés have them.
        sections.sort_by_key(|section| {
            section.kind == SectionKind::Other && section.title != CERTIFICATIONS
        });

        let facts = material
            .name
            .iter()
            .chain(&material.contacts)
            .chain(&material.facts)
            .map(|fact| (fact.id, fact.text.clone()))
            .collect();
        Self {
            sections,
            claims,
            facts,
        }
    }
}

/// Saves `draft` as the job's current résumé, laid out with the default
/// template on `paper`.
pub fn save(
    write: &mut Write,
    job: JobId,
    draft: &Draft,
    paper: Paper,
    instructions: Option<&str>,
) -> Result<ResumeId, Error> {
    let generation = Uuid::now_v7();
    let template = typeset::template(typeset::DEFAULT_TEMPLATE);
    write.insert(
        Generations,
        generation,
        &[
            generations::JOB_ID.value(&job.0),
            generations::KIND.value(&"resume"),
            generations::GENERATED_AT.value(&write.now()),
            generations::TEMPLATE_VERSION_ID.value(&Some(template.version_id)),
            generations::PAPER.value(&Some(paper.stored())),
            generations::INSTRUCTIONS.value(&instructions),
        ],
    )?;
    for (position, section) in draft.sections.iter().enumerate() {
        let id = Uuid::now_v7();
        write.insert(
            Sections,
            id,
            &[
                sections::GENERATION_ID.value(&generation),
                sections::SORT_KEY.value(&sort_key(position)),
                sections::KIND.value(&section.kind.stored()),
                sections::TITLE.value(&section.title),
            ],
        )?;
        for (position, block) in section.blocks.iter().enumerate() {
            save_block(write, id, position, block)?;
        }
    }
    for (id, text) in &draft.facts {
        write.insert(
            FactVersions,
            (generation, id.0),
            &[
                fact_versions::TEXT.value(text),
                fact_versions::HIDDEN.value(&0),
            ],
        )?;
    }
    for (position, (text, reason)) in draft.claims.iter().enumerate() {
        let (stored, figure, year, hidden) = match reason {
            UnsupportedReason::InventedFigure(figure) => {
                ("invented_figure", Some(figure.as_str()), None, None)
            }
            UnsupportedReason::InventedYear(year) => {
                ("invented_year", None, Some(i64::from(*year)), None)
            }
            UnsupportedReason::NoSupportingFact => ("no_supporting_fact", None, None, None),
            UnsupportedReason::UsesHiddenFact(fact) => {
                ("uses_hidden_fact", None, None, Some(fact.0))
            }
        };
        write.insert(
            UnsupportedClaims,
            Uuid::now_v7(),
            &[
                unsupported_claims::GENERATION_ID.value(&generation),
                unsupported_claims::SORT_KEY.value(&sort_key(position)),
                unsupported_claims::TEXT.value(text),
                unsupported_claims::REASON.value(&stored),
                unsupported_claims::FIGURE.value(&figure),
                unsupported_claims::YEAR.value(&year),
                unsupported_claims::HIDDEN_FACT_ID.value(&hidden),
            ],
        )?;
    }
    Ok(ResumeId(generation))
}

/// The instructions the job's last résumé was written with, which the next
/// starts from.
pub fn last_instructions(connection: &Connection, job: JobId) -> Result<Option<String>, Error> {
    Ok(connection
        .prepare_cached(
            "SELECT instructions FROM generations
             WHERE job_id = ?1 AND kind = 'resume' AND deleted_at IS NULL
             ORDER BY generated_at DESC, id DESC LIMIT 1",
        )?
        .query_row([job.0], |row| row.get(0))
        .optional()?
        .flatten())
}

/// The paper a new résumé is laid out on: the one the user last chose, or
/// else letter where the locale's country uses it, and A4 elsewhere. The
/// locale is only a guess: many systems are set to en_US wherever they are.
pub fn paper(connection: &Connection) -> Result<Paper, Error> {
    let last: Option<Option<String>> = connection
        .prepare_cached(
            "SELECT paper FROM generations
             WHERE kind = 'resume' AND paper IS NOT NULL AND deleted_at IS NULL
             ORDER BY generated_at DESC, id DESC LIMIT 1",
        )?
        .query_row([], |row| row.get(0))
        .optional()?;
    if let Some(last) = last {
        return Ok(Paper::from_stored(last.as_deref()));
    }
    let locale = ["LC_PAPER", "LC_ALL", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_default();
    // The countries that use letter paper, by ISO 3166 code.
    const LETTER: [&str; 7] = ["_US", "_CA", "_MX", "_PH", "_CL", "_CO", "_VE"];
    Ok(if LETTER.iter().any(|country| locale.contains(country)) {
        Paper::Letter
    } else {
        Paper::A4
    })
}

/// Why `text`, a line the user edited, says more than the facts it `cites`,
/// if it does, as a generated line is checked. Like the summary it may say
/// how many years the person has worked, as far as the cited `dates` cover.
pub fn check_edit(
    text: &str,
    cites: &[FactId],
    given: &[GivenFact],
    dates: &[&str],
) -> Option<UnsupportedReason> {
    let reason = verifier::verify(text, cites, given)?;
    let now = this_month();
    let years = years_covered(dates.iter().filter_map(|dates| span(dates, now)).collect());
    without_years_worked(text, years)
        .filter(|rest| verifier::verify(rest, cites, given).is_none())
        .map_or(Some(reason), |_| None)
}

/// The month it is now, as `month` counts it.
fn this_month() -> i32 {
    let today = Local::now().date_naive();
    month(today.year(), today.month0())
}

/// How many whole years `spans` of months cover, a month two share counted
/// once.
fn years_covered(mut spans: Vec<Range<i32>>) -> u32 {
    spans.sort_by_key(|span| span.start);
    let mut months = 0;
    let mut counted_to = i32::MIN;
    for span in spans {
        let start = span.start.max(counted_to);
        if span.end > start {
            months += span.end - start;
            counted_to = span.end;
        }
    }
    u32::try_from(months / 12).unwrap_or_default()
}

/// `text` without where it says how many years the person has worked, if
/// it says so and never more than `years`.
fn without_years_worked(text: &str, years: u32) -> Option<String> {
    let worked = years_worked(text);
    if worked.is_empty() || worked.iter().any(|(_, claimed)| *claimed > years) {
        return None;
    }
    let mut rest = text.to_owned();
    for (range, _) in worked.iter().rev() {
        rest.replace_range(range.clone(), "");
    }
    Some(rest)
}

/// A month as a count from year 0, so months subtract.
fn month(year: i32, month0: u32) -> i32 {
    year * 12 + i32::try_from(month0).unwrap_or_default()
}

/// The months a job's dates, like "June 2021 – Present" or "2018 – 2021",
/// cover, "present" being `now`; `None` without a start and an end.
fn span(dates: &str, now: i32) -> Option<Range<i32>> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let dates = dates.to_lowercase();
    let mut parts = dates
        .split(['-', '\u{2013}', '\u{2014}', '\u{2212}'])
        .flat_map(|part| part.split(" to "));
    let (start, end) = (parts.next()?, parts.next()?);
    // A month named, or else the year's last month for a start and its
    // first for an end.
    let read = |part: &str, starts: bool| -> Option<i32> {
        let year = part
            .split(|character: char| !character.is_ascii_digit())
            .find_map(|digits| {
                digits
                    .parse::<i32>()
                    .ok()
                    .filter(|year| (1900..2100).contains(year))
            })?;
        let named = part
            .split(|character: char| !character.is_alphabetic())
            .find_map(|word| {
                MONTHS
                    .iter()
                    .position(|name| word.len() >= 3 && word.starts_with(name))
            });
        let month0 = named.map_or(if starts { 11 } else { 0 }, |month0| month0 as u32);
        Some(month(year, month0))
    };
    let start = read(start, true)?;
    let end = if ["present", "now", "current"]
        .iter()
        .any(|word| end.contains(word))
    {
        now
    } else {
        read(end, false)?
    };
    (end > start).then_some(start..end)
}

/// Where `text` says how many years the person has worked, as "5 years",
/// "5+ years" or "five years", with the number.
fn years_worked(text: &str) -> Vec<(Range<usize>, u32)> {
    const WORDS: [&str; 20] = [
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    let lower = text.to_lowercase();
    let mut found = Vec::new();
    for (at, _) in lower.match_indices("year") {
        let before = lower[..at].trim_end();
        let before = before.strip_suffix('+').unwrap_or(before).trim_end();
        let word_start = before
            .rfind(|character: char| !character.is_alphanumeric())
            .map_or(0, |index| index + 1);
        let word = &before[word_start..];
        let number = word.parse::<u32>().ok().or_else(|| {
            WORDS
                .iter()
                .position(|name| *name == word)
                .and_then(|index| u32::try_from(index + 1).ok())
        });
        let end = at
            + if lower[at..].starts_with("years") {
                5
            } else {
                4
            };
        if let Some(number) = number
            && !word.is_empty()
        {
            found.push((word_start..end, number));
        }
    }
    found
}

/// `json` with each id outside a string, like `F12` or `E3`, put in quotes.
fn quoted_ids(json: &str) -> String {
    let mut quoted = String::with_capacity(json.len());
    let mut characters = json.char_indices().peekable();
    let mut in_string = false;
    while let Some((index, character)) = characters.next() {
        if in_string {
            quoted.push(character);
            match character {
                '\\' => quoted.extend(characters.next().map(|(_, escaped)| escaped)),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        let digits = json[index + character.len_utf8()..]
            .find(|next: char| !next.is_ascii_digit())
            .unwrap_or(json.len() - index - character.len_utf8());
        if matches!(character, 'F' | 'E') && digits > 0 {
            quoted.push('"');
            quoted.push_str(&json[index..index + 1 + digits]);
            quoted.push('"');
            for _ in 0..digits {
                characters.next();
            }
            continue;
        }
        in_string = character == '"';
        quoted.push(character);
    }
    quoted
}

fn save_block(
    write: &mut Write,
    section: Uuid,
    position: usize,
    block: &Block,
) -> Result<(), Error> {
    let id = Uuid::now_v7();
    let (kind, lines): (&str, Vec<(&str, &Line)>) = match block {
        Block::Header { name, contacts } => (
            "header",
            std::iter::once(("name", name))
                .chain(contacts.iter().map(|contact| ("contact", contact)))
                .collect(),
        ),
        Block::Entry(entry) => (
            "entry",
            std::iter::once(("title", &entry.title))
                .chain(entry.organization.iter().map(|line| ("organization", line)))
                .chain(entry.dates.iter().map(|line| ("dates", line)))
                .chain(entry.location.iter().map(|line| ("location", line)))
                .chain(entry.stack.iter().map(|line| ("stack", line)))
                .chain(entry.bullets.iter().map(|line| ("bullet", line)))
                .collect(),
        ),
        Block::Skills { label, items } => (
            "skills",
            label
                .iter()
                .map(|line| ("label", line))
                .chain(std::iter::once(("items", items)))
                .collect(),
        ),
        Block::Text(line) => ("text", vec![("text", line)]),
        Block::Item(line) => ("item", vec![("item", line)]),
    };
    write.insert(
        Blocks,
        id,
        &[
            blocks::SECTION_ID.value(&section),
            blocks::SORT_KEY.value(&sort_key(position)),
            blocks::KIND.value(&kind),
        ],
    )?;
    for (position, (role, line)) in lines.into_iter().enumerate() {
        write.insert(
            Lines,
            line.id.0,
            &[
                lines::BLOCK_ID.value(&id),
                lines::ROLE.value(&role),
                lines::SORT_KEY.value(&sort_key(position)),
                lines::GENERATED.value(&line.generated),
                lines::TEXT.value(&line.text),
            ],
        )?;
        for (position, fact) in line.cites.iter().enumerate() {
            write.insert(
                LineCitations,
                (line.id.0, fact.0),
                &[line_citations::SORT_KEY.value(&sort_key(position))],
            )?;
        }
    }
    Ok(())
}

/// An entry's block: its title, organization, dates and location as its facts
/// have them, the skills chosen from its own as its stack, and `bullets`.
/// An entry with neither a title nor an organization has nothing to head it.
fn entry_block(
    material: &Material,
    entry: usize,
    skills: &[String],
    bullets: Vec<Line>,
) -> Option<Box<EntryBlock>> {
    let field = |kind: FactKind| {
        material.entries[entry]
            .facts
            .iter()
            .map(|index| &material.facts[*index])
            .find(|fact| fact.kind == kind)
            .map(|fact| line(fact.text.clone(), vec![fact.id]))
    };
    let mut organization = field(FactKind::Organization);
    let title = field(FactKind::Title).or_else(|| organization.take())?;
    Some(Box::new(EntryBlock {
        title,
        organization,
        dates: field(FactKind::Date),
        location: field(FactKind::Location),
        stack: skill_line(material, entry, skills),
        bullets,
    }))
}

/// The skills `chosen` of `entry`'s own, as one line citing each.
fn skill_line(material: &Material, entry: usize, chosen: &[String]) -> Option<Line> {
    let mut skills: Vec<usize> = Vec::new();
    for index in chosen.iter().filter_map(|id| material.fact(id)) {
        let fact = &material.facts[index];
        if fact.entry == Some(entry) && fact.kind == FactKind::Skill && !skills.contains(&index) {
            skills.push(index);
        }
    }
    if skills.is_empty() {
        return None;
    }
    let text = skills
        .iter()
        .map(|index| material.facts[*index].text.as_str())
        .collect::<Vec<_>>()
        .join(SKILL_SEPARATOR);
    Some(line(
        text,
        skills
            .iter()
            .map(|index| material.facts[*index].id)
            .collect(),
    ))
}

fn line(text: String, cites: Vec<FactId>) -> Line {
    Line {
        id: LineId::new(),
        generated: text.clone(),
        text,
        cites,
    }
}

/// The index `id` gives, like 11 for "F12" with `prefix` 'F': the model
/// numbers from one.
fn number(id: &str, prefix: char) -> Option<usize> {
    let id = id.trim();
    let digits = id.strip_prefix(prefix).unwrap_or(id);
    digits.parse::<usize>().ok()?.checked_sub(1)
}

/// Sort keys in order: zero-padded, so they sort as text.
fn sort_key(position: usize) -> String {
    format!("{position:04}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{redaction, sample};

    const STAND_IN: &str = "Jordan Ellis";

    /// The sample's material, and the draft `json` describes from it.
    fn draft(connection: &Connection, json: &str) -> (Material, Draft) {
        let material = match Material::read(connection) {
            Ok(material) => material,
            Err(error) => panic!("the sample's facts: {error}"),
        };
        let names = ["Alex Rahman"];
        let redaction = Redaction::new(
            &material.listing,
            &redaction::find(&material.listing, &names, &[]),
            STAND_IN,
        );
        let answer = match Answer::parse(json) {
            Ok(answer) => answer,
            Err(error) => panic!("a test answer that parses: {error}"),
        };
        let draft = Draft::new(&answer, &material, &material.listing, &redaction);
        (material, draft)
    }

    /// Each section as its title, then each line in reading order with the facts
    /// it cites.
    fn shown(sections: &[Section], material: &Material) -> Vec<String> {
        let number = |fact: &FactId| {
            material
                .facts
                .iter()
                .position(|given| given.id == *fact)
                .map_or("header".to_owned(), |index| format!("F{}", index + 1))
        };
        let mut shown = Vec::new();
        for section in sections {
            shown.push(format!("# {:?} {}", section.kind, section.title));
            for line in section.blocks.iter().flat_map(Block::lines) {
                let cites: Vec<String> = line.cites.iter().map(number).collect();
                shown.push(format!("{} [{}]", line.text, cites.join(" ")));
            }
        }
        shown
    }

    const ANSWER: &str = r#"```json
        {"summary": {"text": "Backend engineer with eight years of Rust, Go and Python, most recently leading a routing platform.",
                     "facts": ["F1"]},
         "entries": [
           {"entry": "E2"},
           {"entry": "E1", "bullets": [
              {"text": "Cut the routing API's p99 latency from 480 ms to 95 ms.", "facts": ["F10"]},
              {"text": "Led moving 12 backend services from Python to Rust, one service at a time.",
               "facts": ["F9", "F12", "F12"]},
              {"text": "Cut cloud costs by 40%.", "facts": ["F10"]},
              {"text": "Built the appointment scheduling service used by 40 clinics.", "facts": ["F23"]}]},
           {"entry": "E5", "bullets": [
              {"text": "An open-source Rust crate for trace context across async tasks, with about 900 GitHub stars.",
               "facts": ["F45"]}],
            "skills": ["F42", "F43", "F58"]},
           {"entry": "E7"},
           {"entry": "E11", "bullets": [
              {"text": "Ran a volunteer team of 15 students over three days.", "facts": ["F75"]}]},
           {"entry": "E99"}],
         "skills": [{"entry": "E8", "skills": ["F58", "F42", "F59"]}, {"entry": "E1", "skills": ["F9"]}],
         "items": ["F2", "F1"]}
        ```"#;

    #[test]
    fn lines_their_facts_back_are_kept_and_the_rest_listed_as_unsupported() {
        let writer = match sample::written() {
            Ok(writer) => writer,
            Err(error) => panic!("the sample: {error}"),
        };
        let (material, draft) = draft(writer.kron(), ANSWER);
        assert_eq!(
            shown(&draft.sections, &material),
            [
                "# Header ",
                "Alex Rahman [header]",
                "alex.rahman@example.com [header]",
                "+880 1711 555 210 [header]",
                "github.com/alexr [header]",
                "linkedin.com/in/alexrahman [header]",
                "# Summary Summary",
                "Backend engineer with eight years of Rust, Go and Python, most recently leading \
                 a routing platform. [F1]",
                "# Experience Experience",
                "Senior Software Engineer [F5]",
                "2021 – present [F8]",
                "Northwind Logistics [F6]",
                "Dhaka, Bangladesh [F7]",
                "Cut the routing API's p99 latency from 480 ms to 95 ms. [F10]",
                "Led moving 12 backend services from Python to Rust, one service at a time. [F9 F12]",
                "Backend Engineer [F19]",
                "2018 – 2021 [F22]",
                "Brightline Health [F20]",
                "Remote [F21]",
                "# Projects Projects",
                "tracewire [F40]",
                "Rust, Tokio [F42 F43]",
                "2022 – present [F41]",
                "An open-source Rust crate for trace context across async tasks, with about 900 \
                 GitHub stars. [F45]",
                "# Education Education",
                "B.Sc. Computer Science [F53]",
                "2014 – 2018 [F56]",
                "University of Dhaka [F54]",
                "Dhaka, Bangladesh [F55]",
                "# Skills Skills",
                "Languages [F57]",
                "Rust, Go [F58 F59]",
                "# Other Other",
                "Tracing async Rust in production, RustConf 2024. [F2]",
                "Organizer [F72]",
                "2019 [F74]",
                "University of Dhaka Programming Club [F73]",
                "Ran a volunteer team of 15 students over three days. [F75]",
            ],
            "the model's entries in the documents' order, each line citing only facts of its own \
             entry; Kron's lines as the facts have them"
        );
        assert_eq!(
            draft.claims,
            [
                (
                    "Cut cloud costs by 40%.".to_owned(),
                    UnsupportedReason::InventedFigure("40%".to_owned())
                ),
                (
                    "Built the appointment scheduling service used by 40 clinics.".to_owned(),
                    UnsupportedReason::NoSupportingFact
                ),
            ],
            "a figure no fact states, and a fact of another job"
        );
    }

    #[test]
    fn a_jobs_dates_are_read_as_the_months_they_cover() {
        let now = month(2026, 10);
        assert_eq!(span("June 2021 – Present", now), Some(month(2021, 5)..now));
        assert_eq!(
            span("Jan 2024 - May 2024", now),
            Some(month(2024, 0)..month(2024, 4))
        );
        assert_eq!(
            span("2018 – 2021", now),
            Some(month(2018, 11)..month(2021, 0)),
            "a year alone adds no time it may not have had"
        );
        assert_eq!(span("2015", now), None);
        assert_eq!(span("May 2024 – Jan 2024", now), None);
    }

    #[test]
    fn an_edit_is_checked_against_its_facts_and_the_years_their_dates_cover() {
        let (bullet, dates) = (FactId(Uuid::now_v7()), FactId(Uuid::now_v7()));
        let given = [
            GivenFact {
                id: bullet,
                text: "Led the migration of 12 backend services from Python to Rust.",
                hidden: false,
            },
            GivenFact {
                id: dates,
                text: "Jan 2010 – Jan 2020",
                hidden: false,
            },
        ];
        let check =
            |text: &str| check_edit(text, &[bullet, dates], &given, &["Jan 2010 – Jan 2020"]);
        assert_eq!(check("Moved 12 backend services to Rust."), None);
        assert_eq!(
            check("Moved 14 backend services to Rust."),
            Some(UnsupportedReason::InventedFigure("14".to_owned()))
        );
        assert_eq!(
            check("Ten years of moving backend services to Rust, 12 of them."),
            None
        );
        assert!(check("11 years of moving backend services to Rust.").is_some());
        assert!(
            check_edit(
                "10 years of backend services in Rust.",
                &[bullet],
                &given,
                &[]
            )
            .is_some(),
            "without dates there are no years to say"
        );
    }

    #[test]
    fn the_summary_may_say_the_years_the_jobs_dates_cover() {
        let writer = match sample::written() {
            Ok(writer) => writer,
            Err(error) => panic!("the sample: {error}"),
        };
        let mut material = match Material::read(writer.kron()) {
            Ok(material) => material,
            Err(error) => panic!("the sample's facts: {error}"),
        };
        // 2016 – 2018, 2018 – 2021 and 2021 – present: 97 months.
        material.now = month(2026, 10);
        assert_eq!(material.experience().0, 8);
        let summary = |text: &str| {
            let json = format!(r#"{{"summary": {{"text": "{text}", "facts": ["F9"]}}}}"#);
            let answer = match Answer::parse(&json) {
                Ok(answer) => answer,
                Err(error) => panic!("{json}: {error}"),
            };
            let redaction = Redaction::new(&material.listing, &[], STAND_IN);
            let draft = Draft::new(&answer, &material, &material.listing, &redaction);
            let kept = draft
                .sections
                .iter()
                .find(|section| section.kind == SectionKind::Summary)
                .and_then(|section| section.blocks.first())
                .map(|block| match block {
                    Block::Text(line) => line.cites.len(),
                    _ => 0,
                });
            (kept, draft.claims.len())
        };
        assert_eq!(
            summary("Backend engineer with 8+ years, moving 12 services from Python to Rust."),
            (Some(4), 0),
            "kept, citing the three jobs' dates too"
        );
        assert_eq!(
            summary("Backend engineer with seven years, moving 12 services from Python to Rust."),
            (Some(4), 0)
        );
        assert_eq!(
            summary("Backend engineer with 9 years, moving 12 services from Python to Rust."),
            (None, 1),
            "more than the dates cover"
        );
        assert_eq!(
            summary("Backend engineer with 8 years, moving 30 services from Python to Rust."),
            (None, 1),
            "the rest of it is still checked"
        );
    }

    #[test]
    fn ids_written_bare_are_read_and_text_like_them_is_left() {
        let answer = Answer::parse(
            r#"{"summary": {"text": "Cut F1 \"E2\" time.", "facts": [F1, F12]},
                "entries": [{"entry": E3, "bullets": [{"text": "Ran F9.", "facts": [F4]}],
                             "skills": [F5]}],
                "skills": [{"entry": E6, "skills": [F7,F8]}], "items": [F10]}"#,
        );
        let answer = match answer {
            Ok(answer) => answer,
            Err(error) => panic!("bare ids are read: {error}"),
        };
        let summary = answer.summary.unwrap_or_default();
        assert_eq!(summary.text, "Cut F1 \"E2\" time.");
        assert_eq!(summary.facts, ["F1", "F12"]);
        let entry = answer.entries.first().map(|entry| {
            let bullet = entry
                .bullets
                .first()
                .map(|bullet| (&bullet.text, &bullet.facts));
            (&entry.entry, bullet, &entry.skills)
        });
        let (bullet_text, bullet_facts) = (String::from("Ran F9."), vec![String::from("F4")]);
        assert_eq!(
            entry,
            Some((
                &"E3".to_owned(),
                Some((&bullet_text, &bullet_facts)),
                &vec!["F5".to_owned()]
            ))
        );
        let skills = answer
            .skills
            .first()
            .map(|skills| (&skills.entry, &skills.skills));
        assert_eq!(
            skills,
            Some((&"E6".to_owned(), &vec!["F7".to_owned(), "F8".to_owned()]))
        );
        assert_eq!(answer.items, ["F10"]);
    }

    #[test]
    fn the_name_and_contacts_are_never_sent() {
        let writer = match sample::written() {
            Ok(writer) => writer,
            Err(error) => panic!("the sample: {error}"),
        };
        let (material, _) = draft(writer.kron(), "{}");
        for kept in material.name.iter().chain(&material.contacts) {
            assert!(!material.listing.contains(&kept.text), "{}", kept.text);
        }
        assert!(material.listing.contains("F1 summary: Backend engineer"));
    }

    #[test]
    fn instructions_are_sent_after_the_facts_with_the_name_hidden() {
        let writer = match sample::written() {
            Ok(writer) => writer,
            Err(error) => panic!("the sample: {error}"),
        };
        let (material, _) = draft(writer.kron(), "{}");
        assert_eq!(material.sent(None), material.listing);
        let sent = material.sent(Some("  Lead with Alex Rahman's talks.\n"));
        assert!(sent.starts_with(&material.listing));
        assert!(
            sent.ends_with("Their instructions for this résumé:\nLead with Alex Rahman's talks.\n")
        );
        let redaction = Redaction::new(
            &sent,
            &redaction::find(&sent, &["Alex Rahman"], &[]),
            STAND_IN,
        );
        assert!(
            !redaction.text.contains("Alex"),
            "the name in them is hidden too"
        );
        assert!(redaction.text.contains("Lead with Jordan Ellis's talks."));
    }

    #[test]
    fn a_resume_keeps_its_instructions_and_the_next_starts_from_them() -> Result<(), Error> {
        let mut writer = sample::written()?;
        let (_, first) = draft(writer.kron(), "{}");
        let job = sample::job(writer.kron(), "Cinder Labs")?;
        writer
            .execute(|write| save(write, job, &first, Paper::A4, Some("Lead with the talks.")))?;
        assert_eq!(
            last_instructions(writer.kron(), job)?.as_deref(),
            Some("Lead with the talks.")
        );
        let (_, again) = draft(writer.kron(), "{}");
        writer.execute(|write| save(write, job, &again, Paper::A4, None))?;
        assert_eq!(
            last_instructions(writer.kron(), job)?,
            None,
            "the latest's, even none"
        );
        Ok(())
    }

    #[test]
    fn a_new_resume_starts_on_the_paper_last_chosen() -> Result<(), Error> {
        let mut writer = sample::written()?;
        let job = sample::job(writer.kron(), "Cinder Labs")?;
        for chosen in [Paper::A4, Paper::Letter] {
            let (_, draft) = draft(writer.kron(), "{}");
            writer.execute(|write| save(write, job, &draft, chosen, None))?;
            assert_eq!(paper(writer.kron())?, chosen);
        }
        Ok(())
    }

    #[test]
    fn a_draft_saved_reads_back_as_it_was() -> Result<(), Error> {
        let mut writer = sample::written()?;
        let (material, draft) = draft(writer.kron(), ANSWER);
        let job = sample::job(writer.kron(), "Cinder Labs")?;
        let (resume, _) = writer.execute(|write| save(write, job, &draft, Paper::A4, None))?;
        let read = Section::read_all(writer.kron(), resume)?;
        assert_eq!(shown(&read, &material), shown(&draft.sections, &material));
        let ids = |sections: &[Section]| -> Vec<LineId> {
            sections
                .iter()
                .flat_map(|section| &section.blocks)
                .flat_map(Block::lines)
                .map(|line| line.id)
                .collect()
        };
        assert_eq!(ids(&read), ids(&draft.sections));
        let current: Uuid = writer.kron().query_row(
            "SELECT generation_id FROM current_generations WHERE job_id = ?1 AND kind = 'resume'",
            [job.0],
            |row| row.get(0),
        )?;
        assert_eq!(current, resume.0, "the job's current résumé");
        let count = |sql: &str| -> Result<i64, Error> {
            Ok(writer.kron().query_row(sql, [resume.0], |row| row.get(0))?)
        };
        assert_eq!(
            count("SELECT count(*) FROM unsupported_claims WHERE generation_id = ?1")?,
            2
        );
        assert_eq!(
            count("SELECT count(*) FROM fact_versions WHERE generation_id = ?1")?,
            (material.facts.len() + material.contacts.len() + 1) as i64
        );
        Ok(())
    }
    /// Writes a résumé from the sample's facts for its Cinder Labs job with
    /// the provider set up in the development data, its key from the
    /// keyring, and prints it with the claims left out. It costs money and
    /// needs the network, so it runs only when asked:
    /// `cargo test live_generation -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_generation() -> Result<(), Box<dyn std::error::Error>> {
        let device = dirs::data_dir()
            .ok_or("no data directory")?
            .join("kron-dev/device.sqlite");
        let device = rusqlite::Connection::open_with_flags(
            device,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let assigned = crate::llm::assigned(&device, crate::llm::Purpose::GenerateResume)
            .map_err(|error| format!("{error:?}"))?
            .ok_or("no provider set up in the development data")?;
        let key = crate::llm::key(assigned.provider.id)?.ok_or("no key saved for the provider")?;
        let writer = sample::written().map_err(|error| error.to_string())?;
        let job = sample::job(writer.kron(), "Cinder Labs").map_err(|error| error.to_string())?;
        let (title, company, description): (String, String, String) = writer.kron().query_row(
            "SELECT title, company, text FROM jobs
             JOIN job_descriptions ON job_descriptions.job_id = jobs.id WHERE jobs.id = ?1",
            [job.0],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let material = Material::read(writer.kron()).map_err(|error| error.to_string())?;
        let names = redaction::names(writer.kron()).map_err(|error| error.to_string())?;
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let redaction = Redaction::new(
            &material.listing,
            &redaction::find(&material.listing, &names, &[]),
            STAND_IN,
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let started = std::time::Instant::now();
        let job = Job {
            title: &title,
            company: &company,
            description: &description,
        };
        let reply = runtime
            .block_on(crate::llm::call(
                &assigned,
                key,
                request(&redaction.text, &job),
            ))
            .map_err(|error| error.to_string())?;
        let draft = Draft::new(
            &Answer::parse(&reply.response.text())?,
            &material,
            &material.listing,
            &redaction,
        );
        for line in shown(&draft.sections, &material) {
            println!("{line}");
        }
        for (text, reason) in &draft.claims {
            println!("UNSUPPORTED {reason:?}: {text}");
        }
        let usage = &reply.response.usage;
        println!(
            "== {:?} in, {:?} out, ${:.5}, {:.1} s",
            usage.input_tokens,
            usage.output_tokens,
            reply.cost.unwrap_or(0.),
            started.elapsed().as_secs_f64()
        );
        assert!(draft.sections.len() > 2, "more than a header and a summary");
        Ok(())
    }
    /// Builds the test set of real output: reads each test CV, finds its
    /// facts and writes résumés for its jobs in `generation/jobs.txt` with
    /// the provider set up in the development data, then writes every line
    /// the model wrote, with the facts it cites and what the verifier said,
    /// to `verifier/generated.txt`, labelled "?" for a person to label. It
    /// costs money, needs the network and overwrites that file, so it runs
    /// only when asked:
    /// `cargo test write_test_set -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn write_test_set() -> Result<(), Box<dyn std::error::Error>> {
        use crate::model::extraction;

        const JOBS: &str = include_str!("generation/jobs.txt");
        const CVS: [(&str, &[u8]); 4] = [
            ("junior", include_bytes!("extraction/junior.pdf")),
            ("senior", include_bytes!("extraction/senior.pdf")),
            ("columns", include_bytes!("extraction/columns.pdf")),
            ("linkedin", include_bytes!("extraction/linkedin.pdf")),
        ];
        let device = dirs::data_dir()
            .ok_or("no data directory")?
            .join("kron-dev/device.sqlite");
        let device = rusqlite::Connection::open_with_flags(
            device,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let assigned = crate::llm::assigned(&device, crate::llm::Purpose::GenerateResume)
            .map_err(|error| format!("{error:?}"))?
            .ok_or("no provider set up in the development data")?;
        let key = crate::llm::key(assigned.provider.id)?.ok_or("no key saved for the provider")?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let call = |request| {
            runtime
                .block_on(crate::llm::call(&assigned, key.clone(), request))
                .map_err(|error| error.to_string())
        };
        let jobs: Vec<(&str, &str, &str, String)> = JOBS
            .split("\n===\n")
            .filter_map(|block| {
                let mut lines = block.lines().filter(|line| !line.starts_with('#'));
                let mut field = |name: &str| {
                    lines
                        .find(|line| !line.is_empty())
                        .and_then(|line| line.strip_prefix(name))
                };
                let (cv, title, company) = (field("cv: ")?, field("title: ")?, field("company: ")?);
                let description: Vec<&str> = lines.collect();
                Some((cv, title, company, description.join("\n").trim().to_owned()))
            })
            .collect();

        let mut set = String::from(
            "# Every line the model wrote in résumés for the test CVs and the jobs in\n\
             # `generation/jobs.txt`, with the facts it cites: written by\n\
             # `generation::tests::write_test_set`, then labelled by hand, in\n\
             # `cases.txt`'s format and labels. Each case's comment says where it's\n\
             # from and what the verifier said when it was written.\n",
        );
        let mut cost = 0.;
        for (cv, pdf) in CVS {
            let reading = crate::import::read(crate::import::Format::Pdf, pdf.to_vec())
                .map_err(|error| format!("{error:?}"))?;
            let guessed = crate::import::name::guess(&reading);
            let names: Vec<&str> = guessed.as_deref().into_iter().collect();
            let redaction = Redaction::new(
                &reading.text,
                &redaction::find(&reading.text, &names, &[]),
                STAND_IN,
            );
            let reply = call(extraction::request(&redaction.text))?;
            cost += reply.cost.unwrap_or(0.);
            let found = extraction::Extraction::locate(
                extraction::Answer::parse(&reply.response.text())?,
                &reading.text,
                &redaction,
            );
            let mut writer =
                crate::database::Writer::open_in_memory().map_err(|error| error.to_string())?;
            writer
                .execute(|write| {
                    let document =
                        crate::model::commands::import_document(write, None, &reading.text)?;
                    extraction::save(write, document, &found)
                })
                .map_err(|error| error.to_string())?;
            let material = Material::read(writer.kron()).map_err(|error| error.to_string())?;
            let listed = Redaction::new(
                &material.listing,
                &redaction::find(&material.listing, &names, &[]),
                STAND_IN,
            );

            for (_, title, company, description) in jobs.iter().filter(|job| job.0 == cv) {
                let job = Job {
                    title,
                    company,
                    description,
                };
                let reply = call(request(&listed.text, &job))?;
                cost += reply.cost.unwrap_or(0.);
                let answer = Answer::parse(&reply.response.text())?;
                let given: Vec<GivenFact> = material
                    .facts
                    .iter()
                    .map(|fact| GivenFact {
                        id: fact.id,
                        text: &fact.text,
                        hidden: false,
                    })
                    .collect();
                // As `Draft::new` takes them: a bullet cites only facts of
                // its own entry.
                let mut written: Vec<(&str, Option<usize>, &Written)> = Vec::new();
                written.extend(
                    answer
                        .summary
                        .iter()
                        .map(|summary| ("summary", None, summary)),
                );
                for chosen in &answer.entries {
                    let entry = material.entry(&chosen.entry);
                    written.extend(
                        chosen
                            .bullets
                            .iter()
                            .map(|bullet| ("bullet", entry, bullet)),
                    );
                }
                let mut lines = 0;
                for (kind, entry, line) in written {
                    let text = listed.restore(line.text.trim(), &material.listing);
                    let cited: Vec<usize> = line
                        .facts
                        .iter()
                        .filter_map(|id| material.fact(id))
                        .filter(|index| {
                            entry.is_none_or(|entry| material.facts[*index].entry == Some(entry))
                        })
                        .fold(Vec::new(), |mut cited, index| {
                            if !cited.contains(&index) {
                                cited.push(index);
                            }
                            cited
                        });
                    let cites: Vec<FactId> = cited
                        .iter()
                        .map(|index| material.facts[*index].id)
                        .collect();
                    let verdict = match verifier::verify(&text, &cites, &given) {
                        None => "passed".to_owned(),
                        Some(reason) => format!("rejected: {reason:?}"),
                    };
                    set.push_str(&format!(
                        "\n?\n# {cv}, {company}, {kind}; the verifier {verdict}\n"
                    ));
                    if cited.is_empty() {
                        set.push_str("# cites no fact of its own entry\n");
                    }
                    for index in &cited {
                        set.push_str(&format!(
                            "fact: {}\n",
                            material.facts[*index]
                                .text
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ")
                        ));
                    }
                    set.push_str(&format!("line: {text}\n"));
                    lines += 1;
                }
                println!("{cv}, {company}: {lines} lines written");
            }
        }
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/model/verifier/generated.txt"
        );
        std::fs::write(path, set)?;
        println!("== total ${cost:.5}");
        Ok(())
    }
}
