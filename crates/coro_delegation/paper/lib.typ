// Helpers shared by paper.typ and the generated tables/*.typ.

// median [min, max] over repeats, as printed by scripts/make_tables.py
#let rng(lo, hi) = text(size: 7pt)[#h(0.17em)\[#lo,#h(0.17em)#hi\]]

// Scale content down to the available width if it is wider (like
// \adjustbox{max width=\linewidth}); never scales up.
#let fit(body) = layout(size => {
  let w = measure(body).width
  if w > size.width {
    scale(size.width / w * 100%, origin: top + left, reflow: true, body)
  } else {
    body
  }
})
