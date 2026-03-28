// The project function defines how your document looks.
// It takes your content and some metadata and formats it.
// Go ahead and customize it to your liking!
#let project(title: "", authors: (), advisor : (), body) = {
  // Set the document's basic properties.
  set document(author: authors, title: title)
  set page(numbering: "1", number-align: center, margin: 1in)
  set text(font: "Charis SIL", lang: "en", weight: 300, size: 11pt)
  set par(justify: false)
   
  set heading(numbering: "1.1")
   
  set block(below: 1.5em, above: 1.5em)
   
  set par(leading: 1em, linebreaks: auto, first-line-indent: 1.5em)

  set enum(spacing: 1.5em)
  
  // Title row.
  align(center)[
    #block(text(font: "quicksand", weight: 700, 1.75em, title))
  ]
   
  // Author information.
  pad(
    top: 0.5em,
    x: 2em,
    grid(
      columns: (1fr,) * calc.min(3, authors.len()),
      gutter: 1em,
      ..authors.map(author => text(font: "Innovate")[#align(center, strong(author))]),
    ),
  )

  // Advisor information.
  pad(
    x: 2em,
    grid(
      columns: (1fr,),
      gutter: 1em,
      text(font: "Innovate")[#align(center, strong[Advisor: #advisor])]
    ),
  )
   
  // Main body.
  set par(justify: true)
   
  body
}