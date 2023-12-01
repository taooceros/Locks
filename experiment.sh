cd rust

cargo build --release

# target/release/dlock -e counter-ratio-one-three -d 30
target/release/dlock -e response-time-single-addition -t 2 4 8 16 32 64 128 -d 30
target/release/dlock -e response-time-ratio-one-three -t 2 4 8 16 32 64 128 -d 30