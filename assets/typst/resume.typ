// Résumé template. `typeset.rs` appends the content as calls to the functions
// below, one per generated line, each followed by a `<line-…>` label.
//
// Every line function returns a single element (a block or a list) so the
// label attaches to it and the app can find where the line was laid out.

#set page(paper: "us-letter", margin: (x: 0.75in, y: 0.7in))
#set text(font: ("Inter", "Noto Sans", "Libertinus Serif"), size: 10pt, fill: rgb("#1f2328"))
#set par(leading: 0.5em, spacing: 0.55em)
#set list(marker: [•], indent: 2pt, body-indent: 6pt, spacing: 0.5em)

#let muted = rgb("#59636e")

#let section(title) = {
  v(10pt)
  text(size: 8.5pt, weight: "semibold", fill: muted, tracking: 0.04em, upper(title))
  v(-5pt)
  line(length: 100%, stroke: 0.5pt + rgb("#d0d7de"))
}

#let name(body) = block(text(size: 20pt, weight: "bold", body))
#let contact(body) = block(text(size: 9pt, fill: muted, body))
#let role(body) = block(above: 0.9em, text(weight: "semibold", body))
#let bullet(body) = list(body)
#let plain(body) = block(body)
