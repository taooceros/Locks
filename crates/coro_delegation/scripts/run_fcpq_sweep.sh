#!/usr/bin/env bash
# FC-PQ starvation-clamp x newcomer-init sweep (FINDINGS 2026-09-29) through
# run_p3.sh, with a frozen binary (BIN, default target/fcpq-sweep/coro-bench,
# a copy of target/release/coro-bench) so concurrent rebuilds cannot swap it.
# Run under the measurement lock:
#   flock ~/.cache/locks-experiments/measurement.lock \
#     crates/coro_delegation/scripts/run_fcpq_sweep.sh sweep
#   VARIANTS="<labels>" flock ... run_fcpq_sweep.sh confirm
#   flock ... run_fcpq_sweep.sh ab
# sweep:   fcpq-h16-home-c{8,16,32,0}-n{mean,zero,min,median}; W=8,
#          balance 31, sus + bur, 3 repeats, with --fcpq-wait-stats.
# confirm: VARIANTS at W=8 balance 0 and W=16 balance 31, --fcpq-wait-stats.
# ab:      same-window check of the defaults, repeats interleaved, W=8 b31:
#          PRE_BIN (pre-change code, default target/fcpq-sweep/coro-bench-pre)
#          and BIN with default flags on fcpq-h16-home + fc-remote
#          (p3ab-pre-*, p3ab-final-*); BIN with --fcpq-wait-stats and, if
#          present, SWEEP_BIN (default target/fcpq-sweep/coro-bench-sweep-d07645d5,
#          the binary the 2026-09-29 sweep ran on, accounting always on) on
#          fcpq-h16-home (p3ab-stats-*, p3ab-sweepbin-*).
# iso:     like ab on fcpq-h16-home with default flags, for four binaries:
#          ISO_PRE / ISO_MINE = the pre-change crate and pre-change + this
#          entry's fc_pq.rs / coro_bench.rs diff, each built alone in a
#          minimal workspace (target/fcpq-sweep/coro-bench-iso-{pre,mine}),
#          plus PRE_BIN and BIN (p3iso-{isopre,isomine,pre,final}-*).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
export BIN=${BIN:-$root/target/fcpq-sweep/coro-bench} SKIP_BUILD=1
stage=${1:-sweep}
echo "start unix $(date -u +%s)"; uptime
sha256sum "$BIN"
"$BIN" --sanity
case $stage in
  sweep)
    grid=""
    for c in 8 16 32 0; do
      for n in mean zero min median; do grid+="fcpq-h16-home-c$c-n$n "; done
    done
    FCPQ_WAIT_STATS=1 VARIANTS="$grid" BALANCES=31 WORKERS=8 "$here/run_p3.sh"
    ;;
  confirm)
    : "${VARIANTS:?set VARIANTS to the label(s) to confirm}"
    FCPQ_WAIT_STATS=1 BALANCES=0 WORKERS=8 "$here/run_p3.sh"
    FCPQ_WAIT_STATS=1 BALANCES=31 WORKERS=16 "$here/run_p3.sh"
    ;;
  ab)
    pre=${PRE_BIN:-$root/target/fcpq-sweep/coro-bench-pre}
    sweepbin=${SWEEP_BIN:-$root/target/fcpq-sweep/coro-bench-sweep-d07645d5}
    sha256sum "$pre"
    [ -x "$sweepbin" ] && sha256sum "$sweepbin"
    both="fcpq-h16-home fc-remote"
    for r in 1 2 3; do
      BIN=$pre PREFIX=p3ab-pre REPEATS=$r VARIANTS="$both" BALANCES=31 WORKERS=8 "$here/run_p3.sh"
      PREFIX=p3ab-final REPEATS=$r VARIANTS="$both" BALANCES=31 WORKERS=8 "$here/run_p3.sh"
      FCPQ_WAIT_STATS=1 PREFIX=p3ab-stats REPEATS=$r VARIANTS=fcpq-h16-home \
        BALANCES=31 WORKERS=8 "$here/run_p3.sh"
      if [ -x "$sweepbin" ]; then
        BIN=$sweepbin PREFIX=p3ab-sweepbin REPEATS=$r VARIANTS=fcpq-h16-home \
          BALANCES=31 WORKERS=8 "$here/run_p3.sh"
      fi
    done
    ;;
  iso)
    runs="isopre:${ISO_PRE:-$root/target/fcpq-sweep/coro-bench-iso-pre}
isomine:${ISO_MINE:-$root/target/fcpq-sweep/coro-bench-iso-mine}
pre:${PRE_BIN:-$root/target/fcpq-sweep/coro-bench-pre}
final:$BIN"
    for r in 1 2 3; do
      while IFS=: read -r name bin; do
        [ "$r" = 1 ] && sha256sum "$bin"
        BIN=$bin PREFIX=p3iso-$name REPEATS=$r VARIANTS=fcpq-h16-home \
          BALANCES=31 WORKERS=8 "$here/run_p3.sh" </dev/null
      done <<<"$runs"
    done
    ;;
  *) echo "usage: $0 {sweep|confirm|ab|iso}" >&2; exit 2 ;;
esac
echo "end unix $(date -u +%s)"; uptime
