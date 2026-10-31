// Guesses the user's name from a document as read, before any edits, with
// no model: CVs put it in the largest type on the first page; text without
// sizes puts it just above the contact details; and the email's local part
// often spells it. The user confirms it, so a guess may be wrong, but one
// that's plainly not a name (a heading, an address) is left out.

use super::Reading;

/// Words of a heading, not a name.
const HEADINGS: &[&str] = &[
    "about",
    "contact",
    "curriculum",
    "cv",
    "education",
    "experience",
    "profile",
    "projects",
    "resume",
    "résumé",
    "skills",
    "summary",
    "top",
];
/// A name has at most this many words. Ours: long Spanish and Portuguese
/// names run to five or six.
const MAX_WORDS: usize = 6;
/// Without sizes, the name is looked for among this many first lines. Ours.
const TOP_LINES: usize = 10;
/// Phone numbers have at least this many digits (ITU-T E.164 numbers have
/// up to 15; national ones rarely fewer than 7).
const PHONE_DIGITS: usize = 7;

/// The user's name as the document seems to give it.
pub fn guess(reading: &Reading) -> Option<String> {
    let first_page: Vec<&str> = reading
        .lines
        .iter()
        .take_while(|line| line.page == 0)
        .map(|line| &reading.text[line.span.clone()])
        .collect();
    let sizes: Vec<f32> = reading
        .lines
        .iter()
        .take(first_page.len())
        .map(|line| line.size)
        .collect();
    let email = first_page
        .iter()
        .flat_map(|line| line.split_whitespace())
        .find_map(email_local_part);

    let largest = (0..first_page.len())
        .filter(|&index| looks_like_name(first_page[index]))
        .max_by(|&a, &b| sizes[a].total_cmp(&sizes[b]).then(b.cmp(&a)))
        .filter(|&index| sizes[index] > median(&sizes));
    let above_contacts = first_page
        .iter()
        .take(TOP_LINES)
        .position(|line| is_contact(line))
        .and_then(|contact| {
            first_page[..contact]
                .iter()
                .rposition(|line| looks_like_name(line))
        });
    let spelled_by_email = email.as_deref().and_then(|email| {
        first_page
            .iter()
            .take(TOP_LINES)
            .position(|line| looks_like_name(line) && spells(email, line))
    });
    // The line above the contacts can be a job title; the email decides.
    let above_and_spelled = above_contacts.filter(|&index| {
        email
            .as_deref()
            .is_none_or(|email| spells(email, first_page[index]))
    });
    largest
        .or(above_and_spelled)
        .or(spelled_by_email)
        .or(above_contacts)
        .map(|index| tidy(first_page[index]))
}

/// A short line of capitalised words, with no digits or contact details.
fn looks_like_name(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() || words.len() > MAX_WORDS || is_contact(line) {
        return false;
    }
    if words
        .iter()
        .any(|word| HEADINGS.contains(&word.to_lowercase().trim_end_matches(':')))
    {
        return false;
    }
    let letters = words.iter().all(|word| {
        word.chars().all(|character| {
            character.is_alphabetic() || matches!(character, '.' | '-' | '\'' | '’')
        })
    });
    // Words between can be lowercase, as "van der" and "ur" are: too many
    // to list.
    let first = words.first().is_some_and(|word| starts_capitalised(word));
    let last = words
        .last()
        .is_some_and(|word| word.split(['-', '\'', '’']).any(starts_capitalised));
    letters && first && last
}

fn starts_capitalised(word: &str) -> bool {
    word.chars().next().is_some_and(char::is_uppercase)
}

/// An email address, phone number or web address.
fn is_contact(line: &str) -> bool {
    let digits = line.chars().filter(char::is_ascii_digit).count();
    let lowercase = line.to_lowercase();
    line.split_whitespace()
        .any(|word| email_local_part(word).is_some())
        || digits >= PHONE_DIGITS
        || ["http", "www.", ".com", ".dev", ".io", "linkedin", "github"]
            .iter()
            .any(|marker| lowercase.contains(marker))
}

