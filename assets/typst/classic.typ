// Classic: after Jake Gutierrez's LaTeX résumé. Computer Modern, a centered
// header, small-caps section titles over a rule, and each entry on two rows
// with its dates and place flush right.
//
// The app appends the résumé as `resume` and calls `render(resume)`; its
// shape is described in `typeset.rs`. Every piece of content in it is a line
// the app tracks: style it, place it, but don't turn it into a string.

#set page(paper: "us-letter", margin: (x: 0.55in, y: 0.5in))
#set text(font: "New Computer Modern", size: 10.5pt)
#set par(leading: 0.42em, spacing: 0.42em)
#set list(marker: [•], indent: 0.6em, body-indent: 0.5em, spacing: 0.32em)

#let maybe(style, body) = if body != none { style(body) }

// Sticky: a title is never the last thing on a page.
#let section-title(title) = std.block(
  above: 0.9em,
  below: 0.5em,
  sticky: true,
  width: 100%,
  inset: (bottom: 2.5pt),
  stroke: (bottom: 0.5pt),
  text(size: 12.5pt, smallcaps(title)),
)

#let header(block) = align(center, {
  text(size: 24pt, weight: "bold", block.name)
  v(-0.2em)
  let contacts = block.contacts.map(contact => {
    if contact.link == none or contact.link.starts-with("tel:") {
      contact.text
    } else {
      link(contact.link, underline(contact.text))
    }
  })
  text(size: 9.5pt, contacts.join[ #h(0.15em)|#h(0.15em) ])
})

// `right` pushed to the end of the line, so the two share a baseline.
#let row(left, right) = [#left #h(1fr) #right]

// A job leads with its title and dates; a school, like Jake's, with its name
// and place, the degree and dates below.
#let entry(kind, block) = {
  let (first, first-right, second, second-right) = if kind == "education" and block.organization != none {
    (block.organization, block.location, block.title, block.dates)
  } else if block.stack != none {
    ([#strong(block.title) | #emph(block.stack)], block.dates, block.organization, block.location)
  } else {
    (block.title, block.dates, block.organization, block.location)
  }
  // Sticky, so the heading rows stay on the page with the bullets.
  std.block(above: 0.62em, below: 0.4em, sticky: true, {
    row(if block.stack != none and kind != "education" { first } else { strong(first) }, first-right)
    if second != none or second-right != none {
      linebreak()
      row(maybe(emph, second), text(size: 9.5pt, maybe(emph, second-right)))
    }
  })
  if block.bullets.len() > 0 {
    pad(left: 0.5em, text(size: 9.8pt, list(..block.bullets)))
  }
}

#let skills(block) = std.block(above: 0.35em, below: 0.35em, {
  if block.label != none [#strong(block.label): ]
  block.items
})

#let render(resume) = {
  for section in resume.sections {
    if section.title != "" and section.kind != "header" { section-title(section.title) }
    pad(left: 0.4em, right: 0.4em, {
      for block in section.blocks {
        if block.kind == "header" { header(block) }
        else if block.kind == "entry" { entry(section.kind, block) }
        else if block.kind == "skills" { skills(block) }
        else if block.kind == "text" { std.block(block.body) }
      }
      let items = section.blocks.filter(block => block.kind == "item")
      if items.len() > 0 { text(size: 9.8pt, list(..items.map(item => item.body))) }
    })
  }
}
