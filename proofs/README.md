# Proof development

The files in this directory are independent admission candidates, not proofs of
the `fabric` crate. `Admission.lean` models natural-number byte credits.
`admission.rs` is a Verus-checked executable candidate using machine-sized
integers. Neither models allocations, queue ownership, concurrency, I/O, or
the current SDK implementation. Integrating a candidate requires a separate
review of its interface and evidence connecting the checked code to the code
actually executed.

## Checkers

The following commands use Linux x86-64 release archives. Keep the unpacked
archives in a directory of your choice; the examples use `$proof_tools`.

```sh
proof_tools="$HOME/.local/share/fabrico11y/proof-tools"
mkdir -p "$proof_tools/downloads"
gh release download v4.35.0 --repo leanprover/lean4 \
  --pattern lean-4.35.0-linux.tar.zst --dir "$proof_tools/downloads"
gh release download release/0.2026.10.26.d17fbd0 --repo verus-lang/verus \
  --pattern verus-0.2026.10.26.d17fbd0-x86-linux.zip --dir "$proof_tools/downloads"
```

Verify the downloaded files before extracting them:

```text
7289597a44543246c9227b8cae501767e095ce17f6d448b95fc4a8dc97268f6d  lean-4.35.0-linux.tar.zst
361674d05f0c1b2c9c3f4d5bc56a8fb2ab9241ed0cceca27780668c292bb4940  verus-0.2026.10.26.d17fbd0-x86-linux.zip
```

Use `sha256sum` on both archives and compare with the values above, then extract:

```sh
tar --zstd -xf "$proof_tools/downloads/lean-4.35.0-linux.tar.zst" -C "$proof_tools"
unzip "$proof_tools/downloads/verus-0.2026.10.26.d17fbd0-x86-linux.zip" -d "$proof_tools/verus-0.2026.10.26.d17fbd0"
rustup toolchain install 1.99.1-x86_64-unknown-linux-gnu
```

From the repository root, run the pinned checkers without changing the project's
Rust toolchain:

```sh
"$proof_tools/lean-4.35.0-linux/bin/lean" -E warning proofs/Admission.lean
"$proof_tools/verus-0.2026.10.26.d17fbd0/verus-x86-linux/verus" \
  --no-cheating --num-threads 2 proofs/admission.rs
```

Alternatively, run the pinned-version checks together with `xtask`:

```sh
FABRIC_LEAN="$proof_tools/lean-4.35.0-linux/bin/lean" \
FABRIC_VERUS="$proof_tools/verus-0.2026.10.26.d17fbd0/verus-x86-linux/verus" \
  cargo xtask prove
```

This command checks the two named theorem reports against the current allowlist;
new or renamed claims require updating the audit as well as the source.

Lean prints each theorem's axiom dependencies. Review the output: the current
allowlist is `propext` and `Quot.sound`; `sorryAx` or any other dependency is a
failure. Verus's `--no-cheating` rejects assumptions and external bodies in the
candidate. Checker success does not automatically establish either toolchain's
correctness or a relationship between the two files.

For any new claim, state its scope, assumptions and excluded behavior; check the
exact artifact and tool versions; inspect the axiom and trusted-code boundary;
and run a negative control that violates the property. Keep proof models and
implementation contracts distinct, and do not infer system guarantees from a
standalone model. Aeneas/Charon extraction is not yet configured.
