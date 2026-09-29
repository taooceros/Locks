#!/usr/bin/env bash
# Phase-3 matrix (H-D mitigations + placement + H-C revisited):
#   workers 8, heavy 8, contention {sus, bur}, balance {0, 31}, 3 repeats,
#   26 variants (see VARIANTS below) -> 312 runs, 2 s window, 200 ms warm-up.
# Override for the confirmation run, e.g.
#   WORKERS=16 VARIANTS="ces-t64000-home fcpq-h8-home" scripts/run_p3.sh
# Repeats are the outermost loop; existing files are skipped (restartable).
# Further overrides: BALANCES="31" (balance intervals), BIN=<coro-bench path>,
# SKIP_BUILD=1 (use BIN as is), PREFIX=<file prefix, default p3>,
# REPEATS="1 2 3" (repeat indices to run), FCPQ_WAIT_STATS=1 (fcpq and
# dispatch-pq runs add --fcpq-wait-stats: queue-wait histogram in the JSON),
# HANDOFF_STATS=1 (dispatch-pq / cfl runs add --handoff-stats), CONTENTIONS="sus"
# (default "sus bur"), OUT=<output dir, relative to the workspace root;
# default crates/coro_delegation/results>, PARALLEL_MODE=yield (default spin:
# client parallel work spun in the poll that released the lock; yield: one
# yield_now() first; file name gets -p<mode> after the label when != spin).
# dispatch-pq labels: dispatch-pq[-home|-remote][-c<N>][-n<init>] (clamp in
# handoffs, default 16).
# cfl labels: cfl[-noshfl][-nopre][-ovl][-remote][-c<N>][-spin<N>] (own
# placement home; clamp in handoffs, default 256); HANDOFF_STATS applies.
# co labels (coroutine-style mutex): co-fifo-s<step>-k<K>[-home|-remote] and
# co-pq-s<step>-k<K>[-home|-remote]-c<N>, step in {remote,home,none} (where
# unlock().await puts the releaser), K = chain bound (0 = unbounded),
# -home/-remote = chain-break wake placement, c = clamp in handoffs; the
# label coro-bench writes always carries -s, -k (and -c for co-pq).
set -euo pipefail
cd "$(dirname "$0")/../../.."
BIN=${BIN:-target/release/coro-bench}
OUT=${OUT:-crates/coro_delegation/results}
WORKERS=${WORKERS:-8}
BALANCES=${BALANCES:-0 31}
PREFIX=${PREFIX:-p3}
REPEATS=${REPEATS:-1 2 3}
CONTENTIONS=${CONTENTIONS:-sus bur}
PARALLEL_MODE=${PARALLEL_MODE:-spin}
if [ "$PARALLEL_MODE" = spin ]; then pm=""; pmargs=(); else
  pm="-p$PARALLEL_MODE"; pmargs=(--parallel-mode "$PARALLEL_MODE"); fi
LIGHT=1000
DEFAULT_VARIANTS="dispatch dispatch-home \
ces ces-home ces-k64 ces-k64-home ces-t64000 ces-t64000-home \
fc fc-home fc-remote fcpq fcpq-home fcpq-remote \
fcpq-h8 fcpq-h8-home fcpq-h16 fcpq-h16-home fcpq-t16000 fcpq-t16000-home \
fcpq-rotate fcpq-rotate-home fcpq-credit fcpq-credit-home fcpq-elect fcpq-elect-home"
VARIANTS=${VARIANTS:-$DEFAULT_VARIANTS}

# label -> coro-bench arguments
args_for() {
  local v=$1 base rest a=()
  base=${v%%-*}
  # multi-word base ids
  case $v in
    actor-inline|actor-inline-*) base=actor-inline ;;
    dispatch-pq|dispatch-pq-*) base=dispatch-pq ;;
    co-fifo|co-fifo-*) base=co-fifo ;;
    co-pq|co-pq-*) base=co-pq ;;
  esac
  rest=${v#"$base"}
  a+=(--lock "$base")
  case $base in fcpq|dispatch-pq|co-pq)
    if [ -n "${FCPQ_WAIT_STATS:-}" ]; then a+=(--fcpq-wait-stats); fi ;;
  esac
  case $base in dispatch-pq|cfl)
    if [ -n "${HANDOFF_STATS:-}" ]; then a+=(--handoff-stats); fi ;;
  esac
  # suffixes: -home -remote -k<K> -t<T> -h<H> -rotate -credit -elect -noyield
  #           -c<N> (fcpq starvation clamp, 0 = off) -n{mean,zero,min,median}
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
      c[0-9]*) a+=(--starvation-clamp "${s#c}") ;;
      nmean|nzero|nmin|nmedian) a+=(--newcomer-init "${s#n}") ;;
      noshfl) a+=(--cfl-shuffle off) ;;
      nopre)  a+=(--cfl-prewake off) ;;
      ovl)    a+=(--cfl-scan overlap) ;;
      spin[0-9]*) a+=(--head-spin-cycles "${s#spin}") ;;
      sremote|shome|snone) a+=(--co-step-aside "${s#s}") ;;
      k*)     a+=(--ces-chain-bound "${s#k}") ;;
      h*)     a+=(--pass-limit "${s#h}") ;;
      t*)     if [ "$base" = ces ]; then a+=(--ces-chain-budget-cycles "${s#t}");
              else a+=(--pass-budget-cycles "${s#t}"); fi ;;
      *) echo "unknown suffix $s in $v" >&2; exit 2 ;;
    esac
  done
  printf '%s\n' "${a[@]}"
}

[ -n "${SKIP_BUILD:-}" ] || cargo build --release -p coro_delegation >/dev/null
mkdir -p "$OUT"
nv=$(wc -w <<<"$VARIANTS")
nb=$(wc -w <<<"$BALANCES")
total=$(($(wc -w <<<"$REPEATS") * nv * nb * $(wc -w <<<"$CONTENTIONS")))
n=0
for r in $REPEATS; do
  for v in $VARIANTS; do
    mapfile -t vargs < <(args_for "$v")
    for b in $BALANCES; do
      for c in $CONTENTIONS; do
        case $c in
          sus) cl=64; pw=$((4 * LIGHT)) ;;
          bur) cl=16; pw=$((32 * LIGHT)) ;;
        esac
        f="$OUT/$PREFIX-$v$pm-w$WORKERS-h8-b$b-$c-r$r.json"
        n=$((n + 1))
        if [ -f "$f" ]; then continue; fi
        "$BIN" "${vargs[@]}" "${pmargs[@]}" --workers "$WORKERS" --heavy-ratio 8 \
          --balance-interval "$b" --clients "$cl" --parallel-work-cycles "$pw" \
          --light-cs-cycles "$LIGHT" --duration-ms 2000 --warmup-ms 200 \
          --out "$f" 2>/dev/null
        echo "[$n/$total] $f"
      done
    done
  done
done
echo "p3 matrix complete"
