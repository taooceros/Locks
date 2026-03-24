#import "@preview/touying:0.6.1": *
#import themes.simple: *
#import "@preview/cetz:0.3.4"
#import "@preview/fletcher:0.5.8" as fletcher: diagram, node, edge

// Advisor meeting presentation for the Locks repository
// Touying-based rewrite using the visual language from the provided sample.

// ── Theme configuration ─────────────────────────────────────────────
#let c-title = rgb("#1e3a5f")
#let c-accent = rgb("#2563eb")
#let c-head = rgb("#eff6ff")
#let c-row = rgb("#f8fafc")
#let c-blue = rgb("#f0f9ff")
#let c-orange = rgb("#fff7ed")
#let c-green = rgb("#f0fdf4")
#let c-mint = rgb("#ecfeff")
#let c-app = rgb("#dbeafe")
#let c-bench = rgb("#bfdbfe")
#let c-lock = rgb("#bbf7d0")
#let c-data = rgb("#fef3c7")

#show: simple-theme.with(
  aspect-ratio: "16-9",
  primary: c-accent,
  header: none,
  header-right: none,
  footer: [Locks repository overview],
  footer-right: context utils.slide-counter.display() + " / " + utils.last-slide-number,
  config-page(
    margin: (x: 52pt, y: 44pt),
    footer-descent: 0em,
  ),
  config-common(
    zero-margin-header: true,
  ),
)

#set text(font: "Latin Modern Sans", size: 17pt)
#set par(leading: 0.85em, spacing: 1.1em)
#set table(stroke: 0.4pt + luma(200), inset: 8pt)

// ── Helpers ───────────────────────────────────────────────────────
#let slide-title(body) = {
  block(width: 100%, below: 14pt, stroke: (bottom: 2.5pt + c-accent))[
    #text(size: 24pt, weight: "bold", fill: c-title)[#body]
    #v(4pt)
  ]
}

#let tbl-fill(x, y) = if y == 0 { c-head } else if calc.odd(y) { white } else { c-row }

#let callout(body) = block(
  width: 100%,
  radius: 5pt,
  inset: (x: 16pt, y: 11pt),
  fill: c-blue,
  stroke: (left: 3.5pt + c-accent),
  body,
)

#let note(body) = block(
  width: 100%,
  radius: 5pt,
  inset: (x: 16pt, y: 11pt),
  fill: c-orange,
  stroke: (left: 3.5pt + rgb("#f97316")),
  body,
)

#let ok(body) = block(
  width: 100%,
  radius: 5pt,
  inset: (x: 16pt, y: 11pt),
  fill: c-green,
  stroke: (left: 3.5pt + rgb("#22c55e")),
  body,
)

#let diagram(..args) = fletcher.diagram(node-corner-radius: 4pt, ..args)

#let stat-card(title, value, fill: c-blue) = block(
  width: 100%,
  radius: 5pt,
  inset: (x: 14pt, y: 12pt),
  fill: fill,
  stroke: 0.6pt + luma(205),
  [
    #text(size: 12pt, fill: luma(100))[#title]
    #v(4pt)
    #text(size: 24pt, weight: "bold", fill: c-title)[#value]
  ],
)

#let schematic(body) = block(
  width: 100%,
  radius: 5pt,
  inset: (x: 16pt, y: 12pt),
  fill: luma(247),
  stroke: (left: 3pt + c-accent),
  [
    #set text(font: "Latin Modern Mono", size: 12pt)
    #body
  ],
)

// ========================================================================
// TITLE
// ========================================================================
#title-slide[
  #align(center + horizon)[
    #text(size: 30pt, weight: "bold", fill: c-title)[Locks: Repository Overview]
    #v(1.2em)
    #text(size: 19pt)[Hongtao Zhang]
    #v(0.4em)
    #text(size: 14pt, fill: luma(120))[Mar 23, 2026 · Advisor meeting]
    #v(1.0em)
    #text(size: 16pt, fill: luma(90))[
      Usage-fair delegation locks: code, experiments, stored results, and next algorithmic direction
    ]
  ]
]

