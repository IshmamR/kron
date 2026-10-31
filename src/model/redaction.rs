// Puts placeholders in place of the user's name and of contact details
// before text is sent to a model. Every way the name is written becomes the
// same placeholder: the full name, a first or last name alone, and the
// initials of author lists ("A. Rahman", "Rahman, A.", "Rahman AR"), so a
// publication list can't give it away. What's found is shown to the user,
// who can keep any of it or hide more, before the text is built. The
// model's quotes come back in the redacted text and are mapped to the
// original with `original`.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

/// Top-level domains read as a web address without "http" or "www". Ours:
/// common ones that aren't also file extensions, as ".js", ".py" and ".md"
/// are.
const DOMAINS: &str = "com|net|org|io|dev|me|co|ai|app|edu|gov|info|xyz|tech|page|site|blog|\
                       us|uk|ca|de|in|bd|au|eu|nl|fr";
/// Phone numbers have 7 to 15 digits: ITU-T E.164 numbers have up to 15;
/// national ones rarely fewer than 7.
const PHONE_DIGITS: Range<usize> = 7..16;
/// Four-digit numbers in this range are years, so "2019 - 2023" and
/// "2023-01-15" aren't phone numbers.
const YEARS: Range<u16> = 1900..2100;
/// A part of the name this long or longer in a web address makes it the
/// user's. Ours: "al" or "li" is in many addresses.
const MIN_PART_IN_ADDRESS: usize = 3;
/// Initials glued together, as "IU" in "Rahman IU", run to this many letters.
const MAX_INITIALS: usize = 3;

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:mailto:)?[a-z0-9._%+-]+@[a-z0-9-]+(?:\.[a-z0-9-]+)+")
        .expect("a valid pattern")
});
static LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)\b(?:https?://|www\.)[^\s<>()\[\]{{}}"']+|\b[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:{DOMAINS})/[^\s<>()\[\]{{}}"']*"#
    ))
    .expect("a valid pattern")
});
/// Case-sensitive, so "ASP.NET" isn't an address.
static DOMAIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"\b[a-zA-Z0-9-]+(?:\.[a-zA-Z0-9-]+)*\.(?:{DOMAINS})\b"
    ))
    .expect("a valid pattern")
});
static PHONE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\+|\(|\b)\d[\d ().-]*\d\b").expect("a valid pattern"));
/// A run of text with no space, as a username is written.
static HANDLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[^\s()<>\[\]{},;:"'’]+"#).expect("a valid pattern"));
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\p{L}+(?:['’]\p{L}+)*").expect("a valid pattern"));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identifier {
    Name,
    Email,
    Phone,
    Link,
    /// A username, as "alexr99" or "AlexR".
    Handle,
    /// A word or phrase the user chose to hide.
    Hidden,
}

impl Identifier {
    pub fn placeholder(self) -> &'static str {
        match self {
            Self::Name => "[NAME]",
            Self::Email => "[EMAIL]",
            Self::Phone => "[PHONE]",
            Self::Link => "[LINK]",
            Self::Handle => "[HANDLE]",
            Self::Hidden => "[HIDDEN]",
        }
    }
}

/// Text with its identifiers replaced by placeholders.
#[derive(Clone, Debug)]
pub struct Redaction {
    pub text: String,
    /// In order, none overlapping.
    pub replacements: Vec<Replacement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    pub identifier: Identifier,
    /// The identifier's bytes in the original text.
    pub original: Range<usize>,
    /// The placeholder's bytes in the redacted text.
    pub redacted: Range<usize>,
}

/// An identifier in the original text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub identifier: Identifier,
    pub range: Range<usize>,
}

