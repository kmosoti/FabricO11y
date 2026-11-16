#!/bin/sh
# Bootstrap the pinned native checkers on Fedora; does not modify host security policy.
set -eu
: "${FABRIC_PROOF_TOOLS:=$HOME/.local/share/fabrico11y/proof-tools}"
export FABRIC_PROOF_TOOLS
cargo_jobs="${CARGO_BUILD_JOBS:-4}"
lean_threads="${LEAN_NUM_THREADS:-4}"
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
dnf -y install curl git tar zstd unzip make gcc gcc-c++ gmp-devel opam bubblewrap time perf
mkdir -p "$FABRIC_PROOF_TOOLS/downloads"
fetch() {
    if ! printf '%s  %s\n' "$3" "$FABRIC_PROOF_TOOLS/downloads/$2" | sha256sum -c - 2>/dev/null; then
        curl --proto '=https' --tlsv1.2 -fLsS "$1" -o "$FABRIC_PROOF_TOOLS/downloads/$2"
        printf '%s  %s\n' "$3" "$FABRIC_PROOF_TOOLS/downloads/$2" | sha256sum -c -
    fi
}
fetch https://github.com/leanprover/lean4/releases/download/v4.35.0/lean-4.35.0-linux.tar.zst lean-4.35.0-linux.tar.zst 7289597a44543246c9227b8cae501767e095ce17f6d448b95fc4a8dc97268f6d
fetch https://github.com/verus-lang/verus/releases/download/release%2F0.2026.10.26.d17fbd0/verus-0.2026.10.26.d17fbd0-x86-linux.zip verus-0.2026.10.26.d17fbd0-x86-linux.zip 361674d05f0c1b2c9c3f4d5bc56a8fb2ab9241ed0cceca27780668c292bb4940
fetch https://github.com/ocaml/opam/releases/download/2.6.1/opam-2.6.1-x86_64-linux opam-2.6.1-x86_64-linux b533d393a6967150d6ef41cb3d71e147e93ecbabeb59e921aa412c0ae79ebe3d
chmod +x "$FABRIC_PROOF_TOOLS/downloads/opam-2.6.1-x86_64-linux"
opam() { "$FABRIC_PROOF_TOOLS/downloads/opam-2.6.1-x86_64-linux" "$@"; }
lean_bin="$FABRIC_PROOF_TOOLS/lean-4.35.0-linux/bin/lean"
lake_bin="$FABRIC_PROOF_TOOLS/lean-4.35.0-linux/bin/lake"
if [ ! -x "$lean_bin" ] || [ ! -x "$lake_bin" ]; then
    rm -rf "$FABRIC_PROOF_TOOLS/lean-4.35.0-linux"
    tar --zstd -xf "$FABRIC_PROOF_TOOLS/downloads/lean-4.35.0-linux.tar.zst" -C "$FABRIC_PROOF_TOOLS"
fi
verus_bin="$FABRIC_PROOF_TOOLS/verus-0.2026.10.26.d17fbd0/verus-x86-linux/verus"
if [ ! -x "$verus_bin" ]; then
    rm -rf "$FABRIC_PROOF_TOOLS/verus-0.2026.10.26.d17fbd0"
    unzip -q "$FABRIC_PROOF_TOOLS/downloads/verus-0.2026.10.26.d17fbd0-x86-linux.zip" -d "$FABRIC_PROOF_TOOLS/verus-0.2026.10.26.d17fbd0"
fi
if ! rustup run 1.99.1 rustc --version >/dev/null 2>&1; then
    rustup toolchain install 1.99.1 --profile minimal
fi
nightly_components=$(rustup component list --toolchain nightly-2026-09-17 --installed 2>/dev/null || true)
if ! rustup run nightly-2026-09-17 rustc --version >/dev/null 2>&1 \
    || ! printf '%s\n' "$nightly_components" | grep -q '^rustc-dev-' \
    || ! printf '%s\n' "$nightly_components" | grep -q '^rust-src' \
    || ! printf '%s\n' "$nightly_components" | grep -q '^llvm-tools'; then
    rustup toolchain install nightly-2026-09-17 --profile minimal --component rustc-dev,rust-src,llvm-tools-preview
fi
cargo_fuzz_bin="$FABRIC_PROOF_TOOLS/cargo-fuzz-0.13.2/bin/cargo-fuzz"
if [ ! -x "$cargo_fuzz_bin" ] \
    || [ "$("$cargo_fuzz_bin" --version 2>/dev/null || true)" != "cargo-fuzz 0.13.2" ]; then
    cargo install cargo-fuzz --version '=0.13.2' --locked --force --root "$FABRIC_PROOF_TOOLS/cargo-fuzz-0.13.2" --jobs "$cargo_jobs"