// ========================================================================
// AGENDA
// ========================================================================
#slide[
  #slide-title[Today's Agenda]
  #v(0.4em)
  #block(inset: (x: 16pt, y: 14pt), fill: luma(247), radius: 5pt, width: 88%)[
    #set text(size: 18pt)
    #set par(leading: 1.1em)
    + *What is in this repo* --- workspace structure and execution architecture
    + *What is already implemented* --- lock families, workloads, automation, and result pipeline
    + *What the repo already shows* --- fairness/locality findings, status gaps, and the March 23 plan
  ]
  #v(1em)
  #note[
    #text(size: 16pt)[*Repo snapshot in one line*: this is already a full research workbench, not just a lock library.]
    #v(0.3em)
    #text(
      size: 16pt,
    )[`dlock` runs experiments, `libdlock` implements algorithms, `c/` supplies reference baselines, `visualization/` stores data and plots, and `docs/`/`plan/` carry the paper story.]
    #v(0.3em)
    #text(
      size: 16pt,
    )[Main tension for the meeting: how much of the next milestone is paper-ready reruns versus an algorithm pivot from `FC-PQ` toward `FC-EW`?]
  ]
  #v(0.8em)
  #text(size: 15pt, fill: luma(130))[
    The deck is organized around what a new reader actually needs to understand the repository quickly.
  ]
]

// ========================================================================
// DELEGATION TEACHING SLIDES
// ========================================================================
#slide[
  #slide-title[What Is a Delegation Lock?]

  #callout[
    *Core idea*: waiters do not execute the critical section themselves. They publish a request, and one thread — the *combiner* — executes requests on behalf of everyone.
  ]

  #v(0.45em)

  #align(center)[
    #cetz.canvas(length: 1.15cm, {
      import cetz.draw: *

      let waiter-w = 2.1
      let waiter-h = 0.9
      let comb-w = 2.8
      let data-w = 3.4

      rect((0, 3.3), (waiter-w, 4.2), fill: c-app, stroke: 0.6pt + luma(150), radius: 4pt)
      rect((0, 1.95), (waiter-w, 2.85), fill: c-app, stroke: 0.6pt + luma(150), radius: 4pt)
      rect((0, 0.60), (waiter-w, 1.50), fill: c-app, stroke: 0.6pt + luma(150), radius: 4pt)
      content((1.05, 3.75), text(size: 10pt, weight: "bold")[Waiter A])
      content((1.05, 2.40), text(size: 10pt, weight: "bold")[Waiter B])
      content((1.05, 1.05), text(size: 10pt, weight: "bold")[Waiter C])

      rect((4.0, 1.7), (4.0 + comb-w, 3.1), fill: c-lock, stroke: 0.8pt + luma(140), radius: 4pt)
      content((5.4, 2.58), text(size: 12pt, weight: "bold")[Combiner])
      content((5.4, 2.08), text(size: 8.5pt, fill: luma(60))[picks next request and runs it])

      rect((8.3, 1.55), (8.3 + data-w, 3.25), fill: c-data, stroke: 0.8pt + luma(140), radius: 4pt)
      content((10.0, 2.58), text(size: 12pt, weight: "bold")[Shared object])
      content((10.0, 2.08), text(size: 8.5pt, fill: luma(60))[counter / queue / priority queue / map])

      for y in (3.75, 2.40, 1.05) {
        line(
          (waiter-w + 0.1, y),
          (3.85, 2.40),
          stroke: (paint: c-accent, thickness: 0.8pt),
          mark: (end: ">", fill: c-accent),
        )
      }

      line(
        (6.85, 2.40),
        (8.15, 2.40),
        stroke: (paint: luma(90), thickness: 1pt),
        mark: (end: ">", fill: luma(90)),
      )

      content((2.95, 3.15), text(size: 8.5pt, fill: c-accent)[publish request])
      content((7.45, 2.75), text(size: 8.5pt, fill: luma(75))[execute delegate])
    })
  ]

  #v(0.45em)

  - Each request carries *what operation to perform* and *its arguments*.
  - The combiner temporarily acts like a *scheduler* for critical sections.
  - Fairness is about *service order and cumulative usage*, not just who touched the lock first.
]

