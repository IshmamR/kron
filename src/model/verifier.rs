// Checks generated lines against the facts they cite, with no model: a line
// may state only figures and years its facts state, counting what they
// count, and may not rest on a fact the user hid. A line that fails is left
// out of the résumé and listed as an unsupported claim.

use std::borrow::Cow;
use std::ops::Range;

use rust_stemmers::{Algorithm, Stemmer};
use text2num::{BasicAnnotate, LangInterpreter, Language, Token, find_numbers};

use super::{FactId, UnsupportedReason};

/// Four-digit numbers in this range are read as years.
const YEARS: std::ops::RangeInclusive<u16> = 1900..=2099;
const CURRENCIES: [char; 6] = ['$', '€', '£', '¥', '₹', '৳'];
/// Lone numbers under this stay words when number words are read: "one" and
/// "first" are more often "one of the first" than a count.
const LONE_NUMBER_WORDS_BELOW: f64 = 2.0;

/// A fact as the generator was given it.
pub struct GivenFact<'a> {
    pub id: FactId,
    pub text: &'a str,
    pub hidden: bool,
}

/// Why the line `text`, citing `cites` from the `given` facts, can't go on
/// the page, if it can't: it cites nothing or a fact it wasn't given, cites a
/// hidden fact, or states a figure or year none of its facts state. A cited
/// fact supports a line with words only if they share one that isn't a stop
/// word or part of a figure, so a line can't borrow a figure by citing more.
/// Figures
/// are compared by value, so "1,200,000", "1.2M" and "1.2 million" are the
/// same, and so are "40%", "40 percent", "four" and 4; a rounded figure is
/// not. A count must count what a fact stating it counts: "12 engineers"
/// fails on "12 services". Lines and facts are read as English.
pub fn verify(text: &str, cites: &[FactId], given: &[GivenFact]) -> Option<UnsupportedReason> {
    if cites.is_empty() {
        return Some(UnsupportedReason::NoSupportingFact);
    }
    let line = figures(text);
    let words = content(text, &line);
    let mut facts = Vec::new();
    for cite in cites {
        let Some(fact) = given.iter().find(|fact| fact.id == *cite) else {
            return Some(UnsupportedReason::NoSupportingFact);
        };
        if fact.hidden {
            return Some(UnsupportedReason::UsesHiddenFact(fact.id));
        }
        let figures = figures(fact.text);
        let stems = content(fact.text, &figures);
        // A line of only figures, like "2016 – 2018", has no words to share.
        if words.is_empty() || stems.iter().any(|stem| words.contains(stem)) {
            facts.push((figures, stems));
        }
    }
    if facts.is_empty() {
        return Some(UnsupportedReason::NoSupportingFact);
    }
    line.iter().find_map(|figure| {
        let stating: Vec<&Vec<String>> = facts
            .iter()
            .filter(|(figures, _)| figures.iter().any(|stated| stated.value == figure.value))
            .map(|(_, stems)| stems)
            .collect();
        if stating.is_empty() {
            return Some(match figure.year {
                Some(year) => UnsupportedReason::InventedYear(year),
                None => UnsupportedReason::InventedFigure(figure.written.to_owned()),
            });
        }
        let (end, counted) = counted(text, figure, &line);
        let counts_the_same = stating
            .iter()
            .any(|stems| counted.iter().all(|stem| stems.contains(stem)));
        (!counts_the_same)
            .then(|| UnsupportedReason::InventedFigure(text[figure.start..end].to_owned()))
    })
}

/// Where the line `text` has words that carry meaning and none of `facts`
/// has, compared by stem: "AWS" or "leader" in a line whose facts say
/// neither, for the user to check. It warns, it doesn't reject: a fair
/// rewording uses words of its own. Figures are left to `verify`.
pub fn new_words(text: &str, facts: &[&str]) -> Vec<Range<usize>> {
    let known: Vec<String> = facts.iter().flat_map(|fact| content(fact, &[])).collect();
    words(text, &figures(text))
        .into_iter()
        .filter(|(_, stem)| !known.contains(stem))
        .map(|(range, _)| range)
        .collect()
}

