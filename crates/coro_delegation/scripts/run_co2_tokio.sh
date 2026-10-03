#!/usr/bin/env bash
# tokio::sync::Mutex in the session of the co2 matrix (scripts/run_co2.sh):
# W=8, heavy 8, sustained + bursty, parallel mode spin and yield, 3 repeats.
#   TOKIO_BIN=<frozen tokio-bench> scripts/run_co2_tokio.sh
# Files: $OUT/co2-tokio-mutex-w8-h8-<sus|bur>-<spin|yield>-r<i>.json
set -euo pipefail
: "${TOKIO_BIN:?set TOKIO_BIN to the frozen tokio-bench binary}"
LOCKFILE=${LOCKFILE:-$HOME/.cache/locks-experiments/measurement.lock}
if [ -z "${UNDER_FLOCK:-}" ]; then
  exec env UNDER_FLOCK=1 flock "$LOCKFILE" "$0" "$@"
fi
cd "$(dirname "$0")/../../.."
OUT=${OUT:-crates/coro_delegation/results}
LIGHT=1000
echo "tokio binary $(sha256sum "$TOKIO_BIN")"
echo "start $(date -Is)"
for r in 1 2 3; do
  for pm in spin yield; do
    for c in sus bur; do
      case $c in sus) cl=64; pw=$((4 * LIGHT)) ;; bur) cl=16; pw=$((32 * LIGHT)) ;; esac
      f="$OUT/co2-tokio-mutex-w8-h8-$c-$pm-r$r.json"
      [ -f "$f" ] && continue
      "$TOKIO_BIN" --lock tokio-mutex --workers 8 --heavy-ratio 8 --clients "$cl" \
        --parallel-work-cycles "$pw" --light-cs-cycles "$LIGHT" --parallel-mode "$pm" \
        --duration-ms 2000 --warmup-ms 200 --out "$f" 2>/dev/null
      echo "$f"
    done
  done
done
echo "end $(date -Is)"
