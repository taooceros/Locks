sudo apt-get update
sudo apt-get install clang mold

git clone https://github.com/taooceros/Locks.git
cd Locks
git submodule init
git submodule update

curl https://sh.rustup.rs -sSf | sh -s -- -y
source "$HOME/.cargo/env"
rustup toolchain install nightly
rustup override set nightly
rustup component add rust-src
