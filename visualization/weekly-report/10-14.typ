#import "@preview/touying:0.5.2": *
#import "@preview/codly:1.0.0": *
#show: codly-init.with()

#import themes.dewdrop: *

#show: dewdrop-theme.with(aspect-ratio: "16-9", navigation: none)

= Profiling Result

== Flat Combining

#let fc-profile = csv("fc-profile-1.csv")

#[
  #set text(size: 20pt, hyphenate: false)
  #set par(linebreaks: "optimized")

  #table(
    columns: (auto, auto, auto, auto, auto, auto),
    inset: 5pt,
    align: center,
    stroke: none,
    table.hline(),
    [*Function*], [*CPU Time*], [*Clockticks*], [*Instructions Retired*], [*CPI Rate*], [*Module*], 
    table.hline(),
    [lock], [44.490s], [$1.11 times 10^11$], [$1.30 times 10^9$], [85.489], [dlock],
    [bench code], [4.682s], [$1.17 times 10^10$], [$1.27 times 10^10$], [0.918], [dlock],
    [[vmlinux]], [0.069s], [$1.49 times 10^8$], [$6.27 times 10^7$], [2.368], [vmlinux],
    [thread::current], [0.037s], [$9.68 times 10^7$], [$1.98 times 10^7$], [4.889], [dlock],
    table.hline(),
  )

  #v(1em)

  #table(
    columns: (auto, auto, auto, auto, auto),
    inset: 5pt,
    align: center,
    stroke: none,
    table.hline(),
    [*Function*], [*Retiring*], [*Front-End Bound*], [*Bad Speculation*], [*Back-End Bound*],
    table.hline(),
    [lock], [0.90%], [0.10%], [0.00%], [99.10%],
    [bench code], [30.90%], [0.30%], [0.80%], [68.00%],
    [[vmlinux]], [34.30%], [57.20%], [11.40%], [0.00%],
    [thread::current], [87.80%], [100.00%], [0.00%], [0.00%],
    table.hline(),
  )
]


== CCSynch

== FC-PQ-BHeap


== Mutex

= Code Change

== Shared Counter

+ Remove `blackbox` for accessing the data
+ Change the blackbox position

```diff
- while black_box(loop_limit) > 0 {
-   *data += 1;
- }
+ while loop_limit > 0 {
+   *black_box(&mut *data) += 1;
+   loop_limit -= 1;
+ }
```

#pagebreak()

=== Reason for `blackbox`

+ `loop_limit` => the length of *Critical Section*.
+ Compiler will optimize the code to something like `*data += loop_limit;`, which will make varying the `loop_limit` not affecting the length of *Critical Section*.

=== Reason for the change

+ I want to mimic more access to the shared variable (hopefully something like `inc (rax)` in assembly).
+ The previous version contains too much overhead for doing the loop.