#!/bin/sh
set -eu
dnf -y install curl tar gzip git python3 gcc openssl-devel
# Match the existing workstation's system RPM provider; limit it to jj-cli.
curl -fLsS https://download.copr.fedorainfracloud.org/results/aldantanneo/jj-vcs/pubkey.gpg -o /tmp/jj-provider.gpg
printf '%s  %s\n' 4a97c99ae7e49688e50a9ec6b646e798119c533acd17d4633986d0a3cb1ff8c4 /tmp/jj-provider.gpg | sha256sum -c -
cat > /etc/yum.repos.d/fabric-jj-system.repo <<'REPO'
[fabric-jj-system]
name=Existing workstation Jujutsu RPM provider
baseurl=https://download.copr.fedorainfracloud.org/results/aldantanneo/jj-vcs/fedora-$releasever-$basearch/
enabled=1
gpgcheck=1
gpgkey=file:///tmp/jj-provider.gpg
includepkgs=jj-cli
skip_if_unavailable=False
REPO
dnf -y install jj-cli-0.46.0-1.fc44
curl --proto '=https' --tlsv1.2 -fsS https://sh.rustup.rs -o /tmp/rustup-init.sh
sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain 1.100.0-beta
. "$HOME/.cargo/env"
rustup component add rustfmt clippy
rustc --version --verbose | grep -F 'commit-hash: fd8e211ae301c84a86d13cf52a6d3ce08c589e2e'
rpm -qf /usr/bin/jj
/usr/bin/jj --version | grep -Fx "jj 0.46.0"