/// What in `text` to hide, in order: the phrases in `hidden`, the names in
/// `names` as the user's Name facts give them, usernames made from them,
/// and every email address, phone number and web address. A web address
/// with no path, like a company's, is left unless it holds a name.
pub fn find(text: &str, names: &[&str], hidden: &[&str]) -> Vec<Found> {
    let name = Name::new(names);
    let mut found: Vec<Found> = Vec::new();
    for phrase in hidden {
        for range in phrase_ranges(text, phrase) {
            add(&mut found, Identifier::Hidden, range);
        }
    }
    for found_email in EMAIL.find_iter(text) {
        add(&mut found, Identifier::Email, found_email.range());
    }
    for found_link in LINK.find_iter(text) {
        let link = found_link
            .as_str()
            .trim_end_matches(['.', ',', ';', ':', '!', '?']);
        add(
            &mut found,
            Identifier::Link,
            found_link.start()..found_link.start() + link.len(),
        );
    }
    for found_domain in DOMAIN.find_iter(text) {
        if name.within(found_domain.as_str()) {
            add(&mut found, Identifier::Link, found_domain.range());
        }
    }
    for found_phone in PHONE.find_iter(text) {
        if is_phone(found_phone.as_str()) {
            add(&mut found, Identifier::Phone, found_phone.range());
        }
    }
    for found_handle in HANDLE.find_iter(text) {
        let handle = found_handle.as_str().trim_end_matches(['.', '!', '?']);
        if name.handle(handle) {
            add(
                &mut found,
                Identifier::Handle,
                found_handle.start()..found_handle.start() + handle.len(),
            );
        }
    }
    // Words in a contact are left out of names, so none overlap.
    let names = name.find(text, &found);
    found.extend(names.into_iter().map(|range| Found {
        identifier: Identifier::Name,
        range,
    }));
    found.sort_by_key(|found| found.range.start);
    found
}

impl Redaction {
    /// `text` with each of `hide`, in order and not overlapping, replaced by
    /// its placeholder.
    pub fn new(text: &str, hide: &[Found]) -> Self {
        let mut redacted = String::with_capacity(text.len());
        let mut replacements = Vec::with_capacity(hide.len());
        let mut copied = 0;
        for found in hide {
            redacted.push_str(&text[copied..found.range.start]);
            let start = redacted.len();
            redacted.push_str(found.identifier.placeholder());
            replacements.push(Replacement {
                identifier: found.identifier,
                redacted: start..redacted.len(),
                original: found.range.clone(),
            });
            copied = found.range.end;
        }
        redacted.push_str(&text[copied..]);
        Self {
            text: redacted,
            replacements,
        }
    }

    /// The bytes of the original text that `redacted`, bytes of the redacted
    /// text, came from. Part of a placeholder stands for all of what it
    /// replaced.
    pub fn original(&self, redacted: Range<usize>) -> Range<usize> {
        self.position(redacted.start, false)..self.position(redacted.end, true)
    }

    fn position(&self, redacted: usize, end: bool) -> usize {
        let (mut original_base, mut redacted_base) = (0, 0);
        for replacement in &self.replacements {
            let placeholder = &replacement.redacted;
            if redacted < placeholder.start || (end && redacted == placeholder.start) {
                break;
            }
            if redacted < placeholder.end || (end && redacted == placeholder.end) {
                return if end {
                    replacement.original.end
                } else {
                    replacement.original.start
                };
            }
            (original_base, redacted_base) = (replacement.original.end, placeholder.end);
        }
        original_base + (redacted - redacted_base)
    }
}

/// The words of the user's names.
struct Name {
    /// Capitalised words, lowercased: "alex", "rahman".
    parts: Vec<String>,
    /// Lowercase words between, as "ur" and "van der" are.
    particles: Vec<String>,
    /// The parts' first letters.
    initials: Vec<char>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Part { capitalised: bool },
    Particle,
    Initial,
}

#[derive(Clone, Debug)]
struct Token {
    range: Range<usize>,
    kind: Kind,
}

impl Name {
    fn new(names: &[&str]) -> Self {
        let mut name = Self {
            parts: Vec::new(),
            particles: Vec::new(),
            initials: Vec::new(),
        };
        for word in names.iter().flat_map(|full| WORD.find_iter(full)) {
            let word = word.as_str();
            let lowercase = normalize(word);
            if word.chars().next().is_some_and(char::is_uppercase) {
                if word.chars().count() > 1 && !name.parts.contains(&lowercase) {
                    name.parts.push(lowercase);
                }
                name.initials.extend(word.chars().next());
            } else if !name.particles.contains(&lowercase) {
                name.particles.push(lowercase);
            }
        }
        name
    }

