#!/bin/sh
# Check the installed runtime with network access disabled by the caller.
set -eu
(cd / && rustc --version) | grep -E '^rustc 1\.100\.0 '
product_components=$(rustup component list --toolchain 1.100.0 --installed)
for component in rustfmt clippy; do
    printf '%s\n' "$product_components" | grep -E "^$component-"
done
jj --version | grep -Fx 'jj 0.46.0-7d382314f9420b4b8d932245b49c99492c812b9f'
"$FABRIC_SCHEMA_TOOLS/weaver-0.28.0" --version | grep -Fx 'weaver 0.28.0'
case "${1:-}" in
    common) ;;
    native)
        tools="$FABRIC_PROOF_TOOLS"
        aeneas="$tools/aeneas-ab34bd2"
        charon="$aeneas/charon/charon/target/release/charon"
        lean="$tools/lean-4.35.0-linux/bin"
        verus="$tools/verus-0.2026.10.26.d17fbd0/verus-x86-linux"
        rustup run 1.99.1 rustc --version | grep -E '^rustc 1\.99\.1 '
        native_components=$(rustup component list --toolchain nightly-2026-09-17 --installed)
        for component in rust-src rustc-dev llvm-tools; do
            printf '%s\n' "$native_components" | grep -E "^$component(-|$)"
        done
        "$charon" version | grep -Fx '0.1.304 (7b462d9159db2ed3015162352183906a30bfafc6)'
        "$charon" toolchain-version | grep -Fx 'nightly-2026-09-17'
        "$aeneas/src/_build/default/main.exe" -version | grep -F 'ab34bd28458a08c150b7cbf1873e59af74588138'
        "$lean/lean" --version | grep -F 'Lean (version 4.35.0,'
        "$verus/verus" --version | grep -F 'Version: 0.2026.10.26.d17fbd0'
        "$verus/z3" --version | grep -Fx 'Z3 version 4.16.0 - 64 bit'
        "$tools/cargo-fuzz-0.13.2/bin/cargo-fuzz" --version | grep -Fx 'cargo-fuzz 0.13.2'
        "$FABRIC_ENGINE_TOOLS/greptime-linux-amd64-v1.2.1/greptime" --version | grep -Fx 'version: 1.2.1'
        lean_path=$(cd "$aeneas/backends/lean" && "$lean/lake" env printenv LEAN_PATH)
        (
            probe=$(mktemp -d "${TMPDIR:-/var/tmp}/fabric-ci-native-probe.XXXXXXXX")
            trap 'rm -rf -- "$probe"' EXIT
            cd "$probe"
            printf 'pub fn identity(x: u64) -> u64 { x }\n' > probe.rs
            RUSTUP_TOOLCHAIN=nightly-2026-09-17 "$charon" rustc --preset aeneas --sysroot default \
                --error-on-warnings --dest-file probe.llbc -- --crate-type lib probe.rs
            "$aeneas/src/_build/default/main.exe" -backend lean -warnings-as-errors \
                -abort-on-error -no-progress-bar -dest "$probe" probe.llbc
            LEAN_PATH="$lean_path" "$lean/lean" -E warning Probe.lean
        )
        ;;
    *) echo 'Usage: check-ci-image-tools.sh common|native' >&2; exit 2 ;;
esac