#slide[
  #slide-title[Why Delegation Can Make Fairness Cheaper]

  #text(size: 15pt, fill: luma(90))[
    Traditional locks transfer ownership of the critical section from thread to thread. Delegation transfers *requests* while keeping the shared data on one core for a while.
  ]

  #v(0.45em)

  #grid(
    columns: (1fr, 1fr),
    column-gutter: 18pt,
    [
      #text(size: 11pt, weight: "bold", fill: c-title)[Traditional lock]
      #v(0.2em)
      #diagram(
        node-stroke: 0.8pt,
        spacing: (17mm, 8mm),
        node((0,0), [*Core A* #linebreak() #text(size: 0.68em)[op 1 on `X`]], fill: c-app, shape: rect),
        node((1,0), [*Core B* #linebreak() #text(size: 0.68em)[op 2 on `X`]], fill: c-app, shape: rect),
        node((2,0), [*Core C* #linebreak() #text(size: 0.68em)[op 3 on `X`]], fill: c-app, shape: rect),
        edge((0,0), (1,0), "-}>", stroke: 1.4pt + rgb("#ef4444"), mark-scale: 71%),
        edge((1,0), (2,0), "-}>", stroke: 1.4pt + rgb("#ef4444"), mark-scale: 71%),
      )
      #v(0.15em)
      #text(size: 12.5pt, fill: rgb("#b91c1c"))[
        Each handoff may pull `X` to a new core.
      ]
    ],
    [
      #text(size: 11pt, weight: "bold", fill: c-title)[Delegation]
      #v(0.2em)
      #diagram(
        node-stroke: 0.8pt,
        spacing: (12mm, 7mm),
        node((0,0), [Req A], fill: c-app, shape: rect),
        node((0,1), [Req B], fill: c-app, shape: rect),
        node((0,2), [Req C], fill: c-app, shape: rect),
        node((1,1), [*Combiner* #linebreak() #text(size: 0.68em)[runs A, #linebreak() B, C]], fill: c-lock, shape: rect),
        node((2,1), [*Shared object `X`* #linebreak() #text(size: 0.68em)[reused #linebreak() locally]], fill: c-data, shape: rect),
        edge((0,0), (1,1), "-}>", stroke: 1.2pt + c-accent, mark-scale: 83%),
        edge((0,1), (1,1), "-}>", stroke: 1.2pt + c-accent, mark-scale: 83%),
        edge((0,2), (1,1), "-}>", stroke: 1.2pt + c-accent, mark-scale: 83%),
        edge((1,1), (2,1), "-}>", stroke: 1.4pt + rgb("#16a34a"), mark-scale: 71%),
      )
      #v(0.15em)
      #text(size: 12.5pt, fill: rgb("#166534"))[
        Requests move; the same core can keep touching `X`.
      ]
    ],
  )

  #v(0.5em)

  #table(
    columns: (1.25fr, 1.7fr, 1.7fr),
    fill: tbl-fill,
    table.header([*Question*], [*Traditional queue lock*], [*Delegation lock*]),
    [What moves?], [ownership of the critical section and often the shared cache lines], [requests move to the combiner while shared data can stay local],
    [Cost of fairness], [fair handoff often increases cross-core migration], [reordering inside the combiner can be mostly local],
    [What becomes schedulable?], [who acquires next], [which request the combiner serves next],
  )
]

#slide[
  #slide-title[How the Main Algorithms Differ]

  #callout[
    The key fairness signal in this project is *cumulative usage*: how much lock-holding time a thread has already received.
  ]

  #v(0.45em)

  #table(
    columns: (1.2fr, 1.9fr, 1.75fr),
    fill: tbl-fill,
    table.header([*Algorithm*], [*Scheduling rule*], [*Main tradeoff*]),
    [`FC`], [combiner scans a publication list and serves ready requests], [simple and fast, but heterogeneous critical sections can skew usage badly],
    [`CC` / `DSM`], [same delegation idea with different publication / synchronization structure], [alternative engineering points in the same design space],
    [`FC-Ban` / `CC-Ban`], [threads that consumed too much recent service are temporarily suppressed], [stronger fairness but can stop being work-conserving],
    [`FC-PQ`], [combiner chooses the smallest cumulative-usage waiter next], [cleanest fairness story, pays ordering overhead],
    [`Traditional fair baselines`], [MCS/CFL/Ticket/CLH enforce fairer lock handoff directly], [fairness often costs more because the shared data still migrates],
  )

  #v(0.55em)

  #note[
    The March 23 plan argues that `FC-PQ` should remain the theory anchor, but the next practical algorithm may be an eligibility-window scheduler (`FC-EW`) rather than another exact heap variant.
  ]
]

