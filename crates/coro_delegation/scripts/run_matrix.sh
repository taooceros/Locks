#!/usr/bin/env bash
# Phase-2 matrix (RESEARCH.md, Method step 3):
#   locks {dispatch, ces, fc, fcpq, fc-noyield, fcpq-noyield}
#   x workers {4, 8, 16} x heavy-ratio {1, 8} x balance-interval {0, 31}
#   x contention {sus: 64 clients, 4x parallel; bur: 16 clients, 32x parallel}
#   x 3 repeats, 2 s window, 200 ms warm-up.
# `fc`/`fcpq` use the locks' default (combiner yields after a pass);
# `*-noyield` is the original flat-combining behaviour (--no-combiner-yield).
# Repeats are the outermost loop so every cell gets its i-th repeat in the
# same time band (drift affects all cells alike). Existing files are skipped,
# so the script is restartable. Runs sequentially; keep the box otherwise idle.
set -euo pipefail
cd "$(dirname "$0")/../../.."
BIN=target/release/coro-bench
OUT=crates/coro_delegation/results
cargo build --release -p coro_delegation >/dev/null
mkdir -p "$OUT"
total=$((3 * 6 * 3 * 2 * 2 * 2))
n=0
for r in 1 2 3; do
  for variant in dispatch ces fc fcpq fc-noyield fcpq-noyield; do
    case $variant in
      fc-noyield)   lock=fc;   extra=(--no-combiner-yield) ;;
      fcpq-noyield) lock=fcpq; extra=(--no-combiner-yield) ;;
      *)            lock=$variant; extra=() ;;
    esac
    for w in 4 8 16; do
      for h in 1 8; do
        for b in 0 31; do
          for c in sus bur; do
            case $c in
              sus) cl=64; pw=4000 ;;
              bur) cl=16; pw=32000 ;;
            esac
            f="$OUT/matrix-$variant-w$w-h$h-b$b-$c-r$r.json"
            n=$((n + 1))
            if [ -f "$f" ]; then continue; fi
            "$BIN" --lock "$lock" "${extra[@]}" --workers "$w" --heavy-ratio "$h" \
              --balance-interval "$b" --clients "$cl" --parallel-work-cycles "$pw" \
              --duration-ms 2000 --warmup-ms 200 --out "$f" 2>/dev/null
            echo "[$n/$total] $f"
          done
        done
      done
    done
  done
done
echo "matrix complete"
