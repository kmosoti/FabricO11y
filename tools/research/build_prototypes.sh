#!/bin/bash
# Build the research binaries from the repository, reproducibly, without
# touching the product tree: a throwaway git worktree at the current HEAD,
# the research patches applied on top, a release build, and the binaries
# installed under $FABRIC_RESEARCH_ROOT/bin (default target/research/bin).
#
#   bin/stock/fabric-server   the product server at HEAD
#   bin/proto/fabric-server   with the tail index and threshold walk prototypes
#                             (FABRIC_PROTO_TAIL=index, FABRIC_PROTO_TOPK=on)
#   bin/budget/fabric-server  the same binary; FABRIC_PROTO_BUDGET=<rows> adds the boundary
#   bin/sealbench             the sealer-study generator and measurements
#   bin/fobbench              the observation codec micro-benchmark
#
# Usage: bash tools/research/build_prototypes.sh   (about two minutes warm; needs the vendored registry, offline)
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT="${FABRIC_RESEARCH_ROOT:-$PWD/target/research}"
WT="$ROOT/worktree"
mkdir -p "$ROOT/bin/stock" "$ROOT/bin/proto" "$ROOT/bin/budget"
# the stock server, from the product tree as it is
cargo build --release --offline -p fabric-server --bin fabric-server
cp target/release/fabric-server "$ROOT/bin/stock/fabric-server"
# the prototypes, in a worktree so the product tree stays clean
if [ -d "$WT" ]; then git -C "$WT" checkout -q -- . && git -C "$WT" clean -qfd; git worktree remove --force "$WT"; fi
git worktree add -q --detach "$WT" HEAD
(
  cd "$WT"
  git apply ../../../tools/research/patches/query-prototypes.diff
  git apply ../../../tools/research/patches/bench-wiring.diff
  cp ../../../tools/research/patches/sealbench.rs crates/fabric-server/src/sealbench.rs
  mkdir -p crates/fabric-server/examples crates/fabric-observation/examples
  cp ../../../tools/research/patches/sealbench-driver.rs crates/fabric-server/examples/sealbench.rs
  cp ../../../tools/research/patches/fobbench.rs crates/fabric-observation/examples/fobbench.rs
  CARGO_TARGET_DIR="$ROOT/target" cargo build --release --offline -p fabric-server --bin fabric-server --example sealbench
  CARGO_TARGET_DIR="$ROOT/target" cargo build --release --offline -p fabric-observation --example fobbench
)
cp "$ROOT/target/release/fabric-server" "$ROOT/bin/proto/fabric-server"
cp "$ROOT/target/release/fabric-server" "$ROOT/bin/budget/fabric-server"
cp "$ROOT/target/release/examples/sealbench" "$ROOT/bin/sealbench"
cp "$ROOT/target/release/examples/fobbench" "$ROOT/bin/fobbench"
grep -q "topk:" "$ROOT/bin/proto/fabric-server" || { echo "prototype binary lacks the walk" >&2; exit 1; }
echo "research binaries in $ROOT/bin"