#slide[
  #slide-title[FC: Flat Combining, Phase 1]

  #callout[
    *Core mechanism*: `FC` gives each thread one *persistent publication node*.
    To request service, a thread writes its operation into its own node, then pushes that node onto the global `head` list with a CAS loop.
  ]

  #v(0.35em)

  #text(size: 14pt, weight: "bold", fill: c-title)[Step 1: each thread owns one persistent node]
  #v(0.1em)
  #align(center)[
    #diagram(
      node-stroke: 0.8pt,
      spacing: (14mm, 8mm),
      node((0,0), [Thread A #linebreak() #text(size: 0.68em)[owns node A]], fill: c-app, shape: rect),
      node((1,0), [Thread B #linebreak() #text(size: 0.68em)[owns node B]], fill: c-app, shape: rect),
      node((2,0), [Thread C #linebreak() #text(size: 0.68em)[owns node C]], fill: c-app, shape: rect),
    )
  ]

  #v(0.35em)
  #text(size: 14pt, weight: "bold", fill: c-title)[Step 2: publish by writing your own node, then CAS-push it at `head`]
  #v(0.1em)
  #align(center)[
    #diagram(
      node-stroke: 0.8pt,
      spacing: (10mm, 8mm),
      node((0,0), [Thread B #linebreak() #text(size: 0.68em)[write request]], fill: c-blue, shape: rect),
      node((1,0), [head], fill: luma(235), shape: rect),
      node((2,0), [node B #linebreak() #text(size: 0.68em)[ready]], fill: c-blue, shape: rect),
      node((3,0), [node A], fill: c-app, shape: rect),
      node((4,0), [node C], fill: c-app, shape: rect),
      edge((0,0), (2,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((1,0), (2,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((2,0), (3,0), "-}>", stroke: 0.9pt + luma(130), mark-scale: 111%),
      edge((3,0), (4,0), "-}>", stroke: 0.9pt + luma(130), mark-scale: 111%),
    )
  ]

  #v(0.45em)

  - The request lives in the caller's node; there is no per-request queue object.
  - The insertion primitive is a compare-and-swap on the shared `head` pointer.
  - This is the structural difference from `CC` before the combiner does any work.
]

#slide[
  #slide-title[FC: Flat Combining, Phase 2]

  #callout[
    *What the combiner pays for*: after publication, the combiner walks the whole publication list and executes the nodes that are currently ready.
    Idle nodes still stay in the structure, so they still get scanned.
  ]

  #v(0.35em)

  #text(size: 14pt, weight: "bold", fill: c-title)[Step 3: the combiner scans the whole list and executes ready nodes]
  #v(0.1em)
  #align(center)[
    #diagram(
      node-stroke: 0.8pt,
      spacing: (10mm, 8mm),
      node((0,0), [head], fill: luma(235), shape: rect),
      node((1,0), [node B #linebreak() #text(size: 0.68em)[ready]], fill: c-blue, shape: rect),
      node((2,0), [node A #linebreak() #text(size: 0.68em)[idle]], fill: luma(235), shape: rect),
      node((3,0), [node C #linebreak() #text(size: 0.68em)[ready]], fill: c-app, shape: rect),
      node((2,1), [*Combiner* #linebreak() #text(size: 0.68em)[scan all nodes]], fill: c-lock, shape: rect),
      node((4,1), [*Shared object*], fill: c-data, shape: rect),
      edge((0,0), (1,0), "-}>", stroke: 0.9pt + luma(130), mark-scale: 111%),
      edge((1,0), (2,0), "-}>", stroke: 0.9pt + luma(130), mark-scale: 111%),
      edge((2,0), (3,0), "-}>", stroke: 0.9pt + luma(130), mark-scale: 111%),
      edge((2,1), (1,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((2,1), (2,0), "-}>", stroke: 1.0pt + luma(170), mark-scale: 100%),
      edge((2,1), (3,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((2,1), (4,1), "-}>", stroke: 1.3pt + rgb("#16a34a"), mark-scale: 77%),
    )
  ]

  #v(0.45em)

  - Strength: publication is simple and a ready node is easy to serve.
  - Cost: the combiner scans a per-thread list, including idle nodes.
  - Clean contrast with `CC`: `FC` pays scan overhead; `CC` pays queueing overhead.
]

#slide[
  #slide-title[CC: CCSynch]

  #callout[
    *Fundamental idea*: `CC` is a *per-request queue*.
    Each request is appended by `tail.swap` rather than a head CAS, so the combiner walks only the active queue rather than scanning thread-owned slots.
  ]

  #v(0.35em)

  #align(center)[
    #diagram(
      node-stroke: 0.8pt,
      spacing: (10mm, 10mm),
      node((0,0), [dummy], fill: luma(235), shape: rect),
      node((1,0), [req A], fill: c-app, shape: rect),
      node((2,0), [req B], fill: c-app, shape: rect),
      node((3,0), [req C], fill: c-app, shape: rect),
      node((4,0), [new req], fill: c-blue, shape: rect),
      node((2,1), [*Combiner* #linebreak() #text(size: 0.66em)[drain active #linebreak() queue]], fill: c-lock, shape: rect),
      node((5,1), [*Shared object*], fill: c-data, shape: rect),
      edge((0,0), (1,0), "-}>", stroke: 0.9pt + luma(130), mark-scale: 111%),
      edge((1,0), (2,0), "-}>", stroke: 0.9pt + luma(130), mark-scale: 111%),
      edge((2,0), (3,0), "-}>", stroke: 0.9pt + luma(130), mark-scale: 111%),
      edge((3,0), (4,0), "-}>", stroke: 1.1pt + c-accent, mark-scale: 91%),
      edge((2,1), (1,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((2,1), (2,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((2,1), (3,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((2,1), (5,1), "-}>", stroke: 1.3pt + rgb("#16a34a"), mark-scale: 77%),
    )
  ]

  #v(0.2em)
  #text(size: 13pt, fill: luma(90))[
    Visual core: the structure contains *only active requests*. Appending a request extends the queue tail, and the combiner drains a consecutive active segment.
  ]

  #v(0.4em)

  - Strength: no need to scan idle thread slots.
  - Difference from `FC`: publication is per-request queueing, not per-thread persistent slots.
]

#slide[
  #slide-title[DSM]

  #callout[
    *Core mechanism*: each thread owns two local nodes and toggles between them, so a new request does not overwrite a node that may still be linked in the queue.
  ]

  #v(0.35em)

  #align(center)[
    #diagram(
      node-stroke: 0.8pt,
      spacing: (10mm, 9mm),
      node((0,0), [A0], fill: c-app, shape: rect),
      node((1,0), [A1], fill: c-blue, shape: rect),
      node((0,1), [B0], fill: c-blue, shape: rect),
      node((1,1), [B1], fill: c-app, shape: rect),
      node((3,0.5), [*toggle between* #linebreak() *local nodes*], fill: c-bench, shape: rect),
      node((5,0.5), [queue uses #linebreak() A1 -> B0 -> ...], fill: luma(235), shape: rect),
      node((7,0.5), [*Combiner*], fill: c-lock, shape: rect),
      node((9,0.5), [*Shared object*], fill: c-data, shape: rect),
      edge((0,0), (1,0), "<->", stroke: 0.9pt + c-accent, mark-scale: 111%),
      edge((0,1), (1,1), "<->", stroke: 0.9pt + c-accent, mark-scale: 111%),
      edge((1,0), (3,0.5), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((0,1), (3,0.5), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((3,0.5), (5,0.5), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((5,0.5), (7,0.5), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((7,0.5), (9,0.5), "-}>", stroke: 1.3pt + rgb("#16a34a"), mark-scale: 77%),
    )
  ]

  #v(0.2em)
  #text(size: 13pt, fill: luma(90))[
    Visual core: each thread alternates between two local nodes, so a new request can be published without clobbering the node already linked in the queue.
  ]

  #v(0.4em)

  - The double-buffering is the distinctive idea here.
  - Like FC and CC, it keeps delegation locality but does not solve usage fairness by itself.
]

#slide[
  #slide-title[FC-Ban]

  #callout[
    *Core mechanism*: start from FC’s linked-list scan, but each node carries a `banned_until` timestamp; the combiner skips nodes whose penalty window has not expired.
  ]

  #v(0.35em)

  #align(center)[
    #diagram(
      node-stroke: 0.8pt,
      spacing: (12mm, 10mm),
      node((0,0), [A #linebreak() #text(size: 0.68em)[ready]], fill: c-app, shape: rect),
      node((1,0), [B #linebreak() #text(size: 0.68em)[banned]], fill: luma(230), shape: rect),
      node((2,0), [C #linebreak() #text(size: 0.68em)[ready]], fill: c-app, shape: rect),
      node((1,1), [*Combiner* #linebreak() #text(size: 0.68em)[scan + skip]], fill: c-lock, shape: rect),
      node((3,1), [*Shared object*], fill: c-data, shape: rect),
      node((1,2), [ban timer], fill: c-orange, shape: rect),
      edge((0,0), (1,0), "-}>", stroke: 0.9pt + luma(130), mark-scale: 111%),
      edge((1,0), (2,0), "-}>", stroke: 0.9pt + luma(130), mark-scale: 111%),
      edge((1,1), (0,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((1,1), (2,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((1,0), (1,2), "-}>", stroke: 1.0pt + rgb("#f97316"), mark-scale: 100%),
      edge((1,1), (3,1), "-}>", stroke: 1.3pt + rgb("#16a34a"), mark-scale: 77%),
    )
  ]

  #v(0.2em)
  #text(size: 13pt, fill: luma(90))[
    Visual core: same FC list scan, but the combiner refuses to serve a node while its `banned_until` window is still active.
  ]

  #v(0.4em)

  - Benefit: strong correction when one thread has already consumed too much service.
  - Cost: can stop being work-conserving because a ready thread may be forced to wait out its ban.
]

#slide[
  #slide-title[CC-Ban]

  #callout[
    *Fundamental idea*: `CC-Ban` is `CC + cooldown`.
    Keep the same queue discipline as `CC`, but add a `banned_until` gate before a thread can enqueue again, and write back a fresh penalty after service.
  ]

  #v(0.35em)

  #align(center)[
    #diagram(
      node-stroke: 0.8pt,
      spacing: (12mm, 9mm),
      node((0,0), [thread], fill: c-app, shape: rect),
      node((1,0), [ban gate], fill: c-orange, shape: rect),
      node((2,0), [same CC #linebreak() queue], fill: c-bench, shape: rect),
      node((3,0), [*Combiner*], fill: c-lock, shape: rect),
      node((4,0), [*Shared object*], fill: c-data, shape: rect),
      node((3,1), [write #linebreak() penalty back], fill: luma(235), shape: rect),
      edge((0,0), (1,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((1,0), (2,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((2,0), (3,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((3,0), (4,0), "-}>", stroke: 1.3pt + rgb("#16a34a"), mark-scale: 77%),
      edge((3,0), (3,1), "-}>", stroke: 1.0pt + rgb("#f97316"), mark-scale: 100%),
      edge((3,1), (1,0), "-}>", stroke: 1.0pt + rgb("#f97316"), mark-scale: 100%),
    )
  ]

  #v(0.2em)
  #text(size: 13pt, fill: luma(90))[
    Visual core: do ordinary `CC` queueing, but only after passing the cooldown gate; after service, the combiner increases that thread's ban window.
  ]

  #v(0.4em)

  - The queue semantics are inherited from `CC`; the fairness add-on is the cooldown gate.
  - Useful as a strict fairness reference point, even if it sacrifices some throughput.
]

#slide[
  #slide-title[FC-PQ]

  #callout[
    *Core mechanism*: drain new waiters into a priority queue keyed by cumulative usage, pop the minimum-usage node, execute it, increase its usage, and reinsert it.
  ]

  #v(0.35em)

  #align(center)[
    #diagram(
      node-stroke: 0.8pt,
      spacing: (12mm, 9mm),
      node((0,1), [A:3], fill: c-app, shape: rect),
      node((1,0), [PQ], fill: c-bench, shape: rect),
      node((1,1), [C:7], fill: c-app, shape: rect),
      node((1,2), [B:11], fill: c-app, shape: rect),
      node((2,1), [*Combiner*], fill: c-lock, shape: rect),
      node((3,1), [*Shared object*], fill: c-data, shape: rect),
      node((4,1), [A:9 #linebreak() #text(size: 0.68em)[reinsert]], fill: c-blue, shape: rect),
      edge((0,1), (1,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((1,0), (2,1), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((2,1), (3,1), "-}>", stroke: 1.3pt + rgb("#16a34a"), mark-scale: 77%),
      edge((3,1), (4,1), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
    )
  ]

  #v(0.2em)
  #text(size: 13pt, fill: luma(90))[
    Visual core: always serve the smallest-usage node, then measure its service time and reinsert it with a higher usage value.
  ]

  #v(0.4em)

  - This is the cleanest fairness story in the repo: the combiner is explicitly acting as a min-usage scheduler.
  - The research bet is that this ordering cost is still cheaper than fair handoff in traditional locks.
]

#slide[
  #slide-title[FC-SL]

  #callout[
    *Core mechanism*: keep active nodes in a skiplist-style ordered set keyed by cumulative usage; the combiner repeatedly pops the front (minimum) element.
  ]

  #v(0.35em)

  #align(center)[
    #diagram(
      node-stroke: 0.8pt,
      spacing: (12mm, 8mm),
      node((0,0), [A:3], fill: c-app, shape: rect),
      node((1,0), [C:7], fill: c-app, shape: rect),
      node((2,0), [B:11], fill: c-app, shape: rect),
      node((0,1), [A:3], fill: c-app, shape: rect),
      node((2,1), [B:11], fill: c-app, shape: rect),
      node((1,2), [*Combiner* #linebreak() #text(size: 0.68em)[pop front]], fill: c-lock, shape: rect),
      node((3,1), [*Shared object*], fill: c-data, shape: rect),
      edge((0,0), (1,0), "-}>", stroke: 0.9pt + luma(130), mark-scale: 111%),
      edge((1,0), (2,0), "-}>", stroke: 0.9pt + luma(130), mark-scale: 111%),
      edge((0,1), (2,1), "-}>", stroke: 0.9pt + luma(160), mark-scale: 111%),
      edge((1,2), (0,0), "-}>", stroke: 1.0pt + c-accent, mark-scale: 100%),
      edge((1,2), (3,1), "-}>", stroke: 1.3pt + rgb("#16a34a"), mark-scale: 77%),
    )
  ]

  #v(0.2em)
  #text(size: 13pt, fill: luma(90))[
    Visual core: maintain an ordered set by usage and repeatedly remove the front (minimum) element; conceptually similar policy to FC-PQ with a different data structure.
  ]

  #v(0.4em)

  - It belongs to the same “fair delegation by ordered service” idea as FC-PQ.
  - The difference is the data structure used to maintain order, not the core locality argument.
]

#slide[
  #slide-title[Workload Matrix]

  #table(
    columns: (1.35fr, 2.4fr),
    fill: tbl-fill,
    table.header([*Workload*], [*What it teaches*]),
    [`counter-proportional`], [heterogeneous critical section ratios, non-CS sweep, fairness and throughput],
    [`counter-array`], [data-footprint and locality experiments with sequential vs random access],
    [`fetch-and-multiply`], [tiny critical sections, optional lock-free baseline],
    [`queue`], [shared queue throughput and response-time studies],
    [`priority-queue`], [shared priority queue benchmarks],
    [`hash-map`], [mixed get/put/scan workload with skew and scanner threads],
  )

  #v(0.45em)
  #text(size: 15pt, fill: luma(90))[
    Common knobs: `--threads`, `--cpus`, `--duration`, `--warmup`, `--trials`, `--stat-response-time`, and workload-specific parameters like `--array-size`, `--random-access`, `--scan-size`, and `--zipf-theta`.
  ]
]

// ========================================================================
// DATA + AUTOMATION
// ========================================================================
#slide[
  #slide-title[Automation and Measurement]

  #table(
    columns: (1.1fr, 2.6fr),
    fill: tbl-fill,
    table.header([*Artifact*], [*Purpose*]),
    [`justfile`], [common build/run shortcuts for DLock1 and DLock2],
    [`experiment.nu`],
    [grouped experiment orchestration: ratio sweep, crossover, Pareto, combiner study, perf, delegation-vs-ShflLock],

    [`profile.nu`], [`perf stat` sweeps over lock variants],
    [`BUILD.md`], [nightly/x86_64 build and test reference],
    [`src/benchmark/README.md`], [execution flow and record schema],
  )

  #v(0.55em)
  #ok[
    Per-thread records already include throughput, hold time, combine time, JFI, normalized share, and optional combiner/waiter latency vectors.
  ]
]

#slide[
  #slide-title[Data and Reporting Assets]

  - *Output corpus*: 36 top-level result directories under `visualization/output/`
  - *Stored runs*: 918 Arrow files already in-tree
  - *Python analysis*: `plot_pareto.py`, `read_arrow_tables.py`, `analyze_combiner.py`, `summarize_latency.py`, `throughput_table.py`, and related scripts
  - *Writing artifacts*: Typst proposal, report, earlier meeting notes, and this new presentation

  #v(0.7em)

  #callout[
    The repo is set up to go from algorithm code to paper figure without leaving the tree: run benchmarks, emit Arrow, aggregate with Python, and present with Typst.
  ]
]

// ========================================================================
// RESULTS
// ========================================================================
#slide[
  #slide-title[What the Existing Repo Results Already Say]

  #text(size: 14pt, fill: luma(110))[
    These numbers come from `docs/EXPERIMENT_RESULTS_DEMO.md` and the experiment-plan narrative already checked into the repository.
  ]

  #v(0.5em)

  #table(
    columns: (2.1fr, 1.2fr, 3fr),
    fill: tbl-fill,
    table.header([*Observation*], [*Representative value*], [*Interpretation*]),
    [`FC_PQ_BHeap fairness`],
    [`0.9982 JFI`],
    [at 64 threads and 1:100 CS ratio in the demo table, showing that fair delegation can stay near unit fairness],

    [`CCBan fairness`],
    [`> 0.99 JFI`],
    [across the reported 64-thread ratio sweep, making it the fairness champion in the checked-in summary],

    [`Mutex collapse`],
    [`0.016 JFI`],
    [at 64 threads and 1:100 ratio, showing how badly traditional unfair serialization can skew usage],

    [`FC / MCS locality gap`],
    [`2.2×`],
    [at 32 KiB random counter-array footprint, reinforcing the data-migration story],

    [`USCL tradeoff`],
    [`fair but slower`],
    [docs describe it as consistently fair but roughly 3–5× below delegation throughput],
  )

  #v(0.6em)

  #callout[
    The repo's intended paper claim is already visible in the stored material: the cost of adding fairness to delegation should be much smaller than the cost of adding fairness to queue locks.
  ]
]

// ========================================================================
// STATUS
// ========================================================================
#slide[
  #slide-title[What Looks Done]

  - `TODO.md` marks MCS, CFL, Ticket, CLH, response-time CDF export, newcomer initialization, starvation counters, and prefetching as completed.
  - `experiment.nu` already scripts the major paper-shaping groups: benefits, tradeoff, combiner study, factor analysis, and perf validation.
  - `visualization/output/` already contains enough breadth to synthesize an interim story before running new jobs.

  #v(0.7em)

  #ok[
    The repository has already crossed from “prototype implementation” into “evaluation platform with paper infrastructure.”
  ]
]

#slide[
  #slide-title[What Still Needs Reconciliation]

  - `STATUS_REPORT.md` is older and still flags waiting-thread accounting problems in `FC-Ban` and `CC-Ban`.
  - Several final reruns in `TODO.md` remain unchecked, especially long-duration paper-ready collections.
  - Environment capture and result hygiene are still weaker than the algorithm work.

  #v(0.7em)

  #note[
    Practical issue for the meeting: the code, the stored results, and the status documents are no longer perfectly synchronized. Before a paper push, the repo needs one documentation pass that says exactly what is true today.
  ]
]

// ========================================================================
// NEXT DIRECTION
// ========================================================================
#slide[
  #slide-title[March 23 Direction: Keep FC-PQ, Consider FC-EW]

  #callout[
    *Recommendation in `plan/2026-03-23/algorithm-improvement-plan.md`*: keep `FC-PQ` as the correctness and theory anchor, but stop spending time on more heap variants and consider `FC-EW` as the practical next algorithm.
  ]

  #v(0.6em)

  - *Why change?* Exact min-usage scheduling is analytically clean, but it pays ordering cost even when many waiters are already “fair enough.”
  - *FC-EW idea*: replace exact global min-ordering with an eligibility window, so the combiner chooses from waiters whose cumulative usage is within `delta` of the current minimum.
  - *Expected benefit*: preserve strong usage-fairness while reducing combiner-side scheduling overhead and giving more room to address combiner asymmetry.

  #v(0.8em)

  #note[
    Immediate decision point: do we spend the next iteration finishing the paper-ready FC-PQ story, or pivot early and make `FC-EW` the centerpiece before the evaluation is locked in?
  ]
]

// ========================================================================
// CLOSING
// ========================================================================
#slide[
  #align(center + horizon)[
    #text(size: 28pt, weight: "bold", fill: c-title)[
      This repo already contains the paper's spine.
    ]

    #v(0.9em)

    #text(size: 19pt)[
      Algorithms, baselines, workloads, automation, stored results,
      analysis scripts, and writing artifacts are all here.
    ]

    #v(1.2em)

    #text(size: 16pt, fill: luma(95))[
      The main remaining work is not “build the project” — it is choosing and tightening the final research story.
    ]
  ]
]
