// The meaning check: once Kron's rules have passed a résumé's lines, the
// user's AI service reads each line beside the facts it cites and names the
// ones that claim more than those facts say, as "fixed" for "found", or "a
// course" for "a guest lecture": what no list of words can catch. It only
// adds rejections. A line it passes was still passed by the rules, and when
// its answer can't be had or read, the lines stay as the rules left them.
//
// Only a line with words its facts don't have is checked, and a flag stands
// only if it names one of them. Measured on `verifier/cases.txt`, every real
// catch named such a word ("Fixed", "course", "Kubernetes"), and every honest
// line flagged named only its facts' own words: the model misreading.

use rig_core::completion::CompletionRequest;
use rig_core::completion::options::{GenerationOptions, OnUnsupported, Reasoning};
use serde::Deserialize;

use super::LineId;
use super::verifier;

const PREAMBLE: &str = r#"You check the lines of a résumé against the facts each was written from. Lines are numbered, like L3, each followed by its facts.

For each line, decide whether it claims anything its facts don't say.

Fine, and not a problem:
- rewording, and words that mean the same as the facts' words
- saying less: shorter, or leaving details out
- joining its facts, or putting the part that matters most first: check each part of a line against all of its facts, as a part any one of them says is fine
- a summary that sums up what its facts say
- plain verbs for the same act: built, made, added, shipped, created

A problem:
- a bigger part in the work than the facts give: helped or worked on becomes led, reviewed becomes owned
- a result the facts don't state: found becomes fixed, proposed becomes shipped
- a bigger or different thing: a guest lecture becomes a course, a guide becomes the standards
- a skill, tool, title, number or scope the facts don't state
- certain where the facts are hedged: planned, tried, started

Answer with one JSON object:
{"lines": [{"line": the line's number, like "L3", "phrase": the words of the line that claim more, copied exactly, "why": a few words}]}

List only lines with a problem; with none, answer {"lines": []}. When unsure, leave a line out: a line you list is taken off the résumé."#;
/// A checked line's answer is a few dozen tokens; this leaves room for a
/// long résumé with many flagged.
const MAX_TOKENS: u64 = 4_000;

/// A line the rules passed, as the model is given it: redacted, with the
/// redacted text of each fact it cites.
#[derive(Clone, Debug)]
pub struct Checked {
    pub line: LineId,
    pub text: String,
    pub facts: Vec<String>,
    /// Its words its facts don't have, which a flag must name.
    pub new: Vec<String>,
}

/// The words of `line` its `facts` don't have: a line without any can't
/// claim more by its wording, and isn't worth checking.
pub fn new_words(line: &str, facts: &[&str]) -> Vec<String> {
    verifier::new_words(line, facts)
        .into_iter()
        .filter_map(|range| line.get(range))
        .map(str::to_lowercase)
        .collect()
}

/// Whether a flag on `phrase` stands: it names one of the line's `new`
/// words. One naming only its facts' words is the model misreading them.
pub fn stands(phrase: &str, new: &[String]) -> bool {
    let phrase = phrase.to_lowercase();
    new.iter().any(|word| phrase.contains(word.as_str()))
}

/// A line the check says claims more than its facts.
#[derive(Debug, PartialEq, Eq)]
pub struct Flag {
    /// Its place in the lines checked.
    pub index: usize,
    /// The words that claim more, as the model copied them: redacted.
    pub phrase: String,
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    lines: Vec<Flagged>,
}

#[derive(Deserialize)]
struct Flagged {
    line: String,
    #[serde(default)]
    phrase: String,
}

/// Asks which of `lines` claim more than their facts.
pub fn request(lines: &[Checked]) -> CompletionRequest {
    let mut prompt = String::new();
    for (index, line) in lines.iter().enumerate() {
        prompt.push_str(&format!("L{}: {}\n", index + 1, line.text));
        for fact in &line.facts {
            prompt.push_str(&format!("  fact: {fact}\n"));
        }
    }
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

/// The lines the answer `text` flags, of `checked` lines, even wrapped in a
/// Markdown code block. A line number it made up is passed over.
pub fn flags(text: &str, checked: usize) -> Result<Vec<Flag>, serde_json::Error> {
    let json = match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if start < end => &text[start..=end],
        _ => text,
    };
    let answer: Answer = serde_json::from_str(json)?;
    let mut flags: Vec<Flag> = Vec::new();
    for flagged in answer.lines {
        let index = flagged
            .line
            .trim()
            .trim_start_matches(['L', 'l'])
            .parse::<usize>()
            .ok()
            .and_then(|number| number.checked_sub(1))
            .filter(|index| *index < checked);
        if let Some(index) = index
            && !flags.iter().any(|flag| flag.index == index)
        {
            flags.push(Flag {
                index,
                phrase: flagged.phrase.trim().to_owned(),
            });
        }
    }
    Ok(flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_line_is_sent_numbered_with_its_facts() {
        let lines = [Checked {
            line: LineId::new(),
            text: "Fixed two bottlenecks.".to_owned(),
            facts: vec!["Found two bottlenecks.".to_owned()],
            new: vec!["fixed".to_owned()],
        }];
        let request = request(&lines);
        let sent = format!("{request:?}");
        assert!(sent.contains("L1: Fixed two bottlenecks."), "{sent}");
        assert!(sent.contains("fact: Found two bottlenecks."), "{sent}");
    }

    #[test]
    fn the_answer_names_lines_by_number_and_a_made_up_one_is_passed_over() {
        let answer = r#"```json
            {"lines": [{"line": "L2", "phrase": "Fixed", "why": "found, not fixed"},
                       {"line": "L2", "phrase": "again"},
                       {"line": "L9", "phrase": "no such line"},
                       {"line": "l1", "phrase": " a course "}]}
            ```"#;
        let flags = match flags(answer, 3) {
            Ok(flags) => flags,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(
            flags,
            [
                Flag {
                    index: 1,
                    phrase: "Fixed".to_owned()
                },
                Flag {
                    index: 0,
                    phrase: "a course".to_owned()
                },
            ]
        );
        assert!(matches!(self::flags(r#"{"lines": []}"#, 3), Ok(flags) if flags.is_empty()));
        assert!(self::flags("not JSON", 3).is_err());
    }

    #[test]
    fn a_flag_stands_only_on_a_word_the_facts_dont_have() {
        let facts = ["Found two capacity bottlenecks in the routing API."];
        let new = new_words("Fixed two capacity bottlenecks in the routing API.", &facts);
        assert_eq!(new, ["fixed"]);
        assert!(stands("Fixed", &new));
        assert!(stands("fixed two capacity bottlenecks", &new));
        assert!(!stands("capacity bottlenecks", &new), "the facts say so");
        assert!(new_words("Found two capacity bottlenecks.", &facts).is_empty());
    }
}
