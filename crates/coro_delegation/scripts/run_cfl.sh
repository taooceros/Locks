#!/usr/bin/env bash
# async-CFL study (FINDINGS 2026-09-30): one frozen binary, one window.
# Stages (in this order unless STAGES is set), all via scripts/run_p3.sh,
# 3 repeats each (repeats outermost inside a stage), JSONs to results/:
#   sanity  coro-bench --sanity (every lock, incl. cfl)
#   main    W=8 b31 sus+bur: references (dispatch, both fair dispatch-pq
#           candidates, ces-k64-home, fcpq-h16-home-c16) and the cfl variants
#           {shuffle usage, off} x {pre-wake on, off} at clamp 256, clamp
#           {64, 0}, remote wakes, overlap-only scan, spin 2000
#   w16     W=16 b31 sus: best cfl (+ remote) beside the references
#   b0      W=8 b0 sus: same six
#   inst    W=8 b31 sus+bur with --handoff-stats (prefix acfli)
#   yield   W=8 b31 sus+bur with PARALLEL_MODE=yield (one yield_now() before
#           the client's parallel spin; REVIEW-2026-09-30 I1): references and
#           cfl / cfl-nopre / cfl-remote
#   yinst   the yield cells of cfl, cfl-nopre, cfl-remote and
#           dispatch-pq-home-c256 with --handoff-stats (prefix acfli)
# Run under the measurement lock, e.g.
#   flock ~/.cache/locks-experiments/measurement.lock \
#     crates/coro_delegation/scripts/run_cfl.sh
# Env: BIN (default target/cfl/coro-bench, the frozen binary; build one with
# .worktree/cfl/snapshot.sh and copy it there), STAGES,
# OUT (default crates/coro_delegation/results).
set -euo pipefail
cd "$(dirname "$0")/../../.."
BIN=$(realpath "${BIN:-target/cfl/coro-bench}")
STAGES=${STAGES:-sanity main w16 b0 inst yield yinst}
OUT=$(realpath -m "${OUT:-crates/coro_delegation/results}")
# Private copy: bash reads a script while running it, so a concurrent edit
# of run_p3.sh would otherwise corrupt a stage in flight. The copy cd's
# elsewhere, hence the absolute BIN and OUT.
P3=$(mktemp --suffix=-run_p3.sh)
cp crates/coro_delegation/scripts/run_p3.sh "$P3" && chmod +x "$P3"
trap 'rm -f "$P3"' EXIT
export SKIP_BUILD=1 OUT

REFS="dispatch dispatch-pq-home-c256 dispatch-pq-remote-c256 ces-k64-home fcpq-h16-home-c16"
CFL="cfl cfl-noshfl cfl-nopre cfl-noshfl-nopre cfl-c64 cfl-c0 cfl-remote cfl-ovl cfl-spin2000"
CONFIRM="dispatch dispatch-pq-home-c256 dispatch-pq-remote-c256 fcpq-h16-home-c16 cfl cfl-remote"
INST="$CFL dispatch-pq-home-c256 dispatch-pq-remote-c256"
YIELD="dispatch dispatch-pq-home-c256 dispatch-pq-remote-c256 fcpq-h16-home-c16 cfl cfl-nopre cfl-remote"
YINST="cfl cfl-nopre cfl-remote dispatch-pq-home-c256"

echo "bin $(sha256sum "$BIN")"
for s in $STAGES; do
  echo "stage $s start $(date +%s)"
  case $s in
    sanity) "$BIN" --sanity ;;
    main) BIN=$BIN PREFIX=acfl WORKERS=8 BALANCES=31 VARIANTS="$REFS $CFL" "$P3" ;;
    w16) BIN=$BIN PREFIX=acfl WORKERS=16 BALANCES=31 CONTENTIONS=sus VARIANTS=$CONFIRM "$P3" ;;
    b0) BIN=$BIN PREFIX=acfl WORKERS=8 BALANCES=0 CONTENTIONS=sus VARIANTS=$CONFIRM "$P3" ;;
    inst) BIN=$BIN PREFIX=acfli WORKERS=8 BALANCES=31 HANDOFF_STATS=1 VARIANTS=$INST "$P3" ;;
    yield) BIN=$BIN PREFIX=acfl WORKERS=8 BALANCES=31 PARALLEL_MODE=yield VARIANTS=$YIELD "$P3" ;;
    yinst) BIN=$BIN PREFIX=acfli WORKERS=8 BALANCES=31 PARALLEL_MODE=yield HANDOFF_STATS=1 \
             VARIANTS=$YINST "$P3" ;;
    *) echo "unknown stage $s" >&2; exit 2 ;;
  esac
  echo "stage $s end $(date +%s)"
done