/// A number in a text.
#[derive(Debug)]
struct Figure<'a> {
    /// Where it starts in the text, in bytes.
    start: usize,
    /// As written, unit included: "$2M", "40 percent".
    written: &'a str,
    value: Value,
    year: Option<u16>,
}

/// A number reduced so equal ones compare equal however they're written:
/// significant digits and a power of ten, so 1.2 million is "12" and 5.
#[derive(Debug, PartialEq, Eq)]
struct Value {
    digits: String,
    exponent: i32,
    unit: Unit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Plain,
    Percent,
    /// "3x", "3 times".
    Times,
    Currency(char),
}

impl Figure<'_> {
    fn end(&self) -> usize {
        self.start + self.written.len()
    }
}

/// A word, or the run of other characters between two words, split as
/// text2num's own tokenizer splits text.
struct Piece<'a> {
    text: &'a str,
    lowercase: String,
    start: usize,
    end: usize,
    not_a_number: bool,
}

impl Token for &Piece<'_> {
    fn text(&self) -> Cow<'_, str> {
        self.text.into()
    }

    fn text_lowercase(&self) -> Cow<'_, str> {
        self.lowercase.as_str().into()
    }

    fn not_a_number_part(&self) -> bool {
        self.not_a_number
    }
}

impl BasicAnnotate for Piece<'_> {
    fn text_lowercase(&self) -> &str {
        &self.lowercase
    }

    fn set_nan(&mut self, value: bool) {
        self.not_a_number = self.not_a_number || value;
    }
}

/// The numbers in `text`, in order: numerals, and number words.
fn figures(text: &str) -> Vec<Figure<'_>> {
    let mut figures = numerals(text);
    let words = number_words(text, &figures);
    figures.extend(words);
    figures.sort_by_key(|figure| figure.start);
    figures
}

/// The numbers in `text` written in digits. A word that starts with a
/// letter, like "p99", "S3" or "x86", is a name, not a number.
fn numerals(text: &str) -> Vec<Figure<'_>> {
    let mut figures = Vec::new();
    let mut index = 0;
    while let Some(character) = text[index..].chars().next() {
        let after = index + character.len_utf8();
        let currency = CURRENCIES.contains(&character)
            && text[after..].starts_with(|next: char| next.is_ascii_digit());
        if character.is_ascii_digit() || currency {
            let (figure, end) = number(text, index);
            figures.push(figure);
            index = end;
        } else if character.is_alphanumeric() {
            index = word_end(text, index);
        } else {
            index = after;
        }
    }
    figures
}

/// The numbers in `text` written in words, read by text2num: "twenty-five".
/// Words a numeral took in, like the "million" of "1.2 million", are left to
/// it.
fn number_words<'a>(text: &'a str, numerals: &[Figure]) -> Vec<Figure<'a>> {
    let english = Language::english();
    let mut pieces = pieces(text, numerals);
    english.basic_annotate(&mut pieces);
    find_numbers(pieces.iter(), &english, LONE_NUMBER_WORDS_BELOW)
        .into_iter()
        .filter_map(|number| {
            let start = pieces.get(number.start)?.start;
            let end = pieces.get(number.end.checked_sub(1)?)?.end;
            let value = self::numerals(&number.text).into_iter().next()?.value;
            Some(measured(text, start, end, value.digits, value.exponent, Unit::Plain).0)
        })
        .collect()
}