    /// Whether `text` is a username made from the name: it holds a part of
    /// the name, glued to what a word doesn't have, a digit, "_", "@", a
    /// capital after a small letter ("AlexR"), a hyphen in lowercase
    /// ("alex-r") or another part ("alexrahman"). "Markdown" and
    /// "Alexander" are words.
    fn handle(&self, text: &str) -> bool {
        let lowercase = normalize(text);
        let camel = text
            .chars()
            .zip(text.chars().skip(1))
            .any(|(before, after)| before.is_lowercase() && after.is_uppercase());
        let glued = text
            .chars()
            .any(|character| character.is_ascii_digit() || matches!(character, '_' | '@'))
            || camel
            || (text.contains('-') && !text.chars().any(char::is_uppercase))
            || self
                .parts
                .iter()
                .filter(|part| lowercase.contains(part.as_str()))
                .count()
                > 1;
        glued && !self.parts.contains(&lowercase) && self.within(text)
    }

    /// Whether `text`, as a web address, holds a part of the name long
    /// enough not to be in other words by chance.
    fn within(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        self.parts
            .iter()
            .any(|part| part.chars().count() >= MIN_PART_IN_ADDRESS && text.contains(part.as_str()))
    }

    /// Where the name is written in `text`, outside `taken`. A part written
    /// in lowercase counts only beside another, so "mark the release" keeps
    /// its word when the user is Mark.
    fn find(&self, text: &str, taken: &[Found]) -> Vec<Range<usize>> {
        let mut found = Vec::new();
        let mut run: Vec<Token> = Vec::new();
        for word in WORD.find_iter(text) {
            let outside = !taken
                .iter()
                .any(|found| found.range.start < word.end() && word.start() < found.range.end);
            let token = outside
                .then(|| self.token(word.as_str(), word.start()))
                .flatten();
            let joins = match (run.last(), &token) {
                (Some(last), Some(token)) => joins(
                    text[last.range.end..token.range.start].trim_matches([' ', '\t']),
                    last.kind,
                    token.kind,
                ),
                _ => false,
            };
            if !joins {
                found.extend(finish(text, std::mem::take(&mut run)));
            }
            run.extend(token);
        }
        found.extend(finish(text, run));
        found
    }

    fn token(&self, word: &str, start: usize) -> Option<Token> {
        let stem = possessive_stem(word);
        let lowercase = normalize(stem);
        let kind = if self.parts.contains(&lowercase) {
            Kind::Part {
                capitalised: stem.chars().next().is_some_and(char::is_uppercase),
            }
        } else if self.particles.contains(&lowercase) {
            Kind::Particle
        } else if stem.chars().count() <= MAX_INITIALS
            && stem
                .chars()
                .all(|letter| letter.is_uppercase() && self.initials.contains(&letter))
        {
            Kind::Initial
        } else {
            return None;
        };
        Some(Token {
            range: start..start + stem.len(),
            kind,
        })
    }
}

/// Whether two words of a name, with `gap` between, are one way of writing
/// it: "Alex Rahman", "al-Farsi", "A. Rahman", "Rahman, A.".
fn joins(gap: &str, left: Kind, right: Kind) -> bool {
    match gap {
        "" | "-" => true,
        "." => left == Kind::Initial,
        "," => matches!(left, Kind::Part { .. }) && right == Kind::Initial,
        _ => false,
    }
}

/// The span of a run of name words, if it is the name: it holds a
/// capitalised part or two parts, ends on a part or an initial closed by
/// punctuation (so "Rahman I led" keeps its "I"), and takes the initial's
/// full stop.
fn finish(text: &str, mut run: Vec<Token>) -> Option<Range<usize>> {
    while let Some(last) = run.last() {
        let closed = || {
            text[last.range.end..]
                .chars()
                .next()
                .is_none_or(|next| matches!(next, '.' | ',' | ';' | ')' | '\n'))
        };
        match last.kind {
            Kind::Part { .. } => break,
            Kind::Initial if closed() => break,
            _ => {
                run.pop();
            }
        }
    }
    let parts = run
        .iter()
        .filter(|token| matches!(token.kind, Kind::Part { .. }))
        .count();
    let capitalised = run
        .iter()
        .any(|token| token.kind == Kind::Part { capitalised: true });
    if !capitalised && parts < 2 {
        return None;
    }
    let (first, last) = (run.first()?, run.last()?);
    let stop = usize::from(last.kind == Kind::Initial && text[last.range.end..].starts_with('.'));
    Some(first.range.start..last.range.end + stop)
}

