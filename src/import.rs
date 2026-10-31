// Reads the documents a user imports into plain text, on the device and
// with no model, along with what fact extraction needs besides the words:
// each line's font size, for guessing the user's name, and the links, whose
// addresses often appear nowhere in the text.

pub mod markdown;
pub mod pdf;

use std::ops::Range;
use std::path::Path;

/// The font size given to lines of text that has none, in points: a
/// browser's default of 16px (CSS's 96px to 72pt).
const BODY_SIZE: f32 = 12.;

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
    /// A text file with nothing in it.
    Empty,
    /// A text file not in UTF-8 or UTF-16, or not text at all.
    Encoding,
    /// A kind of file that can't be read.
    Unsupported,
}

/// What a file is, to know how to read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Pdf,
    Markdown,
    Text,
}

impl Format {
    /// A PDF by its header, whatever the file's name; other files by their
    /// extension, since any file can look like text.
    pub fn of(file_name: &str, bytes: &[u8]) -> Result<Self, Error> {
        // PDF readers look for the header within the first 1024 bytes
        // (PDF 2.0, annex H).
        let header = &bytes[..bytes.len().min(1024)];
        if header.windows(5).any(|window| window == b"%PDF-") {
            return Ok(Self::Pdf);
        }
        let extension = Path::new(file_name)
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase());
        match extension.as_deref() {
            Some("pdf") => Ok(Self::Pdf),
            Some("md" | "markdown") => Ok(Self::Markdown),
            None | Some("txt" | "text") => Ok(Self::Text),
            Some(_) => Err(Error::Unsupported),
        }
    }

    /// What `files.media_type` stores.
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Pdf => "application/pdf",
            // RFC 7763.
            Self::Markdown => "text/markdown",
            Self::Text => "text/plain",
        }
    }
}

/// The text of the file in `bytes`.
pub fn read(format: Format, bytes: Vec<u8>) -> Result<Reading, Error> {
    match format {
        Format::Pdf => pdf::read(bytes),
        Format::Markdown => markdown::read(&decode(&bytes)?),
        Format::Text => {
            let text = decode(&bytes)?.replace("\r\n", "\n");
            let mut lines = Vec::new();
            let mut start = 0;
            for line in text.split_inclusive('\n') {
                let content = line.trim_end();
                if !content.is_empty() {
                    lines.push(TextLine {
                        span: start..start + content.len(),
                        page: 0,
                        size: BODY_SIZE,
                    });
                }
                start += line.len();
            }
            Ok(Reading {
                text,
                lines,
                ..Reading::default()
            })
        }
    }
}

/// Text in UTF-8, or in UTF-16 with a byte order mark, as Windows' Notepad
/// once saved it.
fn decode(bytes: &[u8]) -> Result<String, Error> {
    let utf16 = |bytes: &[u8], unit: fn([u8; 2]) -> u16| {
        let units = bytes.chunks_exact(2).map(|pair| unit([pair[0], pair[1]]));
        char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .map_err(|_| Error::Encoding)
    };
    let text = match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => {
            String::from_utf8(rest.to_vec()).map_err(|_| Error::Encoding)?
        }
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes)?,
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes)?,
        _ => String::from_utf8(bytes.to_vec()).map_err(|_| Error::Encoding)?,
    };
    // Text files don't hold NUL; a file that does is something else named
    // as text.
    if text.contains('\0') {
        return Err(Error::Encoding);
    }
    if text.trim().is_empty() {
        return Err(Error::Empty);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pdf_is_known_by_its_header_and_others_by_their_extension() {
        assert_eq!(Format::of("cv.txt", b"%PDF-1.7\n"), Ok(Format::Pdf));
        assert_eq!(Format::of("cv.PDF", b"damaged"), Ok(Format::Pdf));
        assert_eq!(Format::of("cv.Markdown", b"# Alex"), Ok(Format::Markdown));
        assert_eq!(Format::of("cv.txt", b"Alex"), Ok(Format::Text));
        assert_eq!(Format::of("CV", b"Alex"), Ok(Format::Text));
        assert_eq!(
            Format::of("cv.docx", b"PK\x03\x04"),
            Err(Error::Unsupported)
        );
    }

    #[test]
    fn text_is_read_as_it_is_with_one_line_per_line() {
        let reading =
            read(Format::Text, b"Alex Rahman\r\n\r\nRust  \r\n".to_vec()).expect("text that reads");
        assert_eq!(reading.text, "Alex Rahman\n\nRust  \n");
        let lines: Vec<_> = reading
            .lines
            .iter()
            .map(|line| &reading.text[line.span.clone()])
            .collect();
        assert_eq!(lines, ["Alex Rahman", "Rust"]);
    }

    #[test]
    fn text_in_utf16_with_a_byte_order_mark_is_read() {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("Zoë".encode_utf16().flat_map(u16::to_le_bytes));
        let reading = read(Format::Text, bytes).expect("UTF-16 that reads");
        assert_eq!(reading.text, "Zoë");
    }

    #[test]
    fn text_that_isnt_text_or_is_empty_says_so() {
        assert_eq!(
            read(Format::Text, vec![0xC3, 0x28]).map(|_| ()),
            Err(Error::Encoding),
            "not UTF-8"
        );
        assert_eq!(
            read(Format::Text, b"PK\x03\x04\0\0".to_vec()).map(|_| ()),
            Err(Error::Encoding),
            "binary"
        );
        assert_eq!(
            read(Format::Text, b"\xEF\xBB\xBF \n".to_vec()).map(|_| ()),
            Err(Error::Empty)
        );
    }
}