/// `text` split into words and what's between them. The pieces inside a
/// numeral can't be part of a number word.
fn pieces<'a>(text: &'a str, numerals: &[Figure]) -> Vec<Piece<'a>> {
    let mut pieces = Vec::new();
    let mut start = 0;
    while let Some(first) = text[start..].chars().next() {
        let word = first.is_alphanumeric();
        let length = text[start..]
            .find(|character: char| {
                let in_word = character.is_alphanumeric() || character == '-' || character == '\'';
                if word {
                    !in_word
                } else {
                    character.is_alphanumeric()
                }
            })
            .unwrap_or(text.len() - start);
        let end = start + length;
        pieces.push(Piece {
            text: &text[start..end],
            lowercase: text[start..end].to_lowercase(),
            start,
            end,
            not_a_number: numerals
                .iter()
                .any(|numeral| numeral.start < end && start < numeral.end()),
        });
        start = end;
    }
    pieces
}

/// The number starting at `start`, a digit or a currency sign, and where it
/// ends.
fn number(text: &str, start: usize) -> (Figure<'_>, usize) {
    let mut unit = Unit::Plain;
    let mut index = start;
    if let Some(sign) = text[start..]
        .chars()
        .next()
        .filter(|sign| CURRENCIES.contains(sign))
    {
        unit = Unit::Currency(sign);
        index += sign.len_utf8();
    }
    let digits_start = index;
    let bytes = text.as_bytes();
    let mut digits = String::new();
    let mut fraction = 0;
    let mut points = 0;
    loop {
        match bytes.get(index) {
            Some(byte) if byte.is_ascii_digit() => {
                digits.push(char::from(*byte));
                if points > 0 {
                    fraction += 1;
                }
            }
            Some(b',') if points == 0 && thousands(&bytes[index + 1..]) => {}
            Some(b'.') if bytes.get(index + 1).is_some_and(u8::is_ascii_digit) => points += 1,
            _ => break,
        }
        index += 1;
    }
    // A version like "3.11.2" is compared as written.
    let (digits, exponent) = if points > 1 {
        (text[digits_start..index].to_owned(), 0)
    } else {
        significant(&digits, fraction)
    };
    measured(text, start, index, digits, exponent, unit)
}

/// The figure from `start` to `end`, taking in a unit or scale written right
/// after it: "10k", "40%", "3x", "1.2 million", "40 percent".
fn measured(
    text: &str,
    start: usize,
    end: usize,
    digits: String,
    mut exponent: i32,
    mut unit: Unit,
) -> (Figure<'_>, usize) {
    let mut end = end;
    let suffix_end = word_end(text, end);
    match &text[end..suffix_end] {
        "k" | "K" => exponent += 3,
        "M" => exponent += 6,
        "B" | "bn" => exponent += 9,
        "x" | "X" => unit = Unit::Times,
        // Others, like "ms" or "th", don't change the value.
        _ => {}
    }
    end = suffix_end;
    if let Some(sign) = text[end..]
        .chars()
        .next()
        .filter(|sign| matches!(sign, '%' | '×'))
    {
        unit = if sign == '%' {
            Unit::Percent
        } else {
            Unit::Times
        };
        end += sign.len_utf8();
    }
    if text[end..].starts_with(' ') {
        let word_end = word_end(text, end + 1);
        let word = text[end + 1..word_end].to_ascii_lowercase();
        let (unit_word, scale) = match word.as_str() {
            "percent" => (Some(Unit::Percent), 0),
            "times" => (Some(Unit::Times), 0),
            "thousand" => (None, 3),
            "million" => (None, 6),
            "billion" => (None, 9),
            _ => (None, -1),
        };
        if unit_word.is_some() || scale > 0 {
            unit = unit_word.unwrap_or(unit);
            exponent += scale;
            end = word_end;
        }
    }
    let written = &text[start..end];
    let year = written
        .parse::<u16>()
        .ok()
        .filter(|year| written.len() == 4 && YEARS.contains(year));
    // Zero has no significant digits, whatever its scale.
    let exponent = if digits.is_empty() { 0 } else { exponent };
    (
        Figure {
            start,
            written,
            value: Value {
                digits,
                exponent,
                unit,
            },
            year,
        },
        end,
    )
}