/// Adds `range` unless it overlaps one found before: an email address is
/// also a web address, and a link can hold a phone number's digits.
fn add(found: &mut Vec<Found>, identifier: Identifier, range: Range<usize>) {
    if !found
        .iter()
        .any(|taken| taken.range.start < range.end && range.start < taken.range.end)
    {
        found.push(Found { identifier, range });
    }
}

/// Where `phrase` is written in `text` as whole words, in any case.
fn phrase_ranges(text: &str, phrase: &str) -> Vec<Range<usize>> {
    let phrase = phrase.trim();
    let Some((first, last)) = phrase.chars().next().zip(phrase.chars().last()) else {
        return Vec::new();
    };
    // \b only beside a letter or digit, so "C++" can be hidden.
    let edge = |character: char| {
        if character.is_alphanumeric() {
            r"\b"
        } else {
            ""
        }
    };
    let pattern = format!("(?i){}{}{}", edge(first), regex::escape(phrase), edge(last));
    match Regex::new(&pattern) {
        Ok(pattern) => pattern.find_iter(text).map(|found| found.range()).collect(),
        Err(error) => {
            eprintln!("redaction: couldn't look for a hidden phrase: {error}");
            Vec::new()
        }
    }
}

/// A number with phone-number digits that isn't a date or a span of years.
fn is_phone(number: &str) -> bool {
    let groups: Vec<&str> = number
        .split(|character: char| !character.is_ascii_digit())
        .filter(|group| !group.is_empty())
        .collect();
    let digits = groups.iter().map(|group| group.len()).sum();
    let year =
        |group: &&str| group.len() == 4 && group.parse().is_ok_and(|year| YEARS.contains(&year));
    let date =
        groups.iter().any(year) && groups.iter().all(|group| year(group) || group.len() <= 2);
    PHONE_DIGITS.contains(&digits) && !date
}

/// "Rahman" for "Rahman's".
fn possessive_stem(word: &str) -> &str {
    ["'s", "’s", "'S", "’S"]
        .iter()
        .find_map(|possessive| word.strip_suffix(possessive))
        .unwrap_or(word)
}

