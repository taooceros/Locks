#!/usr/bin/env bash
# Same-window matrix for the ordinary-waker co-pq (FINDINGS 2026-09-30,
# "ordinary-waker co-pq, same-window matrix"). One frozen binary, one
# measurement-lock hold, repeats outermost. Flags are passed explicitly
# (label grammar of run_p3.sh, restated in args_for). Files:
#   $OUT/co2-<label>-w<W>-h8-b31-<sus|bur>-<spin|yield>-r<i>.json
#   BIN=<frozen coro-bench> scripts/run_co2.sh
# Env: OUT (default crates/coro_delegation/results), REPEATS ("1 2 3"), LOCKFILE.
set -euo pipefail
: "${BIN:?set BIN to the frozen coro-bench binary}"
LOCKFILE=${LOCKFILE:-$HOME/.cache/locks-experiments/measurement.lock}
if [ -z "${UNDER_FLOCK:-}" ]; then
  exec env UNDER_FLOCK=1 flock "$LOCKFILE" "$0" "$@"
fi
cd "$(dirname "$0")/../../.."
OUT=${OUT:-crates/coro_delegation/results}
REPEATS=${REPEATS:-1 2 3}
LIGHT=1000
mkdir -p "$OUT"

args_for() {
  case $1 in
    co-pq-c256)   echo "--lock co-pq --starvation-clamp 256" ;;
    co-pq-c0)     echo "--lock co-pq --starvation-clamp 0" ;;
    co-fifo-sremote-k64-home) echo "--lock co-fifo --co-step-aside remote --ces-chain-bound 64 --wake-placement home" ;;
    ces-k64-home) echo "--lock ces --ces-chain-bound 64 --wake-placement home" ;;
    fc-remote)    echo "--lock fc --wake-placement remote" ;;
    fcpq-h16-home-c16) echo "--lock fcpq --pass-limit 16 --wake-placement home --starvation-clamp 16" ;;
    dispatch)     echo "--lock dispatch" ;;
    dispatch-pq-home-c256) echo "--lock dispatch-pq --wake-placement home --starvation-clamp 256" ;;
    *) echo "unknown label $1" >&2; exit 2 ;;
  esac
}
W8="co-pq-c256 co-pq-c0 co-fifo-sremote-k64-home ces-k64-home fc-remote fcpq-h16-home-c16 dispatch dispatch-pq-home-c256"
W16="co-pq-c256 co-fifo-sremote-k64-home fcpq-h16-home-c16 dispatch-pq-home-c256"

run() { # W label contention mode repeat
  local w=$1 v=$2 c=$3 pm=$4 r=$5 cl pw f
  case $c in sus) cl=64; pw=$((4 * LIGHT)) ;; bur) cl=16; pw=$((32 * LIGHT)) ;; esac
  f="$OUT/co2-$v-w$w-h8-b31-$c-$pm-r$r.json"
  [ -f "$f" ] && return
  # shellcheck disable=SC2046
  "$BIN" $(args_for "$v") --fcpq-wait-stats --parallel-mode "$pm" --workers "$w" \
    --heavy-ratio 8 --balance-interval 31 --clients "$cl" --parallel-work-cycles "$pw" \
    --light-cs-cycles "$LIGHT" --duration-ms 2000 --warmup-ms 200 --out "$f" 2>/dev/null
  echo "$f"
}
echo "binary $(sha256sum "$BIN")"
echo "start $(date -Is)"
for r in $REPEATS; do
  for pm in spin yield; do
    for v in $W8; do for c in sus bur; do run 8 "$v" "$c" "$pm" "$r"; done; done
    for v in $W16; do run 16 "$v" sus "$pm" "$r"; done
  done
done
echo "end $(date -Is)"
echo "co2 matrix complete"
