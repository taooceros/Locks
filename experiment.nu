cd rust

cargo build --release

# target/release/dlock -e counter-ratio-one-three -d 30

let threads = seq 2 6 | each { |it| 2 ** $it} | str join ","
# echo $threads

let base_duration = 500

let cs = seq $base_duration $base_duration ((3 * $base_duration)) | str join ","

# echo $cs

target/release/dlock counter-proportional -t $threads --cs $cs --stat-response-time -d 15 --file-name $"counter-1-3-($base_duration)"

let cs = seq $base_duration $base_duration ((8 * $base_duration)) | str join ","
target/release/dlock counter-proportional -t $threads --cs $cs --stat-response-time -d 15 --file-name $"counter-1-8-($base_duration)"

let noncs = $base_duration
target/release/dlock counter-proportional -t $threads --cs $cs --non-cs $noncs --stat-response-time -d 15 --file-name counter-proportional-one-to-eight

target/release/dlock response-time-single-addition -t 32 64 --stat-response-time
