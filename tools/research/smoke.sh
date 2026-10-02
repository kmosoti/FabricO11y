#!/bin/bash
# The whole research loop at a size this four-CPU container finishes in a
# few minutes: build the binaries, fetch the corpus, generate a 16 MiB
# real-text tail, run the L-21 harness on it (stock against the walk, the
# shapes, a 60-query differential) and the codec micro-benchmark.
set -euo pipefail
cd "$(dirname "$0")/../.."
export FABRIC_RESEARCH_ROOT="${FABRIC_RESEARCH_ROOT:-$PWD/target/research}"
export FABRIC_RESEARCH_SMOKE=1
bash tools/research/build_prototypes.sh
bash tools/research/fetch_corpus.sh
python3 -B tools/research/harness/l21.py "$FABRIC_RESEARCH_ROOT/data/l21-smoke"
taskset -c 0 "$FABRIC_RESEARCH_ROOT/bin/fobbench" > "$FABRIC_RESEARCH_ROOT/data/fobbench.csv"
python3 -B tools/model/test_observation_model.py
echo "smoke: ok; results under $FABRIC_RESEARCH_ROOT/data"
