#set document(title: "Service-Fair Delegation")
#set page(paper: "us-letter", margin: (x: 0.75in, y: 1in), columns: 2)
#set columns(gutter: 0.33in)
#set text(font: "New Computer Modern", size: 10pt)
#set par(justify: true, first-line-indent: 1em, spacing: 0.65em)
#set heading(numbering: "1.1")
#show heading: set block(above: 1.2em, below: 0.7em)
#show heading.where(level: 1): set text(size: 11pt)

#place(top + center, float: true, scope: "parent", clearance: 2em)[
  #text(size: 16pt, weight: "bold")[Service-Fair Delegation: \ Scheduling Critical-Section Time Inside the Combiner]
  #v(0.5em)
  #text(size: 11pt)[Draft]
]

#include "sections/abstract.typ"

#include "sections/introduction.typ"

#bibliography("refs.bib", style: "association-for-computing-machinery")
