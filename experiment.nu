cd rust

cargo build --release

# target/release/dlock -e counter-ratio-one-three -d 30

let threads = seq 1 6 | each { |it| 2 ** $it} | str join ","
# echo $threads

let base_duration = 1000

let cs = seq $base_duration $base_duration ((3 * $base_duration)) | str join ","

alias dlock2 = target/release/dlock d-lock2

# echo $cs

dlock2 counter-proportional -t $threads --cs $cs --stat-response-time -d 10

let cs = seq $base_duration $base_duration ((8 * $base_duration)) | str join ","
dlock2 counter-proportional -t $threads --cs $cs --stat-response-time -d 10

let noncs = $base_duration
dlock2 counter-proportional -t $threads --cs $cs --non-cs $noncs --stat-response-time -d 10

dlock2 counter-proportional -t 8 16 --stat-response-time --cs 1

dlock2 fetch-and-multiply -t $threads --stat-response-time

dlock2 queue -t $threads --stat-response-time

dlock2 priority-queue -t $threads --stat-response-time