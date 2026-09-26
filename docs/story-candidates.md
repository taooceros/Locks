# Story candidates (undecided, 2026-09-26)

Two narratives are retained. `README.md` currently states B in its thesis
paragraph because B has the formal model behind it; this is not a decision.
Both share the same evidence base (`docs/evidence/README.md`) and the same
experiments E0-E4; they differ in what the headline claim is and which figure
leads.

## A. Fair service at a price

**Claim.** "Who gets served" and "how fast the lock runs" are different
problems; existing locks conflate them. Delegation lets the scheduler see the
whole waiting set and charge each requester its actual service cost, so
service can be redistributed deliberately, at a visible and priced cost.

**Lead figure.** redb 1/64 write mix: FC -> FC-PQ service Jain 0.891 -> 0.992,
small-transaction share 32.6% -> 45.4%, small-transaction throughput 1.50x,
total records 0.832x, small-transaction p99 bucket up. Turn-fair MCS stays at
Jain 0.907.

**Arc.** Turn fairness is not service fairness (MCS numbers) -> U-SCL fixes
service fairness by banning (H2 reservation wait, multi-table collapse) ->
delegation + usage-ordered selection -> the price (records/s, CPU, p99) ->
negative controls where there is no imbalance to fix (pure read 0.397x,
split-32 losses) -> open failure (UpScaleDB batch8, Jain 0.628).

**Strength.** Every number already exists; honest about cost.
**Weakness.** The headline (usage-fair scheduling redistributes service) is
SCL's and CFL's contribution; the mechanism's advantage is not explained.

## B. Two sequences: fairness by switching threads moves data

**Claim.** A lock is a service sequence and an executor sequence.
Caller-executed locks tie them: choosing who is served next chooses where the
protected working set lives next. They must migrate every operation (MCS,
ticket, CLH, service-fair CFL), bias selection toward locality and give up
service share (CNA, ShflLock, CFL NUMA grouping), or idle with backlog
(SCL/U-SCL). Delegation removes the executor from the selection; the policy is
free and work-conserving. Cost model: delegation wins iff
b(M + h - d) > M + A + K; no batch wins if d >= h + M. Advantage grows with
D/CS and batch occupancy b.

**Lead figure.** E1 (not yet run): throughput vs protected working set W,
caller-executed locks sloping down, FC/FC-PQ flat, crossover W*, Jain
annotated; same-socket and cross-socket.

**Arc.** Two sequences -> the three options for caller-executed locks (CFL's
NUMA grouping and SCL's bans as evidence of the tension) -> delegation as the
decoupling -> cost model and predicted regimes -> E1 -> DB confirmation ->
negative controls as predicted small-D/low-b regimes.

**Strength.** Structural claim about the design space; predicts its own
negative results; formal backing in `analysis/logp/`.
**Weakness.** Load-bearing figure does not exist yet. Kill risk: real CFL with
same-socket handoff on an LLC-resident working set may show small M, in which
case B degrades to work-conservation vs U-SCL only.

## Relationship

A is the "what fairness costs" section inside B, and B is the "why delegation"
section inside A. The decision depends on E1: if W* exists and is robust across
sockets, lead with B and use redb 1/64 as motivation; if not, lead with A and
present B's model as explanatory scope.
