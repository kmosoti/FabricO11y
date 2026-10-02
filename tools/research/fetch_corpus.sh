#!/bin/bash
# Fetch the eight Loghub 2,000-line samples used as real log text and check
# them against the recorded digests, into $FABRIC_RESEARCH_ROOT/corpus.
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT="${FABRIC_RESEARCH_ROOT:-$PWD/target/research}"
mkdir -p "$ROOT/corpus"; cd "$ROOT/corpus"
for n in Apache/Apache_2k.log HDFS/HDFS_2k.log Linux/Linux_2k.log OpenSSH/OpenSSH_2k.log Zookeeper/Zookeeper_2k.log Hadoop/Hadoop_2k.log Spark/Spark_2k.log Thunderbird/Thunderbird_2k.log; do
  f=$(basename "$n")
  [ -s "$f" ] || curl -sS -L --max-time 120 -o "$f" "https://raw.githubusercontent.com/logpai/loghub/master/$n"
done
sha256sum -c "$OLDPWD/tools/research/corpus.sha256"
cat ./*_2k.log > all.log
echo "corpus: $(wc -l < all.log) lines in $ROOT/corpus/all.log"