fn normalize(word: &str) -> String {
    word.replace('’', "'").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redacted(text: &str, names: &[&str]) -> String {
        Redaction::new(text, &find(text, names, &[])).text
    }

    #[test]
    fn every_way_of_writing_the_name_is_one_placeholder() {
        let names = ["Alex Rahman"];
        for (text, expected) in [
            ("Alex Rahman", "[NAME]"),
            ("ALEX RAHMAN", "[NAME]"),
            ("alex rahman", "[NAME]"),
            ("Alex led it.", "[NAME] led it."),
            ("Rahman's thesis", "[NAME]'s thesis"),
            ("Rahman’s thesis", "[NAME]’s thesis"),
            ("Smith J, Rahman A, Lee K.", "Smith J, [NAME], Lee K."),
            (
                "Smith, J., Rahman, A., Lee, K.",
                "Smith, J., [NAME], Lee, K.",
            ),
            ("J. Smith and A. Rahman.", "J. Smith and [NAME]."),
            ("A.R. Rahman", "[NAME]"),
            ("Rahman AR, Lee K.", "[NAME], Lee K."),
        ] {
            assert_eq!(redacted(text, &names), expected, "{text}");
        }
    }

    #[test]
    fn particles_and_hyphens_stay_inside_the_name() {
        for (names, text, expected) in [
            (["Sami ul Haque"], "Sami ul Haque", "[NAME]"),
            (["Sami ul Haque"], "Sami-ul Haque wrote", "[NAME] wrote"),
            (["Sami ul Haque"], "Haque ul", "[NAME] ul"),
            (["Daan van der Velde"], "Van der Velde, D.", "[NAME]"),
            (
                ["Omar al-Farsi"],
                "Omar Al-Farsi and al Farsi",
                "[NAME] and [NAME]",
            ),
            (["Siobhán O'Neill"], "Siobhán O’Neill's", "[NAME]'s"),
            (["Ronan McDonald"], "R. McDonald", "[NAME]"),
        ] {
            assert_eq!(redacted(text, &names), expected, "{text}");
        }
    }

    #[test]
    fn words_that_only_look_like_the_name_stay() {
        let names = ["Mark Rahman"];
        for text in ["Will mark the release.", "Benchmarked it.", "Rahmania"] {
            assert_eq!(redacted(text, &names), text);
        }
        assert_eq!(
            redacted("Ian Rahman I led it. In 2020 I joined.", &["Ian Rahman"]),
            "[NAME] I led it. In 2020 I joined.",
            "\"I\" is a word, not an initial, when no punctuation closes it"
        );
    }

    #[test]
    fn several_names_are_all_replaced() {
        assert_eq!(
            redacted(
                "Alex R. Rahman, born Alexander",
                &["Alex Rahman", "Alexander Rahman"]
            ),
            "[NAME], born [NAME]"
        );
    }

    #[test]
    fn contacts_are_replaced() {
        let names = ["Alex Rahman"];
        for (text, expected) in [
            ("alex.rahman@example.com", "[EMAIL]"),
            ("mailto:a.r@mail.example.org.", "[EMAIL]."),
            ("+1 (555) 123-4567", "[PHONE]"),
            ("Call +880 1712-345678 now", "Call [PHONE] now"),
            ("555.123.4567", "[PHONE]"),
            ("https://github.com/alexr.", "[LINK]."),
            ("www.example.org/about", "[LINK]"),
            ("linkedin.com/in/someone", "[LINK]"),
            ("alexrahman.dev", "[LINK]"),
            ("alex@alexrahman.dev", "[EMAIL]"),
            (
                "GitHub: alex-r, X: AlexR. Mail alex_99",
                "GitHub: [HANDLE], X: [HANDLE]. Mail [HANDLE]",
            ),
            ("@alex", "[HANDLE]"),
            ("alexrahman", "[HANDLE]"),
            ("AlexRahman's", "[HANDLE]'s"),
        ] {
            assert_eq!(redacted(text, &names), expected, "{text}");
        }
    }

    #[test]
    fn dates_figures_and_tools_are_not_contacts() {
        let names = ["Alex Rahman"];
        for text in [
            "2019 - 2023",
            "2018-2020, 2021-2023",
            "2023-01-15",
            "15.01.2023",
            "1,200,000 users",
            "Node.js, ASP.NET and Socket.io",
            "Worked at Amazon.com",
            "README.md",
            "Alexander used Markdown",
        ] {
            assert_eq!(redacted(text, &names), text, "{text}");
        }
        assert_eq!(
            redacted("Worked at Netlify.com", &["Li Rahman"]),
            "Worked at Netlify.com",
            "a short part of the name is in addresses by chance"
        );
    }

    #[test]
    fn phrases_the_user_hides_are_hidden_as_whole_words() {
        let text = "Built Kafka at Northwind for northwind.example; used C++ and Northwinds.";
        let found = find(text, &["Alex Rahman"], &["northwind", "C++", " "]);
        assert_eq!(
            Redaction::new(text, &found).text,
            "Built Kafka at [HIDDEN] for [HIDDEN].example; used [HIDDEN] and Northwinds."
        );
    }

    #[test]
    fn what_the_user_keeps_is_sent_as_written() {
        let text = "Alex Rahman, alex@example.com";
        let found = find(text, &["Alex Rahman"], &[]);
        let hidden: Vec<Found> = found
            .into_iter()
            .filter(|found| found.identifier != Identifier::Email)
            .collect();
        assert_eq!(
            Redaction::new(text, &hidden).text,
            "[NAME], alex@example.com"
        );
    }

    #[test]
    fn quotes_in_the_redacted_text_map_back_to_the_original() {
        let text = "Alex Rahman\nalex@example.com\nLed Rahman's team of 12.";
        let redaction = Redaction::new(text, &find(text, &["Alex Rahman"], &[]));
        assert_eq!(redaction.text, "[NAME]\n[EMAIL]\nLed [NAME]'s team of 12.");
        let original = |quote: &str| {
            let start = redaction.text.find(quote).expect("a quote in the text");
            &text[redaction.original(start..start + quote.len())]
        };
        assert_eq!(
            original("Led [NAME]'s team of 12."),
            "Led Rahman's team of 12."
        );
        assert_eq!(original("[EMAIL]"), "alex@example.com");
        assert_eq!(original("AME]\n[EM"), "Alex Rahman\nalex@example.com");
        assert_eq!(original("team of 12."), "team of 12.");
        assert_eq!(original("\n"), "\n");
    }
}
