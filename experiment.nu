cd rust

cargo build --release

let threads = seq 1 6 | each { |it| 2 ** $it} | str join ","
# echo $threads

let base_duration = 1000

let simple_cs = "1000,3000"

alias dlock2 = target/release/dlock d-lock2

let short_experiment_length = 3
let long_experiment_length = 8

# echo $cs

dlock2 counter-proportional -t $threads --cs $simple_cs --non-cs 0 -d $long_experiment_length

for non_cs in (seq 1 5 | each { |it| 10 ** $it}) {
    dlock2 counter-proportional -t $threads --cs $simple_cs --non-cs $non_cs -d $long_experiment_length
}

dlock2 counter-proportional -t 8,16 --cs 1 --non-cs 0 -d $short_experiment_length --stat-response-time --file-name "single-addition-latency"
dlock2 counter-proportional -t $threads --cs 1 --non-cs 0 -d $long_experiment_length --file-name "single-addition"



# let cs = seq $base_duration $base_duration ((8 * $base_duration)) | str join ","
# dlock2 counter-proportional -t $threads --cs $cs --stat-response-time -d 10

# let noncs = $base_duration
# dlock2 counter-proportional -t $threads --cs $cs --non-cs $noncs --stat-response-time -d 10

# dlock2 counter-proportional -t 8 16 --stat-response-time --cs 1

# dlock2 fetch-and-multiply -t $threads --stat-response-time -d $short_experiment_length
# dlock2 fetch-and-multiply -t $threads -d $long_experiment_length

# dlock2 queue -t $threads --stat-response-time -d $short_experiment_length
# dlock2 queue -t $threads -d $long_experiment_length

# dlock2 priority-queue -t $threads --stat-response-time -d $short_experiment_length
# dlock2 priority-queue -t $threads -d $long_experiment_length