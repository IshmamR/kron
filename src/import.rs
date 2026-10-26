// Reads the documents a user imports into plain text, on the device and
// with no model, along with what fact extraction needs besides the words:
// each line's font size, for guessing the user's name, and the links, whose
// addresses often appear nowhere in the text.

pub mod pdf;

use std::ops::Range;

/// A document read into text.
#[derive(Debug, Default)]
pub struct Reading {
    /// What `document_texts.text` stores; the spans below are byte offsets
    /// into it.
    pub text: String,
    /// In reading order.
    pub lines: Vec<TextLine>,
    pub links: Vec<Link>,
    /// What may be missing or wrong in the text, for the user to check.
    pub warnings: Vec<Warning>,
}

#[derive(Debug, PartialEq)]
pub struct TextLine {
    pub span: Range<usize>,
    /// Counted from 0.
    pub page: usize,
    /// Its largest font size, in points.
    pub size: f32,
}

#[derive(Debug, PartialEq)]
pub struct Link {
    pub address: String,
    /// The text it covers; none when it covers only an icon.
    pub span: Option<Range<usize>>,
    /// The line it sits on, as an index into `lines`.
    pub line: Option<usize>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Warning {
    /// The page has pictures but no text, like a scanned page, so none of
    /// its words were read.
    NoText { page: usize },
    /// The page uses a font drawn as pictures (Type 3), which gives no
    /// letter widths and often no text, so its words may come apart or be
    /// missing.
    DrawnFont { page: usize },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Needs a password to open.
    Encrypted,
    /// Not a document of its kind, or too damaged to read.
    Invalid,
    /// Has no text to read, like a scan.
    NoText,
}
