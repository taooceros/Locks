cd rust

cargo build --release

# target/release/dlock -e counter-ratio-one-three -d 30

function join_by {
  local d=${1-} f=${2-}
  if shift 2; then
    printf %s "$f" "${@/#/$d}"
  fi
}

base_duration=50000
cs=$(join_by , $(seq $base_duration $((2 * $base_duration)) $((3 * $base_duration + 1))))

echo $cs

target/release/dlock counter-proportional -t 16,32,64 --cs $cs --stat-response-time -d 15 --file-name counter-proportional-one-three

cs=$(join_by , $(seq $base_duration $(($base_duration)) $((8 * $base_duration + 1))))
target/release/dlock counter-proportional -t 16,32,64 --cs $cs --stat-response-time -d 15 --file-name counter-proportional-one-to-eight

noncs=$base_duration
target/release/dlock counter-proportional -t 16,32,64 --cs $cs --non-cs $noncs --stat-response-time -d 15 --file-name counter-proportional-one-to-eight

target/release/dlock response-time-single-addition -t 32 64 --stat-response-time