/// `digits` with `fraction` of them after the point, as significant digits
/// and a power of ten: "1200" is "12" and 2, "0.5" is "5" and -1.
fn significant(digits: &str, fraction: usize) -> (String, i32) {
    let leading = digits.trim_start_matches('0');
    let significant = leading.trim_end_matches('0');
    let exponent = (leading.len() - significant.len()) as i32 - fraction as i32;
    (significant.to_owned(), exponent)
}

/// Whether a comma before `rest` separates thousands: three digits follow,
/// and then no more.
fn thousands(rest: &[u8]) -> bool {
    rest.len() >= 3
        && rest[..3].iter().all(u8::is_ascii_digit)
        && !rest.get(3).is_some_and(u8::is_ascii_digit)
}

/// Where the run of letters and digits starting at `start` ends.
fn word_end(text: &str, start: usize) -> usize {
    text[start..]
        .find(|character: char| !character.is_alphanumeric())
        .map_or(text.len(), |length| start + length)
}

/// What the count `figure` counts: the stems of the words after it, up to a
/// stop word, a mark or another figure, and where they end. "12 backend
/// services from Python" counts "backend services". Splitting at stop words
/// and marks is how RAKE finds phrases (Rose et al., 2010). A percentage,
/// ratio, sum or year counts nothing.
fn counted(text: &str, figure: &Figure, figures: &[Figure]) -> (usize, Vec<String>) {
    let mut end = figure.end();
    let mut stems = Vec::new();
    if figure.value.unit != Unit::Plain || figure.year.is_some() {
        return (end, stems);
    }
    loop {
        let start = text[end..]
            .find(|character: char| !character.is_whitespace() && character != '-')
            .map_or(text.len(), |length| end + length);
        let word = &text[start..word_end(text, start)];
        if word.is_empty() || figures.iter().any(|other| other.start == start) || stop_word(word) {
            return (end, stems);
        }
        stems.push(stem(word));
        end = start + word.len();
    }
}

/// The stems of the words in `text` that carry meaning: not stop words, and
/// not part of its `figures`.
fn content(text: &str, figures: &[Figure]) -> Vec<String> {
    words(text, figures)
        .into_iter()
        .map(|(_, stem)| stem)
        .collect()
}

/// The words in `text` that carry meaning, where each is and its stem.
fn words(text: &str, figures: &[Figure]) -> Vec<(Range<usize>, String)> {
    let mut words = Vec::new();
    let mut start = 0;
    while let Some(length) = text[start..].find(char::is_alphanumeric) {
        start += length;
        let end = word_end(text, start);
        let word = &text[start..end];
        let in_figure = figures
            .iter()
            .any(|figure| figure.start <= start && start < figure.end());
        if !in_figure && !stop_word(word) {
            words.push((start..end, stem(word)));
        }
        start = end;
    }
    words
}

/// Whether `word` is on NLTK's English stop word list: "the", "of", "an".
fn stop_word(word: &str) -> bool {
    stop_words::lookup("en")
        .unwrap_or_default()
        .contains(&word.to_lowercase().as_str())
}

