// Sidebar: two columns. Contacts, skills and education sit in a tinted strip
// down the left; the name, summary and experience take the rest.
//
// The app appends the résumé as `resume` and calls `render(resume)`; its
// shape is described in `typeset.rs`. Every piece of content in it is a line
// the app tracks: style it, place it, but don't turn it into a string.

#let accent = rgb("#1d5a70")
#let muted = rgb("#5b6670")
#let margin = 0.5in
#let sidebar-width = 1.9in
// Wide, and mostly white past the strip: the app marks edited lines in the
// margin left of the main column.
#let gutter = 0.6in

#set page(
  paper: "us-letter",
  margin: (left: margin, right: 0.6in, y: 0.55in),
  background: place(left + top, rect(
    width: margin + sidebar-width + 0.1in,
    height: 100%,
    fill: rgb("#eef3f5"),
  )),
)
#set text(font: ("Inter", "Noto Sans", "Libertinus Serif"), size: 9.5pt, fill: rgb("#20262c"))
#set par(leading: 0.5em, spacing: 0.6em)
#set list(marker: text(fill: accent, sym.bullet), indent: 0pt, body-indent: 6pt, spacing: 0.5em)

#let sidebar-kinds = ("skills", "education")

// Sticky: a title is never the last thing on a page.
#let section-title(title) = std.block(
  above: 14pt,
  below: 0.6em,
  sticky: true,
  text(size: 8.5pt, weight: "bold", fill: accent, tracking: 0.08em, upper(title)),
)

#let contacts(block) = {
  section-title("Contact")
  for contact in block.contacts {
    let body = text(size: 8.5pt, contact.text)
    std.block(above: 0.45em, if contact.link == none { body } else { link(contact.link, body) })
  }
}

// In the narrow strip, everything about an entry stacks.
#let narrow-entry(block) = std.block(above: 0.7em, {
  text(weight: "semibold", block.title)
  for detail in (block.organization, block.location, block.dates).filter(detail => detail != none) {
    linebreak()
    text(size: 8.5pt, fill: muted, detail)
  }
})

// The heading rows are sticky, so they stay on the page with the bullets.
// Dates go on the title's line, pushed right, to share its baseline.
#let entry(block) = {
  std.block(above: 1em, below: 0.45em, sticky: true, {
    text(size: 10.5pt, weight: "semibold", fill: accent, block.title)
    if block.dates != none [#h(1fr) #text(size: 8.5pt, fill: muted, block.dates)]
  })
  let details = (block.organization, block.location, block.stack).filter(detail => detail != none)
  if details.len() > 0 {
    std.block(above: 0.3em, below: 0.55em, sticky: true, text(fill: muted, details.join[ #sym.dot.c ]))
  }
  if block.bullets.len() > 0 { list(..block.bullets) }
}

#let skills(block) = std.block(above: 0.7em, {
  if block.label != none { text(weight: "semibold", block.label) }
  linebreak()
  text(size: 8.5pt, block.items)
})

#let section(section, narrow) = {
  if section.title != "" { section-title(section.title) }
  for block in section.blocks {
    if block.kind == "entry" { if narrow { narrow-entry(block) } else { entry(block) } }
    else if block.kind == "skills" { skills(block) }
    else if block.kind == "text" { std.block(block.body) }
  }
  let items = section.blocks.filter(block => block.kind == "item")
  if items.len() > 0 { list(..items.map(item => item.body)) }
}

#let render(resume) = {
  let header = resume.sections
    .map(section => section.blocks)
    .flatten()
    .find(block => block.kind == "header")
  let sections = resume.sections.filter(section => section.kind != "header")
  grid(
    columns: (sidebar-width, 1fr),
    column-gutter: gutter,
    {
      if header != none { contacts(header) }
      for part in sections.filter(section => section.kind in sidebar-kinds) { section(part, true) }
    },
    {
      if header != none {
        text(size: 26pt, weight: "bold", fill: accent, header.name)
        v(4pt)
      }
      for part in sections.filter(section => section.kind not in sidebar-kinds) { section(part, false) }
    },
  )
}
