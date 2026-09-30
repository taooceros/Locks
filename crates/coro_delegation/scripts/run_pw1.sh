#!/usr/bin/env bash
# Same-window matrix for the paper rewrite (FINDINGS 2026-09-30 "same-window
# matrix: ordinary-waker co-pq, spin vs yield"). One frozen coro-bench and one
# frozen tokio-bench, run under the shared measurement lock; repeats
# outermost, spin and yield interleaved inside each repeat, and within one
# (repeat, mode) the W=8 main cells, the W=16 confirmation cells and the
# tokio cells back to back, so every compared cell comes from one window.
#   BIN=<frozen coro-bench> TOKIO_BIN=<frozen tokio-bench> scripts/run_pw1.sh
# Labels: scripts/run_p3.sh. Tokio files:
#   $PREFIX-tokio-<lock>[-pyield]-w8-h8-<sus|bur>-r<i>.json
# Env: PREFIX (default pw1), OUT (default crates/coro_delegation/results),
# REPEATS (default "1 2 3"), LOCKFILE, STAGES (default "main confirm tokio").
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
: "${BIN:?set BIN to the frozen coro-bench binary}"
: "${TOKIO_BIN:?set TOKIO_BIN to the frozen tokio-bench binary}"
LOCKFILE=${LOCKFILE:-$HOME/.cache/locks-experiments/measurement.lock}
if [ -z "${UNDER_FLOCK:-}" ]; then
  exec env UNDER_FLOCK=1 flock "$LOCKFILE" "$0" "$@"
fi
export BIN SKIP_BUILD=1 FCPQ_WAIT_STATS=1
export PREFIX=${PREFIX:-pw1} OUT=${OUT:-crates/coro_delegation/results}
REPEATS=${REPEATS:-1 2 3}
STAGES=${STAGES:-main confirm tokio}
MAIN="co-pq-sremote-k64-home-c256 co-pq-snone-k64-home-c256 \
co-fifo-sremote-k64-home co-fifo-snone-k64-home \
dispatch dispatch-pq-home-c256 cfl ces-k64-home fc-remote fcpq-h16-home-c16"
CONFIRM="co-pq-sremote-k64-home-c256 co-fifo-sremote-k64-home \
dispatch-pq-home-c256 dispatch-pq-remote-c256 fcpq-h16-home-c16"
TOKIO_LOCKS="tokio-mutex async-lock"
LIGHT=1000
echo "binary $(sha256sum "$BIN")"
echo "tokio binary $(sha256sum "$TOKIO_BIN")"
echo "start $(date -u +%FT%TZ) scaling_max_freq cpu0=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq)"
root=$(cd "$here/../../.." && pwd)
has() { [[ " $STAGES " == *" $1 "* ]]; }
for r in $REPEATS; do
  for pm in spin yield; do
    if has main; then
      WORKERS=8 BALANCES=31 CONTENTIONS="sus bur" VARIANTS="$MAIN" \
        PARALLEL_MODE=$pm REPEATS=$r "$here/run_p3.sh"
    fi
    if has confirm; then
      WORKERS=16 BALANCES=31 CONTENTIONS=sus VARIANTS="$CONFIRM" \
        PARALLEL_MODE=$pm REPEATS=$r "$here/run_p3.sh"
    fi
    if has tokio; then
      if [ "$pm" = spin ]; then sfx=""; else sfx="-p$pm"; fi
      for l in $TOKIO_LOCKS; do
        for c in sus bur; do
          case $c in
            sus) cl=64; pw=$((4 * LIGHT)) ;;
            bur) cl=16; pw=$((32 * LIGHT)) ;;
          esac
          f="$root/$OUT/$PREFIX-tokio-$l$sfx-w8-h8-$c-r$r.json"
          if [ -f "$f" ]; then continue; fi
          "$TOKIO_BIN" --lock "$l" --workers 8 --heavy-ratio 8 --clients "$cl" \
            --parallel-work-cycles "$pw" --light-cs-cycles "$LIGHT" \
            --parallel-mode "$pm" --duration-ms 2000 --warmup-ms 200 \
            --out "$f" 2>/dev/null
          echo "[tokio] $f"
        done
      done
    fi
  done
done
echo "end $(date -u +%FT%TZ)"
echo "pw1 matrix complete"
