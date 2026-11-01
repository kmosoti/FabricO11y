#!/bin/sh
set -eu
dnf -y install curl tar gzip git python3 gcc openssl-devel jj-cli-0.46.0-1.fc44
curl --proto '=https' --tlsv1.2 -fsS https://sh.rustup.rs -o /tmp/rustup-init.sh
sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain 1.100.0-beta
. "$HOME/.cargo/env"
rustup component add rustfmt clippy
rustc --version --verbose | grep -F 'commit-hash: fd8e211ae301c84a86d13cf52a6d3ce08c589e2e'
rpm -qf /usr/bin/jj
/usr/bin/jj --version | grep -Fx "jj 0.46.0"
