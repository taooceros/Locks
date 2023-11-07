#import "../shortcut.typ": *


Similar to #fc, RCL also maintains a list of nodes owned by
each thread. The primary distinction is that RCL designates
a specific thread to act as the server (combiner).
Consequently, it's akin to a remote procedure call, as
proposed in the original paper. Notably, RCL utilizes an
array of nodes, whereas #fc
uses a linked list of nodes. This design difference in RCL
should yield improved performance when the number of
inactive threads is relatively small.

One of the primary goals of RCL is to facilitate easy
migration of legacy code. To achieve this, RCL employs a
complex algorithm to ensure that the server thread won't be
blocked or engaged in active spinning, which could
negatively impact performance. Additionally, RCL's design
enables the server to handle multiple locks, allowing for
straightforward porting of legacy code. While our
implementation doesn't cover the complete version of RCL and
thus won't delve into these aspects, one core goal of
enabling one server to manage various types of locks is
retained. Achieving this presents a significant challenge
within the Rust type system, as we aim to statically
dispatch type information to prevent performance regression.
Further implementation details can be found in the appendix
labeled @rcl_detail_impl.