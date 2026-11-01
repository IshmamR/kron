// Checks a model's fix of a quote against what the document has. A fix may
// change how the text is set (spacing, dashes, quotes, a word split in two
// or two run together) and correct a slip in a long word, never a number, a
// symbol like % or +, or which words are there: the fact must still say what
// the document says. Also finds a quote the model fixed without saying so,
// by approximate substring matching (Sellers, 1980): the stretch of the text
// that the fewest edits turn into the quote.

use std::ops::Range;

/// Punctuation a fix may add, drop or change: how text is set, not what it
/// says. Others, like % or +, must stay as they are.
const LAYOUT: [char; 17] = [
    '-', '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}', '\u{2212}', '\'',
    '\u{2018}', '\u{2019}', '"', '\u{201c}', '\u{201d}', '.', ',', '\u{00b7}',
];
/// Letters a word needs for a slip in it to be fixed: in a shorter word one
/// letter makes another word, as "led" and "lead".
const TYPO_LENGTH: usize = 5;
/// A word this long may have two slips fixed, a shorter one only one.
const LONG_WORD: usize = 10;
/// Most words a word split apart, or run together, is in.
const SPLIT_PIECES: usize = 3;
/// Approximate matching allows one edit per this many characters of the
/// quote, plus `SPARE_EDITS`: a three-line bullet has two line breaks
/// where the quote has spaces, which count as matching, and a split word
/// rejoined costs two.
const CHARACTERS_PER_EDIT: usize = 8;
const SPARE_EDITS: usize = 2;
/// Quotes shorter than this aren't matched approximately: a few letters
/// are nearly anywhere.
const SHORTEST_APPROXIMATE: usize = 12;

/// Whether `fixed` only fixes how `written` is set or a slip in a long
/// word, and says the same.
pub fn acceptable(written: &str, fixed: &str) -> bool {
    symbols(written).eq(symbols(fixed)) && words_agree(&words(written), &words(fixed))
}

