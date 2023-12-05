cd rust

cargo build --release

# target/release/dlock -e counter-ratio-one-three -d 30
target/release/dlock counter-proportional -t 2 4 8 16 32 64 128 -d 5
target/release/dlock response-time-single-addition -t 2 4 8 16 32 64 128 -d 5


cd ..

tar --zstd -cf output.tar.zst visualization/output