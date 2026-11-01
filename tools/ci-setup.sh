#!/bin/sh
set -eu
dnf -y install curl tar gzip git python3 gcc openssl-devel
curl --proto '=https' --tlsv1.2 -fsS https://sh.rustup.rs -o /tmp/rustup-init.sh
sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain 1.100.0-beta
. "$HOME/.cargo/env"
rustup component add rustfmt clippy
rustc --version --verbose | grep -F 'commit-hash: fd8e211ae301c84a86d13cf52a6d3ce08c589e2e'
curl -fLsS https://github.com/jj-vcs/jj/releases/download/v0.46.0/jj-v0.46.0-x86_64-unknown-linux-musl.tar.gz -o /tmp/jj.tar.gz
printf '%s  %s\n' fce0271158e665ceb82dc66c5eb95b1728649094e9133074f3b5baece345fc08 /tmp/jj.tar.gz | sha256sum -c -
tar -xzf /tmp/jj.tar.gz -C /usr/local/bin ./jj
jj --version | grep -Fx "jj 0.46.0-7d382314f9420b4b8d932245b49c99492c812b9f"