/// `word`, lowercase, reduced to its Snowball stem: "volunteers" is "volunt".
fn stem(word: &str) -> String {
    Stemmer::create(Algorithm::English)
        .stem(&word.to_lowercase())
        .into_owned()
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::database::Error;
    use crate::model::sample;

    fn values(text: &str) -> Vec<Value> {
        figures(text)
            .into_iter()
            .map(|figure| figure.value)
            .collect()
    }

    fn written(text: &str) -> Vec<&str> {
        figures(text)
            .into_iter()
            .map(|figure| figure.written)
            .collect()
    }

    #[test]
    fn a_number_written_differently_is_the_same_figure() {
        let million = values("1,200,000");
        assert_eq!(values("1.2M"), million);
        assert_eq!(values("1.2 million"), million);
        assert_eq!(values("1200 thousand"), million);
        assert_eq!(values("40%"), values("40 percent"));
        assert_eq!(values("3x"), values("3×"));
        assert_eq!(values("3x"), values("3 times"));
        assert_eq!(values("four"), values("4"));
        assert_eq!(values("twenty-five"), values("25"));
        assert_eq!(values("one hundred and twenty"), values("120"));
        assert_eq!(values("1.50"), values("1.5"));
        assert_eq!(values("0.5"), values("0.50"));
        assert_eq!(values("10k"), values("10,000"));
    }

    #[test]
    fn a_different_unit_or_rounding_is_a_different_figure() {
        assert_ne!(values("40%"), values("40"));
        assert_ne!(values("$2M"), values("€2M"));
        assert_ne!(values("$2M"), values("2M"));
        assert_ne!(values("3x"), values("3"));
        assert_ne!(values("37%"), values("40%"));
        assert_ne!(values("3.11"), values("3.1"));
        assert_ne!(values("3.11.2"), values("3.112"));
    }

    #[test]
    fn names_with_digits_are_not_figures() {
        assert!(values("p99 latency on S3 and EC2, x86, no_std, OAuth2").is_empty());
        assert!(values("one of the first engineers").is_empty());
    }

    #[test]
    fn figures_are_written_with_their_units() {
        assert_eq!(
            written(
                "Cut costs 30% and $2M, 40 percent faster, 1.2 million users, 3.11.2, 2,500 rows."
            ),
            ["30%", "$2M", "40 percent", "1.2 million", "3.11.2", "2,500"]
        );
        assert_eq!(written("+880 1711 555 210"), ["880", "1711", "555", "210"]);
        assert_eq!(written("2019 – 2021"), ["2019", "2021"]);
        assert_eq!(written("1, 2, 3"), ["1", "2", "3"]);
    }

    #[test]
    fn only_plain_four_digit_numbers_in_range_are_years() {
        let years = |text| {
            figures(text)
                .into_iter()
                .map(|figure| figure.year)
                .collect::<Vec<_>>()
        };
        assert_eq!(years("2016"), [Some(2016)]);
        assert_eq!(years("1711 2,016 2016% $2016 2016s 2150"), [None; 6]);
    }

    fn fact(text: &str) -> GivenFact<'_> {
        GivenFact {
            id: FactId::new(),
            text,
            hidden: false,
        }
    }

    #[test]
    fn a_line_needs_a_fact_it_was_given() {
        let given = [fact("Led the migration of 12 services.")];
        assert_eq!(
            verify("Led a migration.", &[], &given),
            Some(UnsupportedReason::NoSupportingFact)
        );
        assert_eq!(
            verify("Led a migration.", &[FactId::new()], &given),
            Some(UnsupportedReason::NoSupportingFact)
        );
    }

    #[test]
    fn a_line_citing_a_hidden_fact_is_unsupported() {
        let hidden = GivenFact {
            hidden: true,
            ..fact("Wrote property-based tests.")
        };
        let id = hidden.id;
        let shown = fact("Worked on billing.");
        assert_eq!(
            verify("Tested billing.", &[shown.id, id], &[shown, hidden]),
            Some(UnsupportedReason::UsesHiddenFact(id))
        );
    }

    #[test]
    fn a_line_may_reword_but_not_add_figures_or_years() {
        let migration =
            fact("Led the migration of 12 backend services from Python to Rust in 2022.");
        let latency = fact("Cut p99 latency from 480 ms to 95 ms, about 80%.");
        let cites = [migration.id, latency.id];
        let given = [migration, latency];
        let check = |line| verify(line, &cites, &given);
        assert_eq!(
            check("Moved twelve services to Rust (2022), cutting latency 80%."),
            None
        );
        assert_eq!(
            check("Moved twelve services to Rust, cutting latency 85%."),
            Some(UnsupportedReason::InventedFigure("85%".into()))
        );
        assert_eq!(
            check("Moved 12 services to Rust in 2021."),
            Some(UnsupportedReason::InventedYear(2021))
        );
        assert_eq!(
            check("Moved 12 services to Rust with a team of 4."),
            Some(UnsupportedReason::InventedFigure("4".into()))
        );
        assert_eq!(
            verify(
                "Moved 12 services to Rust, cutting latency.",
                &cites[1..],
                &given
            ),
            Some(UnsupportedReason::InventedFigure("12".into())),
            "a figure must come from a fact the line cites"
        );
    }

    #[test]
    fn a_count_must_count_what_its_fact_counts() {
        let migration = fact("Led the migration of 12 backend services from Python to Rust.");
        let event = fact("Ran a volunteer team of 15 students over the three days.");
        let cites = [migration.id, event.id];
        let given = [migration, event];
        let check = |line| verify(line, &cites, &given);
        assert_eq!(check("Migrated 12 backend services to Rust."), None);
        assert_eq!(
            check("Ran a team of 15 student volunteers over three days."),
            None,
            "stems match: volunteers, volunteer"
        );
        assert_eq!(
            check("Led a team of 12 backend engineers."),
            Some(UnsupportedReason::InventedFigure(
                "12 backend engineers".into()
            ))
        );
        assert_eq!(
            check("Ran 15 students over three weeks."),
            Some(UnsupportedReason::InventedFigure("three weeks".into()))
        );
        assert_eq!(
            check("Ran 15 backend services."),
            Some(UnsupportedReason::InventedFigure(
                "15 backend services".into()
            )),
            "what a count counts must come from a fact stating it"
        );
    }

    #[test]
    fn a_figure_cant_come_from_a_fact_the_line_shares_no_word_with() {
        let spend = fact("Reduced cloud spend by moving batch jobs to spot instances.");
        let coverage = fact("Grew test coverage of the billing service from 40% to 85%.");
        let cites = [spend.id, coverage.id];
        let given = [spend, coverage];
        assert_eq!(
            verify(
                "Reduced cloud spend by 85% with spot instances.",
                &cites,
                &given
            ),
            Some(UnsupportedReason::InventedFigure("85%".into()))
        );
        assert_eq!(
            verify("Spoke at RustConf on tracing.", &cites, &given),
            Some(UnsupportedReason::NoSupportingFact)
        );
        assert_eq!(
            verify(
                "Reduced cloud spend with spot instances and grew billing test coverage to 85%.",
                &cites,
                &given
            ),
            None,
            "a line may combine the facts it cites"
        );
    }

    /// Each sample generation's lines as generated, with the facts they cite,
    /// and the facts it was generated from.
    #[allow(clippy::type_complexity)]
    fn generations(
        connection: &Connection,
    ) -> Result<Vec<(Vec<(String, Vec<FactId>)>, Vec<(FactId, String, bool)>)>, Error> {
        let ids: Vec<uuid::Uuid> = connection
            .prepare("SELECT id FROM generations ORDER BY id")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let mut generations = Vec::new();
        for id in ids {
            let lines: Vec<(uuid::Uuid, String)> = connection
                .prepare(
                    "SELECT lines.id, lines.generated FROM lines
                     JOIN blocks ON blocks.id = lines.block_id
                     JOIN sections ON sections.id = blocks.section_id
                     WHERE sections.generation_id = ?1",
                )?
                .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            let mut cited = Vec::new();
            for (line, text) in lines {
                let cites = connection
                    .prepare("SELECT fact_id FROM line_citations WHERE line_id = ?1")?
                    .query_map([line], |row| Ok(FactId(row.get(0)?)))?
                    .collect::<Result<_, _>>()?;
                cited.push((text, cites));
            }
            let given = connection
                .prepare(
                    "SELECT fact_id, text, hidden FROM fact_versions WHERE generation_id = ?1",
                )?
                .query_map([id], |row| {
                    Ok((FactId(row.get(0)?), row.get(1)?, row.get(2)?))
                })?
                .collect::<Result<_, _>>()?;
            generations.push((cited, given));
        }
        Ok(generations)
    }

    #[test]
    fn every_sample_line_passes() -> Result<(), Error> {
        let writer = sample::written()?;
        let generations = generations(writer.kron())?;
        assert!(!generations.is_empty());
        for (lines, given) in &generations {
            let given: Vec<GivenFact> = given
                .iter()
                .map(|(id, text, hidden)| GivenFact {
                    id: *id,
                    text,
                    hidden: *hidden,
                })
                .collect();
            for (text, cites) in lines {
                assert_eq!(verify(text, cites, &given), None, "{text}");
            }
        }
        Ok(())
    }

    /// Each sample claim, citing what a generator might have, is caught for
    /// the reason the sample gives.
    #[test]
    fn the_sample_claims_are_caught() -> Result<(), Error> {
        let writer = sample::written()?;
        let connection = writer.kron();
        let facts: Vec<(FactId, String, bool)> = connection
            .prepare("SELECT id, text, hidden FROM facts")?
            .query_map([], |row| {
                Ok((FactId(row.get(0)?), row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<_, _>>()?;
        let given: Vec<GivenFact> = facts
            .iter()
            .map(|(id, text, hidden)| GivenFact {
                id: *id,
                text,
                hidden: *hidden,
            })
            .collect();
        let cited = [
            ("Improved the team", "Ran the on-call rotation"),
            (
                "Writing production Rust",
                "Backend engineer with eight years",
            ),
            (
                "Introduced property-based tests",
                "Introduced property-based tests",
            ),
            ("Processed $2M", "Designed the event pipeline"),
            (
                "Designed routing algorithms",
                "Cut p99 latency of the routing",
            ),
        ];
        for (claim, fact) in cited {
            let (text, reason, figure, year, hidden_fact): (
                String,
                String,
                Option<String>,
                Option<u16>,
                Option<uuid::Uuid>,
            ) = connection.query_row(
                "SELECT text, reason, figure, year, hidden_fact_id FROM unsupported_claims
                     WHERE text LIKE ?1 || '%'",
                [claim],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;
            let expected = match (reason.as_str(), figure, year, hidden_fact) {
                ("invented_figure", Some(figure), _, _) => {
                    UnsupportedReason::InventedFigure(figure)
                }
                ("invented_year", _, Some(year), _) => UnsupportedReason::InventedYear(year),
                ("uses_hidden_fact", _, _, Some(fact)) => {
                    UnsupportedReason::UsesHiddenFact(FactId(fact))
                }
                _ => panic!("unexpected claim {reason}"),
            };
            let cites = [sample::fact(connection, fact)?];
            assert_eq!(verify(&text, &cites, &given), Some(expected), "{text}");
        }
        Ok(())
    }

    const CASES: &str = include_str!("verifier/cases.txt");
    /// Lines a model wrote, labelled by hand (`generation::tests::write_test_set`).
    const GENERATED: &str = include_str!("verifier/generated.txt");
    /// Labels the verifier handles: getting any of those cases wrong fails
    /// the test. The rest are only reported.
    const HANDLED: [&str; 4] = [
        "paraphrase",
        "invented_figure",
        "invented_year",
        "unit_swap",
    ];

    struct Case {
        label: &'static str,
        facts: Vec<&'static str>,
        line: &'static str,
    }

    fn cases(set: &'static str) -> Vec<Case> {
        set.split("\n\n")
            .filter_map(|block| {
                let mut lines = block
                    .lines()
                    .filter(|line| !line.starts_with('#') && !line.is_empty());
                let label = lines.next()?;
                let mut case = Case {
                    label,
                    facts: Vec::new(),
                    line: "",
                };
                for line in lines {
                    match line.split_once(": ") {
                        Some(("fact", fact)) => case.facts.push(fact),
                        Some(("line", text)) => case.line = text,
                        _ => panic!("can't read case line {line:?}"),
                    }
                }
                assert!(
                    !case.facts.is_empty() && !case.line.is_empty(),
                    "incomplete case {label}"
                );
                Some(case)
            })
            .collect()
    }

    #[test]
    fn words_a_line_adds_to_its_facts_are_found() {
        let shown = |text: &str, facts: &[&str]| -> Vec<String> {
            new_words(text, facts)
                .into_iter()
                .map(|range| text[range].to_owned())
                .collect()
        };
        let fact = "Mentored four junior engineers through their first production launches.";
        assert_eq!(
            shown(fact, &[fact]),
            Vec::<String>::new(),
            "a copy adds nothing"
        );
        assert_eq!(
            shown("Mentoring 4 junior engineers on AWS.", &[fact]),
            ["AWS"],
            "a word by its stem, a figure left to verify, a stop word skipped"
        );
        assert_eq!(
            shown(
                "Product leader with eight years in product.",
                &[
                    "Eight years in product.",
                    "Right now I lead pricing and supply."
                ]
            ),
            ["leader"],
            "across every fact cited, and \"leader\" isn't \"lead\""
        );
    }

    /// Whether a line with `label` is true, and should pass.
    fn true_label(label: &str) -> bool {
        matches!(label, "paraphrase" | "number_rewording" | "verbatim")
    }

    /// Runs every case of `set` citing all its facts, and prints, per label,
    /// how many the verifier gets wrong: false passes for fabrications,
    /// false flags for true lines. Returns the lines it got wrong whose
    /// labels are among `must`.
    fn score(name: &str, set: &'static str, must: &[&str]) -> Vec<&'static str> {
        let mut tally: Vec<(&str, usize, usize)> = Vec::new();
        let mut regressions = Vec::new();
        for case in cases(set) {
            let given: Vec<GivenFact> = case.facts.iter().map(|text| fact(text)).collect();
            let cites: Vec<FactId> = given.iter().map(|fact| fact.id).collect();
            let flagged = verify(case.line, &cites, &given).is_some();
            let wrong = flagged == true_label(case.label);
            match tally.iter_mut().find(|(label, ..)| *label == case.label) {
                Some((.., total, wrongs)) => {
                    *total += 1;
                    *wrongs += usize::from(wrong);
                }
                None => tally.push((case.label, 1, usize::from(wrong))),
            }
            if wrong && must.contains(&case.label) {
                regressions.push(case.line);
            }
        }
        eprintln!("{name}:");
        for (label, total, wrong) in &tally {
            let kind = if true_label(label) {
                "false flags"
            } else {
                "false passes"
            };
            eprintln!("  {label:<17} {wrong:>2} of {total:>2} {kind}");
        }
        regressions
    }

    /// Run with --nocapture to print the tables.
    #[test]
    fn the_labelled_cases() {
        let regressions = score("cases.txt", CASES, &HANDLED);
        assert!(
            regressions.is_empty(),
            "handled cases got wrong: {regressions:#?}"
        );
    }

    /// Real output can't be held to every label `the_labelled_cases` is: its
    /// lines weren't written to test one thing each. A fact copied word for
    /// word must always pass, and every line must be labelled.
    #[test]
    fn lines_a_model_wrote() {
        assert!(
            cases(GENERATED).iter().all(|case| case.label != "?"),
            "label every line in generated.txt"
        );
        let regressions = score("generated.txt", GENERATED, &["verbatim"]);
        assert!(
            regressions.is_empty(),
            "copies of a fact flagged: {regressions:#?}"
        );
    }
}
