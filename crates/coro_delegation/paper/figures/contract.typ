// Figure 2: the executor contract and the two burden policies, drawn with
// cetz. Hand-written (no data); included by paper.typ at two-column width.
#import "@preview/cetz:0.3.4": canvas, draw

// Okabe-Ito tints, matching the data figures (scripts/make_figures.py)
#let c-blue = rgb("#0072B2")
#let c-green = rgb("#009E73")
#let c-verm = rgb("#D55E00")
#let c-grey = rgb("#8C8C8C")
#let t-cs = c-blue.lighten(70%)
#let t-by = luma(225)
#let t-fc = c-green.lighten(65%)
#let arrow = (end: ">", fill: black, scale: 0.55)
#let thin = 0.5pt
#let tl = (paint: luma(160), thickness: 0.4pt) // timeline
#let brk = (paint: c-verm, dash: "dashed", thickness: 0.7pt)
#let pl(body) = text(fill: c-verm, weight: "bold", body) // a placement name

#let box_(a, b, body, fill: white, stroke: thin, name: none) = {
  draw.rect(a, b, fill: fill, stroke: stroke, radius: 0.04, name: name)
  draw.content(((a.at(0) + b.at(0)) / 2, (a.at(1) + b.at(1)) / 2), body)
}

// (a) workers with run-next slot, home inbox and local queue; shared
// injector; the three placements a lock can request for a wake.
#let panel-a = canvas(length: 1cm, {
  import draw: *
  set-style(stroke: thin)
  let ww = 2.9
  let gap = 0.45
  for w in range(2) {
    let x = w * (ww + gap)
    rect((x, 0.75), (x + ww, 2.45), radius: 0.07, stroke: 0.6pt)
    // titles sit clear of the Home arc (leaves worker 0 at left, enters worker 1 at right)
    if w == 0 {
      content((x + ww - 0.1, 2.4), anchor: "north-east", text(weight: "bold")[worker #w])
    } else {
      content((x + 0.1, 2.4), anchor: "north-west", text(weight: "bold")[worker #w])
    }
    box_((x + 0.1, 1.35), (x + 0.8, 1.75), [poll], fill: if w == 0 { t-cs } else { white })
    box_((x + 1.3, 1.35), (x + 2.0, 1.75), [next])
    box_((x + 2.1, 1.35), (x + 2.8, 1.75), [inbox])
    for k in range(4) {
      rect((x + 0.9 + k * 0.22, 0.87), (x + 1.1 + k * 0.22, 1.15), fill: white)
    }
    content((x + 1.85, 1.01), anchor: "west", text(size: 6.5pt)[queue])
  }
  let full = 2 * ww + gap
  rect((0, -0.4), (full, -0.05), radius: 0.04, fill: luma(245))
  content((full / 2, -0.225), [injector (shared)])
  // wakes issued by the lock while worker 0 polls
  line((0.8, 1.55), (1.3, 1.55), mark: arrow, stroke: 0.7pt)
  content((1.05, 1.78), anchor: "south", pl[Inline])
  line((0.45, 1.35), (0.45, -0.05), mark: arrow, stroke: 0.7pt)
  content((0.5, 0.35), anchor: "west", pl[Remote])
  bezier((0.45, 1.75), (ww + gap + 2.45, 1.75), (0.9, 3.05), (ww + gap + 2.2, 3.05), mark: arrow, stroke: 0.7pt)
  content((ww + gap / 2, 2.78), anchor: "south",
    [#pl[Home]: to the worker that last polled the wakee])
  // executor balancing
  bezier((ww + gap + 1.4, 0.87), (1.6, 0.87), (ww + gap + 1.2, 0.5), (1.8, 0.5), mark: arrow,
    stroke: (paint: c-grey, dash: "dashed", thickness: 0.6pt))
  content((ww + gap / 2 + 0.3, 0.52), anchor: "north", text(size: 6.5pt, fill: c-grey)[balancing steal every $b$ polls])
})

// (b) CES: inline chain on one worker, bounded at K, broken to a home worker.
#let panel-b = canvas(length: 1cm, {
  import draw: *
  set-style(stroke: thin)
  let y0 = 1.3
  let y1 = 0.1
  content((-0.08, y0 + 0.2), anchor: "east")[w0]
  content((-0.08, y1 + 0.2), anchor: "east")[w1]
  line((0, y0 - 0.04), (4.6, y0 - 0.04), stroke: tl)
  line((0, y1 - 0.04), (4.6, y1 - 0.04), stroke: tl)
  let labels = ($t_1$, $t_2$, $t_3$, $dots$, $t_K$)
  for (i, l) in labels.enumerate() {
    let x = i * 0.6
    if l == $dots$ {
      content((x + 0.25, y0 + 0.2), l)
    } else {
      box_((x, y0), (x + 0.5, y0 + 0.4), l, fill: t-cs)
    }
    if i < labels.len() - 1 {
      bezier((x + 0.42, y0 + 0.4), (x + 0.68, y0 + 0.4), (x + 0.48, y0 + 0.62), (x + 0.62, y0 + 0.62),
        mark: arrow, stroke: 0.6pt)
    }
  }
  content((1.3, y0 + 0.68), anchor: "south")[#pl[Inline] resumes: one chain]
  let xb = 2.95
  line((xb, y0 + 0.6), (xb, y1 - 0.15), stroke: brk)
  content((xb + 0.08, y0 + 0.62), anchor: "south-west", text(fill: c-verm)[break at $K$])
  box_((xb + 0.1, y0), (xb + 1.4, y0 + 0.4), [own queue], fill: t-by)
  bezier((xb - 0.25, y0), (xb + 0.35, y1 + 0.4), (xb - 0.25, y0 - 0.5), (xb + 0.35, y1 + 0.8), mark: arrow,
    stroke: 0.7pt)
  content((xb - 0.2, y0 - 0.5), anchor: "east", [#pl[Home] owner])
  box_((xb + 0.1, y1), (xb + 0.7, y1 + 0.4), $t_(K+1)$, fill: t-cs)
  content((xb + 1.0, y1 + 0.2), $dots$)
})

// (c) FC: a pass of at most H closures, remote wakes, yield after combining.
#let panel-c = canvas(length: 1cm, {
  import draw: *
  set-style(stroke: thin)
  let y0 = 1.3
  content((-0.08, y0 + 0.2), anchor: "east")[w0]
  line((0, y0 - 0.04), (4.9, y0 - 0.04), stroke: tl)
  let xs = ($r_c$, $r_1$, $r_2$, $dots$, $r_H$)
  for (i, l) in xs.enumerate() {
    let x = i * 0.45
    box_((x, y0), (x + 0.45, y0 + 0.4), l, fill: if l == $dots$ { white } else { t-fc })
  }
  content((1.12, y0 + 0.45), anchor: "south")[pass: $<= H$ closures]
  for i in (1, 2, 4) {
    line((i * 0.45 + 0.22, y0), (i * 0.45 + 0.22, 0.3), mark: arrow, stroke: 0.6pt)
  }
  rect((0, -0.05), (4.9, 0.3), radius: 0.04, fill: luma(245))
  content((3.6, 0.125), [injector])
  content((2.35, 0.95), anchor: "west", [#pl[Remote] wakes of])
  content((2.35, 0.62), anchor: "west", [served waiters])
  let xy = 2.3
  line((xy, y0 + 0.9), (xy, y0 - 0.1), stroke: brk)
  content((xy + 0.08, y0 + 0.45), anchor: "south-west", text(fill: c-verm)[yield after combine])
  box_((xy + 0.1, y0), (xy + 1.35, y0 + 0.4), [bystander], fill: t-by)
  box_((xy + 1.45, y0), (xy + 2.6, y0 + 0.4), [client $c$])
})

#let contract = {
  set text(size: 7.5pt)
  grid(
    columns: (auto, auto, auto),
    column-gutter: 0.2in,
    row-gutter: 4pt,
    align: (left + bottom),
    panel-a, panel-b, panel-c,
    [(a) Executor contract: wake placements], [(b) CES chain bound and break], [(c) FC pass, yield-after-combine],
  )
}
