// Reads Markdown into the text it shows, with markup left out, as a CV's
// text is read from a PDF: one line per heading, paragraph, list item and
// table row, a blank line between blocks. Lines wrapped in the source are
// joined, as Markdown shows them. Headings get the sizes a browser shows
// them in, so the name guess can find a `# Name` at the top.

use std::collections::HashMap;

use ::markdown::mdast::Node;
use ::markdown::{ParseOptions, to_mdast};

use super::{BODY_SIZE, Error, Link, Reading, TextLine};

/// A browser's default sizes for `h1` to `h6`, relative to the body text
/// (HTML Standard, 15.3.6 "Sections and headings").
const HEADING_SCALES: [f32; 6] = [2., 1.5, 1.17, 1., 0.83, 0.67];

/// The text the Markdown in `source` shows.
pub fn read(source: &str) -> Result<Reading, Error> {
    // Only MDX syntax fails to parse, and MDX is off.
    let root = to_mdast(source, &ParseOptions::gfm()).map_err(|_| Error::Invalid)?;
    let mut definitions = HashMap::new();
    collect_definitions(&root, &mut definitions);
    let mut writer = Writer {
        reading: Reading::default(),
        definitions,
        line: None,
    };
    writer.block(&root, 0);
    writer.close_line();
    let end = writer.reading.text.trim_end().len();
    writer.reading.text.truncate(end);
    if writer.reading.text.is_empty() {
        return Err(Error::Empty);
    }
    Ok(writer.reading)
}

struct Writer<'a> {
    reading: Reading,
    /// Link references' addresses, by identifier.
    definitions: HashMap<&'a str, &'a str>,
    /// Where the line being written starts, and its size.
    line: Option<(usize, f32)>,
}

impl<'a> Writer<'a> {
    fn block(&mut self, node: &'a Node, indent: usize) {
        match node {
            Node::Root(_) | Node::Blockquote(_) | Node::FootnoteDefinition(_) => {
                for child in node.children().into_iter().flatten() {
                    self.block(child, indent);
                }
            }
            Node::Heading(heading) => {
                let scale = HEADING_SCALES
                    .get(usize::from(heading.depth).saturating_sub(1))
                    .copied()
                    .unwrap_or(1.);
                self.open_line(BODY_SIZE * scale);
                self.inlines(&heading.children);
                self.end_block();
            }
            Node::Paragraph(paragraph) => {
                self.inlines(&paragraph.children);
                self.end_block();
            }
            Node::List(list) => {
                for (number, item) in list.children.iter().enumerate() {
                    self.open_line(BODY_SIZE);
                    self.reading.text.push_str(&"  ".repeat(indent));
                    match list.start {
                        Some(start) if list.ordered => {
                            let number = u32::try_from(number).unwrap_or(u32::MAX);
                            self.reading
                                .text
                                .push_str(&format!("{}. ", start.saturating_add(number)));
                        }
                        _ => self.reading.text.push_str("- "),
                    }
                    for child in item.children().into_iter().flatten() {
                        if let Node::Paragraph(paragraph) = child {
                            self.inlines(&paragraph.children);
                            self.close_line();
                        } else {
                            self.block(child, indent + 1);
                        }
                    }
                    self.close_line();
                }
                if indent == 0 {
                    self.end_block();
                }
            }
            Node::Table(table) => {
                for row in &table.children {
                    self.open_line(BODY_SIZE);
                    for (index, cell) in row.children().into_iter().flatten().enumerate() {
                        if index > 0 {
                            self.reading.text.push_str(" | ");
                        }
                        self.inlines(cell.children().map_or(&[][..], Vec::as_slice));
                    }
                    self.close_line();
                }
                self.end_block();
            }
            Node::Code(code) => {
                for line in code.value.lines() {
                    self.open_line(BODY_SIZE);
                    self.reading.text.push_str(line);
                    self.close_line();
                }
                self.end_block();
            }
            Node::Math(math) => {
                self.open_line(BODY_SIZE);
                self.reading.text.push_str(&math.value);
                self.end_block();
            }
            // Not shown: HTML, front matter, link definitions, rules.
            _ => {}
        }
    }

    fn inlines(&mut self, nodes: &'a [Node]) {
        for node in nodes {
            self.inline(node);
        }
    }

    fn inline(&mut self, node: &'a Node) {
        match node {
            // A line wrapped in the source shows as one line.
            Node::Text(text) => self.push(&text.value.replace('\n', " ")),
            Node::InlineCode(code) => self.push(&code.value),
            Node::InlineMath(math) => self.push(&math.value),
            Node::Emphasis(_) | Node::Strong(_) | Node::Delete(_) => {
                for child in node.children().into_iter().flatten() {
                    self.inline(child);
                }
            }
            Node::Link(link) => self.link(&link.url, &link.children),
            Node::LinkReference(reference) => {
                match self.definitions.get(reference.identifier.as_str()) {
                    Some(address) => self.link(address, &reference.children),
                    // Without a definition, it shows as the text it is.
                    None => self.inlines(&reference.children),
                }
            }
            Node::Break(_) => self.close_line(),
            // Not shown: images, inline HTML, footnote marks.
            _ => {}
        }
    }