/// The letters before the @ of an email address, lowercased.
fn email_local_part(word: &str) -> Option<String> {
    let (local, domain) = word.split_once('@')?;
    if local.is_empty() || !domain.contains('.') {
        return None;
    }
    Some(
        local
            .chars()
            .filter(|character| character.is_alphabetic())
            .flat_map(char::to_lowercase)
            .collect(),
    )
}

/// Whether the email's local part spells a word of the name, as "ishmam785"
/// spells "Ishmam" and "arahman" spells "Rahman".
fn spells(email: &str, name: &str) -> bool {
    name.split_whitespace().any(|word| {
        let word = word.to_lowercase();
        word.chars().count() >= 3 && email.contains(&word)
    })
}

/// The name as written, with words set in capitals for style written as
/// names are.
fn tidy(line: &str) -> String {
    line.split_whitespace()
        .map(|word| {
            let capitals = word
                .chars()
                .filter(|character| character.is_alphabetic())
                .count()
                > 1
                && !word.chars().any(char::is_lowercase);
            if capitals {
                let mut characters = word.chars();
                characters
                    .next()
                    .into_iter()
                    .chain(characters.flat_map(char::to_lowercase))
                    .collect()
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn median(sizes: &[f32]) -> f32 {
    let mut sorted = sizes.to_vec();
    sorted.sort_by(f32::total_cmp);
    sorted.get(sorted.len() / 2).copied().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::{Format, read};

    fn guessed(format: Format, bytes: &[u8]) -> Option<String> {
        guess(&read(format, bytes.to_vec()).expect("a test document that reads"))
    }

    #[test]
    fn the_largest_type_on_the_first_page_is_the_name() {
        for pdf in [
            &include_bytes!("pdf/latex.pdf")[..],
            include_bytes!("pdf/writer.pdf"),
            // A sidebar read first, the name after it.
            include_bytes!("pdf/sidebar.pdf"),
        ] {
            assert_eq!(guessed(Format::Pdf, pdf).as_deref(), Some("Alex Rahman"));
        }
        assert_eq!(
            guessed(Format::Markdown, include_bytes!("markdown/cv.md")).as_deref(),
            Some("Alex Rahman")
        );
    }

    #[test]
    fn names_with_lowercase_or_hyphenated_parts_are_names() {
        // Larger than the body, a heading would win if the name weren't one.
        for name in [
            "Sami ul Haque",
            "Sami-ul Haque",
            "Daan van der Velde",
            "Omar al-Farsi",
            "Siobhán O'Neill",
        ] {
            let markdown = format!("# {name}\n\n## Tech Skills\n\nRust, Go\n\nKafka\n");
            assert_eq!(
                guessed(Format::Markdown, markdown.as_bytes()).as_deref(),
                Some(name)
            );
        }
        let markdown = "## Tech Skills\n\nRust, Go\n\nKafka, PostgreSQL\n";
        assert_eq!(
            guessed(Format::Markdown, markdown.as_bytes()),
            None,
            "a heading is never the name, whatever its first word"
        );
    }

    #[test]
    fn without_sizes_the_name_is_above_the_contact_details() {
        let text = "Curriculum Vitae\n\nZoë van der Berg\nzoe@example.com · +31 6 1234 5678\n";
        assert_eq!(
            guessed(Format::Text, text.as_bytes()).as_deref(),
            Some("Zoë van der Berg")
        );
        let text = "ALEX RAHMAN\nalex@example.com\n";
        assert_eq!(
            guessed(Format::Text, text.as_bytes()).as_deref(),
            Some("Alex Rahman"),
            "capitals set for style are written as names are"
        );
    }

    #[test]
    fn without_sizes_or_a_line_above_the_contacts_the_email_spells_it() {
        let text = "Ishmam Rahman\nSoftware Engineer\n\nReach me at ishmam785@example.com\n";
        assert_eq!(
            guessed(Format::Text, text.as_bytes()).as_deref(),
            Some("Ishmam Rahman"),
            "the line above the email is a title"
        );
    }

    #[test]
    fn headings_and_sentences_are_not_names() {
        let text = "Experience\nLed the migration of 12 services.\nSkills: Rust, Go\n";
        assert_eq!(guessed(Format::Text, text.as_bytes()), None);
    }
}
