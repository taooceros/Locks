cd rust

cargo build --release

# target/release/dlock -e counter-ratio-one-three -d 30
target/release/dlock counter-proportional -t 32 64 --stat-response-time
target/release/dlock response-time-single-addition -t 32 64 --stat-response-time


cd ..

tar --zstd -cf output.tar.zst visualization/output