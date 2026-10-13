// Modern: sans serif, muted small-caps section titles over a hairline, dates
// flush right.
//
// The app appends the résumé as `resume` and calls `render(resume)`; its
// shape is described in `typeset.rs`. Every piece of content in it is a line
// the app tracks: style it, place it, but don't turn it into a string.

#set page(paper: "us-letter", margin: (x: 0.75in, y: 0.7in))
#set text(font: ("Inter", "Noto Sans", "Libertinus Serif"), size: 10pt, fill: rgb("#1f2328"))
#set par(leading: 0.5em, spacing: 0.55em)
#set list(marker: [•], indent: 2pt, body-indent: 6pt, spacing: 0.5em)

#let muted = rgb("#59636e")

// Sticky: a title is never the last thing on a page.
#let section-title(title) = {
  v(10pt)
  std.block(sticky: true, {
    text(size: 8.5pt, weight: "semibold", fill: muted, tracking: 0.04em, upper(title))
    v(-5pt)
    line(length: 100%, stroke: 0.5pt + rgb("#d0d7de"))
  })
}

#let header(block) = {
  text(size: 20pt, weight: "bold", block.name)
  v(2pt)
  text(size: 9pt, fill: muted, block.contacts.map(contact => contact.text).join[ #sym.dot.c ])
}

// The heading rows are sticky, so they stay on the page with the bullets.
// Dates go on the title's line, pushed right, to share its baseline.
#let entry(block) = {
  std.block(above: 0.9em, below: 0.5em, sticky: true, {
    text(weight: "semibold", block.title)
    if block.organization != none [, #block.organization]
    if block.dates != none [#h(1fr) #text(fill: muted, block.dates)]
  })
  let details = (block.location, block.stack).filter(detail => detail != none)
  if details.len() > 0 {
    std.block(above: 0.3em, below: 0.5em, sticky: true, text(size: 9pt, fill: muted, details.join[ #sym.dot.c ]))
  }
  if block.bullets.len() > 0 { list(..block.bullets) }
}

#let skills(block) = grid(
  columns: (8em, 1fr),
  column-gutter: 0.5em,
  if block.label != none { text(weight: "semibold", block.label) },
  block.items,
)

#let render(resume) = {
  for section in resume.sections {
    if section.title != "" { section-title(section.title) }
    for block in section.blocks {
      if block.kind == "header" { header(block) }
      else if block.kind == "entry" { entry(block) }
      else if block.kind == "skills" { std.block(above: 0.5em, skills(block)) }
      else if block.kind == "text" { std.block(block.body) }
    }
    let items = section.blocks.filter(block => block.kind == "item")
    if items.len() > 0 { list(..items.map(item => item.body)) }
  }
}
