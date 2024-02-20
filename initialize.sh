tmux

sudo apt-get update
sudo apt-get install clang mold -y

git clone https://github.com/taooceros/Locks.git
cd Locks
git submodule init
git submodule update

curl https://sh.rustup.rs -sSf | sh -s -- -y
source "$HOME/.cargo/env"
rustup toolchain install nightly
rustup override set nightly
rustup component add rust-src

curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash
cargo binstall nu --no-confirm
nu