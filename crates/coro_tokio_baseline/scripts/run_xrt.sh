#!/usr/bin/env bash
# Cross-runtime baseline (coro_delegation FINDINGS, 2026-09-28 "Cross-runtime
# baseline: tokio locks"). Sequential, two phases in one time window:
#   1. tokio-bench: 5 locks x workers {8,16} x {sus,bur} x 3 repeats
#      -> results/tokio-<lock>-w<W>-h8-<sus|bur>-r<i>.json
#   2. the EXISTING target/release/coro-bench (never rebuilt here): 5 variants
#      x the same cells, heavy 8, balance 31, 3 repeats
#      -> results/xrt-<label>-w<W>-h8-b31-<sus|bur>-r<i>.json
# Repeats are the outermost loop of each phase; existing files are skipped
# (restartable). Override TOKIO_LOCKS / CORO_VARIANTS / WORKERS_SET to subset.
# Hold ~/.cache/locks-experiments/measurement.lock while running (flock).
#
# COOLDOWN_S (default 20) idle seconds after every std-mutex / parking-lot
# run: on the study machine CPUs 0-7 sit at 2.2 GHz instead of 3.69 GHz
# (scaling_cur_freq) for 5-15 s after such a run, which slows the next runs'
# non-spin work (BTreeMap insert ~2x) and cut tokio-mutex w8 sus throughput
# by ~20 %; 15 s of idle removed it, 5 s did not.
set -euo pipefail
cd "$(dirname "$0")/../../.."
TOKIO_BIN=target/release/tokio-bench
CORO_BIN=target/release/coro-bench
OUT=crates/coro_delegation/results
LIGHT=1000
TOKIO_LOCKS=${TOKIO_LOCKS:-"tokio-mutex async-lock std-mutex parking-lot tokio-mutex-unconstrained"}
CORO_VARIANTS=${CORO_VARIANTS:-"dispatch dispatch-home ces-k64-home fc-remote fcpq-h16-home"}
WORKERS_SET=${WORKERS_SET:-"8 16"}
COOLDOWN_S=${COOLDOWN_S:-20}

# label -> coro-bench arguments (copied from crates/coro_delegation/scripts/run_p3.sh)
args_for() {
  local v=$1 base rest a=()
  base=${v%%-*}
  rest=${v#"$base"}
  a+=(--lock "$base")
  # suffixes: -home -remote -k<K> -t<T> -h<H> -rotate -credit -elect -noyield
  local IFS='-'
  for s in $rest; do
    [ -z "$s" ] && continue
    case $s in
      home)   a+=(--wake-placement home) ;;
      remote) a+=(--wake-placement remote) ;;
      rotate) a+=(--rotate-combiner) ;;
      credit) a+=(--credit-combining) ;;
      elect)  a+=(--elect-max-usage) ;;
      noyield) a+=(--no-combiner-yield) ;;
      k*)     a+=(--ces-chain-bound "${s#k}") ;;
      h*)     a+=(--pass-limit "${s#h}") ;;
      t*)     if [ "$base" = ces ]; then a+=(--ces-chain-budget-cycles "${s#t}");
              else a+=(--pass-budget-cycles "${s#t}"); fi ;;
      *) echo "unknown suffix $s in $v" >&2; exit 2 ;;
    esac
  done
  printf '%s\n' "${a[@]}"
}

# contention label -> "clients parallel_work_cycles"
cell_for() {
  case $1 in
    sus) echo "64 $((4 * LIGHT))" ;;
    bur) echo "16 $((32 * LIGHT))" ;;
  esac
}

cargo build --release -p coro_tokio_baseline >/dev/null
[ -x "$CORO_BIN" ] || { echo "missing $CORO_BIN (build coro_delegation first)" >&2; exit 1; }
mkdir -p "$OUT"
nw=$(wc -w <<<"$WORKERS_SET")
total=$((3 * nw * 2 * ($(wc -w <<<"$TOKIO_LOCKS") + $(wc -w <<<"$CORO_VARIANTS"))))
n=0

for r in 1 2 3; do
  for l in $TOKIO_LOCKS; do
    for w in $WORKERS_SET; do
      for c in sus bur; do
        read -r cl pw <<<"$(cell_for "$c")"
        f="$OUT/tokio-$l-w$w-h8-$c-r$r.json"
        n=$((n + 1))
        if [ -f "$f" ]; then continue; fi
        "$TOKIO_BIN" --lock "$l" --workers "$w" --heavy-ratio 8 \
          --clients "$cl" --parallel-work-cycles "$pw" \
          --light-cs-cycles "$LIGHT" --duration-ms 2000 --warmup-ms 200 \
          --out "$f" 2>/dev/null
        echo "[$n/$total] $f"
        case $l in std-mutex|parking-lot) sleep "$COOLDOWN_S" ;; esac
      done
    done
  done
done

for r in 1 2 3; do
  for v in $CORO_VARIANTS; do
    mapfile -t vargs < <(args_for "$v")
    for w in $WORKERS_SET; do
      for c in sus bur; do
        read -r cl pw <<<"$(cell_for "$c")"
        f="$OUT/xrt-$v-w$w-h8-b31-$c-r$r.json"
        n=$((n + 1))
        if [ -f "$f" ]; then continue; fi
        "$CORO_BIN" "${vargs[@]}" --workers "$w" --heavy-ratio 8 \
          --balance-interval 31 --clients "$cl" --parallel-work-cycles "$pw" \
          --light-cs-cycles "$LIGHT" --duration-ms 2000 --warmup-ms 200 \
          --out "$f" 2>/dev/null
        echo "[$n/$total] $f"
      done
    done
  done
done
echo "xrt matrix complete"
