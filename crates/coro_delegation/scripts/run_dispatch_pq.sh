#!/usr/bin/env bash
# dispatch-pq kill test (FINDINGS 2026-09-29): one frozen binary, one window.
# Stages (run in this order unless STAGES is set), all via scripts/run_p3.sh,
# 3 repeats each (repeats outermost inside a stage), JSONs to results/:
#   sanity  coro-bench --sanity (every lock, incl. dispatch-pq)
#   main    W=8 b31 sus+bur: the requested cells plus the clamp extras
#           (c0 = off, c256 = fcpq-h16-home-c16's bound of 16 passes x 16 ops)
#   w16     W=16 b31 sus: dispatch, fair dispatch-pq candidates, fcpq-h16-home-c16
#   b0      W=8 b0 sus: same four
#   inst    W=8 b31 sus+bur with --handoff-stats / --fcpq-wait-stats (prefix dpqi)
#   ab      fcpq-h16-home-c16 from the pre-change sweep binary (prefix dpqab-pre):
#           guards the shared-queue refactor of fc_pq.rs
# Run under the measurement lock, e.g.
#   flock ~/.cache/locks-experiments/measurement.lock \
#     crates/coro_delegation/scripts/run_dispatch_pq.sh
# Env: BIN (default target/dispatch-pq/coro-bench), PRE_BIN (default
# target/fcpq-sweep/coro-bench), STAGES.
set -euo pipefail
cd "$(dirname "$0")/../../.."
BIN=${BIN:-target/dispatch-pq/coro-bench}
PRE_BIN=${PRE_BIN:-target/fcpq-sweep/coro-bench}
STAGES=${STAGES:-sanity main w16 b0 inst ab}
P3=crates/coro_delegation/scripts/run_p3.sh
export SKIP_BUILD=1

MAIN="dispatch dispatch-pq dispatch-pq-c8 dispatch-pq-remote dispatch-pq-home \
dispatch-pq-c0 dispatch-pq-c256 dispatch-pq-home-c256 dispatch-pq-remote-c256 \
fc-remote fcpq-h16-home-c16"
FAIR="dispatch dispatch-pq-home-c256 dispatch-pq-remote-c256 fcpq-h16-home-c16"
INST="dispatch-pq dispatch-pq-c8 dispatch-pq-remote dispatch-pq-home dispatch-pq-c0 \
dispatch-pq-c256 dispatch-pq-home-c256 dispatch-pq-remote-c256 fcpq-h16-home-c16"

echo "bin $(sha256sum "$BIN")"
for s in $STAGES; do
  echo "stage $s start $(date +%s)"
  case $s in
    sanity) "$BIN" --sanity ;;
    main) BIN=$BIN PREFIX=dpq WORKERS=8 BALANCES=31 VARIANTS=$MAIN "$P3" ;;
    w16) BIN=$BIN PREFIX=dpq WORKERS=16 BALANCES=31 CONTENTIONS=sus VARIANTS=$FAIR "$P3" ;;
    b0) BIN=$BIN PREFIX=dpq WORKERS=8 BALANCES=0 CONTENTIONS=sus VARIANTS=$FAIR "$P3" ;;
    inst) BIN=$BIN PREFIX=dpqi WORKERS=8 BALANCES=31 FCPQ_WAIT_STATS=1 HANDOFF_STATS=1 \
            VARIANTS=$INST "$P3" ;;
    ab) echo "pre-bin $(sha256sum "$PRE_BIN")"
        BIN=$PRE_BIN PREFIX=dpqab-pre WORKERS=8 BALANCES=31 VARIANTS=fcpq-h16-home-c16 "$P3" ;;
    *) echo "unknown stage $s" >&2; exit 2 ;;
  esac
  echo "stage $s end $(date +%s)"
done