fi
# Sources and transitive OCaml packages are pinned separately from application Cargo.
aeneas_dir="$FABRIC_PROOF_TOOLS/aeneas-ab34bd2"
if [ ! -d "$aeneas_dir" ]; then
    git clone https://github.com/AeneasVerif/aeneas.git "$aeneas_dir"
    git -C "$aeneas_dir" checkout ab34bd28458a08c150b7cbf1873e59af74588138
    git clone https://github.com/AeneasVerif/charon.git "$aeneas_dir/charon"
    git -C "$aeneas_dir/charon" checkout 7b462d9159db2ed3015162352183906a30bfafc6
    git -C "$aeneas_dir/charon" submodule update --init --recursive
fi
test "$(git -C "$aeneas_dir" rev-parse HEAD)" = ab34bd28458a08c150b7cbf1873e59af74588138
test "$(git -C "$aeneas_dir/charon" rev-parse HEAD)" = 7b462d9159db2ed3015162352183906a30bfafc6
charon_bin="$aeneas_dir/charon/charon/target/release/charon"
if [ ! -x "$charon_bin" ] \
    || [ "$("$charon_bin" version 2>/dev/null || true)" != "0.1.304 (7b462d9159db2ed3015162352183906a30bfafc6)" ] \
    || [ "$("$charon_bin" toolchain-version 2>/dev/null || true)" != "nightly-2026-09-17" ]; then
    CHARON_GIT_COMMIT=7b462d9159db2ed3015162352183906a30bfafc6 \
    RUSTUP_TOOLCHAIN=nightly-2026-09-17 \
    CARGO_BUILD_JOBS="$cargo_jobs" \
        cargo build --release --locked --manifest-path "$aeneas_dir/charon/charon/Cargo.toml"
fi
export OPAMROOT="$FABRIC_PROOF_TOOLS/opam-root"
opam_lock_hash=$(sha256sum "$repo_root/verification/aeneas.opam.lock" | awk '{print $1}')
opam_stamp="$OPAMROOT/.aeneas-opam-lock.sha256"
aeneas_bin="$aeneas_dir/src/_build/default/main.exe"
need_aeneas=0
if [ ! -x "$aeneas_bin" ] \
    || ! "$aeneas_bin" -version 2>/dev/null | grep -Fq "ab34bd28458a08c150b7cbf1873e59af74588138"; then
    need_aeneas=1
fi
if [ ! -f "$OPAMROOT/config" ]; then
    opam init --bare --no-setup --yes
    need_aeneas=1
fi
if ! opam switch list --short | grep -Fxq "$aeneas_dir"; then
    opam switch create "$aeneas_dir" ocaml-base-compiler.5.3.0 --yes
    need_aeneas=1
fi
if [ "$need_aeneas" -eq 0 ] \
    && [ ! -f "$opam_stamp" ] \
    && [ -x "$aeneas_bin" ] \
    && [ -f "$OPAMROOT/config" ] \
    && opam switch list --short | grep -Fxq "$aeneas_dir"; then
    printf '%s\n' "$opam_lock_hash" > "$opam_stamp"
fi
if [ ! -f "$opam_stamp" ] || [ "$(cat "$opam_stamp")" != "$opam_lock_hash" ]; then
    opam switch import "$repo_root/verification/aeneas.opam.lock" --switch "$aeneas_dir" --yes
    printf '%s\n' "$opam_lock_hash" > "$opam_stamp"
    need_aeneas=1
fi
if [ "$need_aeneas" -eq 1 ]; then
    make -C "$aeneas_dir/charon" charon-ml/src/CharonVersion.ml
    # Aeneas vendors the Charon OCaml sources through its source directory.
    [ -e "$aeneas_dir/src/charon" ] || ln -s ../charon "$aeneas_dir/src/charon"
    (
        cd "$aeneas_dir/src"
        AENEAS_VERSION=ab34bd28458a08c150b7cbf1873e59af74588138 \
            opam exec --switch "$aeneas_dir" -- dune build -j "$cargo_jobs" main.exe
    )
fi
if ! (
    cd "$aeneas_dir/backends/lean"
    printf 'import Aeneas\n' | "$lake_bin" env lean --stdin >/dev/null 2>&1
); then
    (
        cd "$aeneas_dir/backends/lean"
        "$lake_bin" exe cache get
        LEAN_NUM_THREADS="$lean_threads" "$lake_bin" build Aeneas
    )
fi
