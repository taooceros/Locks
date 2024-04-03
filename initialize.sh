sudo apt-get update
sudo apt-get install clang mold -y

git clone https://github.com/taooceros/Locks.git
cd Locks
git submodule init
git submodule update

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain none -y
source "$HOME/.cargo/env"
rustup toolchain install nightly
rustup override set nightly
rustup component add rust-src

# create ~/bin
mkdir -p ~/bin

curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash
cargo binstall nu --no-confirm
cargo binstall just --no-confirm
nu