    fn link(&mut self, address: &str, children: &'a [Node]) {
        self.open_line(BODY_SIZE);
        let start = self.reading.text.len();
        self.inlines(children);
        let end = self.reading.text.len();
        let span = (end > start).then_some(start..end);
        self.reading.links.push(Link {
            address: address.to_owned(),
            line: span.as_ref().map(|_| self.reading.lines.len()),
            span,
        });
    }

    fn push(&mut self, text: &str) {
        self.open_line(BODY_SIZE);
        self.reading.text.push_str(text);
    }

    fn open_line(&mut self, size: f32) {
        if self.line.is_none() {
            self.line = Some((self.reading.text.len(), size));
        }
    }

    /// Ends the line being written, if any; an empty one is left out.
    fn close_line(&mut self) {
        let Some((start, size)) = self.line.take() else {
            return;
        };
        let end = start + self.reading.text[start..].trim_end().len();
        self.reading.text.truncate(end);
        if self.reading.text[start..].trim().is_empty() {
            self.reading.text.truncate(start);
            return;
        }
        self.reading.lines.push(TextLine {
            span: start..end,
            page: 0,
            size,
        });
        self.reading.text.push('\n');
    }

    /// Ends the line, then leaves a blank line before the next block.
    fn end_block(&mut self) {
        self.close_line();
        if !self.reading.text.is_empty() && !self.reading.text.ends_with("\n\n") {
            self.reading.text.push('\n');
        }
    }
}

fn collect_definitions<'a>(node: &'a Node, definitions: &mut HashMap<&'a str, &'a str>) {
    if let Node::Definition(definition) = node {
        definitions.insert(definition.identifier.as_str(), definition.url.as_str());
    }
    for child in node.children().into_iter().flatten() {
        collect_definitions(child, definitions);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(source: &str) -> Reading {
        read(source).expect("test Markdown that reads")
    }

    fn lines(reading: &Reading) -> Vec<(&str, f32)> {
        reading
            .lines
            .iter()
            .map(|line| (&reading.text[line.span.clone()], line.size))
            .collect()
    }

    const CV: &str = include_str!("markdown/cv.md");

    #[test]
    fn markup_is_left_out_and_blocks_are_lines() {
        let reading = reading(CV);
        assert_eq!(
            reading.text,
            "Alex Rahman\n\
             \n\
             alex.rahman@example.com · GitHub\n\
             \n\
             Experience\n\
             \n\
             Northwind Logistics, Senior Software Engineer 2021 – present\n\
             \n\
             - Led the migration of 12 backend services from Python to Rust.\n\
             - Cut p99 latency from 480 ms to 95 ms.\n  \
             - Nested detail"
        );
    }

    #[test]
    fn headings_are_sized_as_a_browser_shows_them() {
        let reading = reading(CV);
        let lines = lines(&reading);
        assert_eq!(lines[0], ("Alex Rahman", 24.));
        assert_eq!(lines[2], ("Experience", 18.));
        assert_eq!(lines[3].1, BODY_SIZE);
    }

    #[test]
    fn links_keep_their_address_and_the_text_they_cover() {
        let reading = reading(CV);
        let addresses: Vec<_> = reading
            .links
            .iter()
            .map(|link| {
                let text = link.span.clone().map(|span| &reading.text[span]);
                (link.address.as_str(), text, link.line)
            })
            .collect();
        assert_eq!(
            addresses,
            [
                (
                    "mailto:alex.rahman@example.com",
                    Some("alex.rahman@example.com"),
                    Some(1)
                ),
                ("https://github.com/alexr", Some("GitHub"), Some(1)),
            ],
            "a bare email is a link too, as GitHub shows it"
        );
    }

    #[test]
    fn a_link_reference_takes_its_definitions_address() {
        let reading = reading("See [my site][site].\n\n[Site]: https://alexr.dev\n");
        assert_eq!(reading.text, "See my site.");
        assert_eq!(reading.links[0].address, "https://alexr.dev");
    }

    #[test]
    fn tables_read_row_by_row() {
        let reading = reading("| Skill | Years |\n| --- | --- |\n| Rust | 5 |\n");
        assert_eq!(reading.text, "Skill | Years\nRust | 5");
    }

    #[test]
    fn markdown_that_shows_nothing_is_empty() {
        assert!(matches!(
            read("<!-- a comment -->\n\n---\n"),
            Err(Error::Empty)
        ));
    }
}
