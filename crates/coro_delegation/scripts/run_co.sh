#!/usr/bin/env bash
# Coroutine-style mutex matrix (FINDINGS 2026-09-30), one frozen binary, run
# under the shared measurement lock, repeats outermost and parallel modes
# interleaved inside each repeat (labels: scripts/run_p3.sh).
#   BIN=<frozen coro-bench> scripts/run_co.sh
#       main: W=8, heavy 8, b31, sus + bur, spin + yield, MAIN variants
#   BIN=... PHASE=confirm CO_FIFO=<label> CO_PQ=<label> scripts/run_co.sh
#       W=16 b31 sus and W=8 b0 sus, spin + yield, the two labels plus
#       fcpq-h16-home-c16
# Env: PREFIX (default co1), OUT (default crates/coro_delegation/results),
# REPEATS (default "1 2 3"), LOCKFILE.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
: "${BIN:?set BIN to the frozen coro-bench binary}"
LOCKFILE=${LOCKFILE:-$HOME/.cache/locks-experiments/measurement.lock}
if [ -z "${UNDER_FLOCK:-}" ]; then
  exec env UNDER_FLOCK=1 flock "$LOCKFILE" "$0" "$@"
fi
export BIN SKIP_BUILD=1 FCPQ_WAIT_STATS=1
export PREFIX=${PREFIX:-co1} OUT=${OUT:-crates/coro_delegation/results}
REPEATS=${REPEATS:-1 2 3}
MAIN="dispatch ces-k64-home fc-remote fcpq-h16-home-c16 dispatch-pq-home-c256 \
co-fifo-sremote-k64-home co-fifo-shome-k64-home co-fifo-snone-k64-home \
co-pq-sremote-k64-home-c256 co-pq-shome-k64-home-c256 co-pq-snone-k64-home-c256 \
co-pq-sremote-k64-home-c64 co-pq-shome-k64-home-c64 co-pq-snone-k64-home-c64 \
co-pq-sremote-k64-home-c0 co-pq-shome-k64-home-c0 co-pq-snone-k64-home-c0"
echo "binary $(sha256sum "$BIN")"
for r in $REPEATS; do
  for pm in spin yield; do
    case ${PHASE:-main} in
      main)
        WORKERS=8 BALANCES=31 CONTENTIONS="sus bur" VARIANTS="$MAIN" \
          PARALLEL_MODE=$pm REPEATS=$r "$here/run_p3.sh" ;;
      confirm)
        : "${CO_FIFO:?}" "${CO_PQ:?}"
        v="$CO_FIFO $CO_PQ fcpq-h16-home-c16"
        WORKERS=16 BALANCES=31 CONTENTIONS=sus VARIANTS="$v" \
          PARALLEL_MODE=$pm REPEATS=$r "$here/run_p3.sh"
        WORKERS=8 BALANCES=0 CONTENTIONS=sus VARIANTS="$v" \
          PARALLEL_MODE=$pm REPEATS=$r "$here/run_p3.sh" ;;
      *) echo "unknown PHASE ${PHASE}" >&2; exit 2 ;;
    esac
  done
done
echo "co matrix (${PHASE:-main}) complete"