/// The stretch of `text` nearest to `quote`, preferring one starting at or
/// after `from`, if few enough edits turn it into the quote. Byte offsets.
pub fn nearest(text: &str, quote: &str, from: usize) -> Option<Range<usize>> {
    let pattern: Vec<char> = quote.chars().collect();
    if pattern.len() < SHORTEST_APPROXIMATE {
        return None;
    }
    let characters: Vec<(usize, char)> = text.char_indices().collect();
    // Each column's cost so far and where its alignment starts, a row at a
    // time: any start in the text is free.
    let mut previous: Vec<(usize, usize)> = (0..=characters.len()).map(|j| (0, j)).collect();
    let mut current = previous.clone();
    for (i, &wanted) in pattern.iter().enumerate() {
        current[0] = (i + 1, 0);
        for (j, &(_, character)) in characters.iter().enumerate() {
            let substitution = usize::from(!same(wanted, character));
            let diagonal = (previous[j].0 + substitution, previous[j].1);
            let skip_quote = (previous[j + 1].0 + 1, previous[j + 1].1);
            let skip_text = (current[j].0 + 1, current[j].1);
            current[j + 1] = [skip_quote, skip_text]
                .into_iter()
                .fold(
                    diagonal,
                    |best, option| if option.0 < best.0 { option } else { best },
                );
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let fewest = previous.iter().map(|(cost, _)| *cost).min()?;
    if fewest > pattern.len() / CHARACTERS_PER_EDIT + SPARE_EDITS {
        return None;
    }
    let position = |index: usize| {
        characters
            .get(index)
            .map_or(text.len(), |(position, _)| *position)
    };
    let ends = previous
        .iter()
        .enumerate()
        .filter(|(end, (cost, start))| *cost == fewest && end > start);
    let (end, (_, start)) = ends
        .clone()
        .find(|(_, (_, start))| position(*start) >= from)
        .or_else(|| ends.clone().next())?;
    let range = position(*start)..position(end);
    // Insertions at either end may take in whitespace around it.
    let found = &text[range.clone()];
    let start = range.start + (found.len() - found.trim_start().len());
    let end = range.end - (found.len() - found.trim_end().len());
    (start < end).then_some(start..end)
}

/// Whether the quote's `wanted` matches the text's `character`: any two
/// whitespace characters do, as do dashes and quote marks of any kind.
fn same(wanted: char, character: char) -> bool {
    let plain = |character: char| match character {
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{2018}' | '\u{2019}' => '\'',
        '\u{201c}' | '\u{201d}' => '"',
        character if character.is_whitespace() => ' ',
        character => character,
    };
    plain(wanted) == plain(character)
}

fn symbols(text: &str) -> impl Iterator<Item = char> {
    text.chars().filter(|character| {
        !character.is_alphanumeric() && !character.is_whitespace() && !LAYOUT.contains(character)
    })
}

fn words(text: &str) -> Vec<&str> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect()
}

/// Whether `fixed` has `written`'s words: the same, a word split apart or
/// run together put right, or a slip in a long word fixed.
fn words_agree(written: &[&str], fixed: &[&str]) -> bool {
    let (mut i, mut j) = (0, 0);
    while i < written.len() && j < fixed.len() {
        if written[i] == fixed[j] {
            i += 1;
            j += 1;
        } else if let Some(pieces) = joined(&written[i..], fixed[j]) {
            i += pieces;
            j += 1;
        } else if let Some(pieces) = joined(&fixed[j..], written[i]) {
            i += 1;
            j += pieces;
        } else if slip(written[i], fixed[j]) {
            i += 1;
            j += 1;
        } else {
            return false;
        }
    }
    i == written.len() && j == fixed.len()
}

/// How many of `pieces`, all letters, make `word` together.
fn joined(pieces: &[&str], word: &str) -> Option<usize> {
    let mut together = String::new();
    for (count, piece) in pieces.iter().take(SPLIT_PIECES).enumerate() {
        if !piece.chars().all(char::is_alphabetic) {
            return None;
        }
        together.push_str(piece);
        if count > 0 && together == word {
            return Some(count + 1);
        }
        if together.len() >= word.len() {
            return None;
        }
    }
    None
}

/// Whether `fixed` corrects a slip in `written`: long words of letters a
/// letter or two apart.
fn slip(written: &str, fixed: &str) -> bool {
    let letters = |word: &str| word.chars().all(char::is_alphabetic);
    let length = written.chars().count().min(fixed.chars().count());
    let allowed = if length >= LONG_WORD { 2 } else { 1 };
    letters(written)
        && letters(fixed)
        && length >= TYPO_LENGTH
        && edit_distance(written, fixed) <= allowed
}

/// Edits between two words, in characters: Levenshtein distance with two
/// letters swapped counting as one edit, as a typo makes them (optimal
/// string alignment).
fn edit_distance(first: &str, second: &str) -> usize {
    let first: Vec<char> = first.chars().collect();
    let second: Vec<char> = second.chars().collect();
    let mut rows = vec![(0..=second.len()).collect::<Vec<usize>>()];
    for i in 1..=first.len() {
        let mut row = vec![i; second.len() + 1];
        for j in 1..=second.len() {
            let above = &rows[i - 1];
            let substitution = above[j - 1] + usize::from(first[i - 1] != second[j - 1]);
            row[j] = substitution.min(above[j] + 1).min(row[j - 1] + 1);
            if i > 1 && j > 1 && first[i - 1] == second[j - 2] && first[i - 2] == second[j - 1] {
                row[j] = row[j].min(rows[i - 2][j - 2] + 1);
            }
        }
        rows.push(row);
    }
    rows[first.len()][second.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fix_may_change_how_text_is_set_but_not_what_it_says() {
        for (written, fixed) in [
            ("Jun 2024 Present", "Jun 2024 – Present"),
            ("Built a real- time tracker", "Built a real-time tracker"),
            ("Work ing on the pipe-\nline", "Working on the pipeline"),
            (
                "Led the managment of releases",
                "Led the management of releases",
            ),
            ("an internal dashbaord", "an internal dashboard"),
            ("the team’s runbooks", "the team's runbooks"),
        ] {
            assert!(acceptable(written, fixed), "{written} → {fixed}");
        }
        for (written, fixed) in [
            ("from 480 ms to 95 ms", "from 480 ms to 90 ms"),
            ("cut costs 40%", "cut costs 40"),
            ("10+ years", "10 years"),
            ("no downtime", "downtime"),
            ("Led the team", "Lead the team"),
            ("raised 1,200 dollars", "raised 12 00 dollars"),
            ("Led 2 3-person teams", "Led 23-person teams"),
            ("Built it in Go", "Built it in Rust"),
            ("Built 3 apps", "Built 3 apps and 2 games"),
        ] {
            assert!(!acceptable(written, fixed), "{written} → {fixed}");
        }
    }

    #[test]
    fn a_quote_fixed_without_saying_is_found_where_it_was() {
        let written = "Designed the event pipe-\nline for ship ment updates.";
        let text = format!("Intro.\n{written}\nThen again:\n{written}\n");
        let quote = "Designed the event pipeline for shipment updates.";
        let found = |from: usize| nearest(&text, quote, from);
        let first = found(0);
        assert_eq!(first.clone().map(|range| &text[range]), Some(written));
        let second = found(first.map_or(0, |range| range.end));
        assert_eq!(second.clone().map(|range| &text[range]), Some(written));
        assert!(
            second.is_some_and(|range| range.start > text.len() / 2),
            "the next one along"
        );
        assert_eq!(
            nearest(&text, "Ran payroll for 300 people.", 0),
            None,
            "nothing like it"
        );
        assert_eq!(nearest(&text, "Intro", 0), None, "too short to guess");
    }
}
