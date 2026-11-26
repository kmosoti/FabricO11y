# Offline feature and verification guide

Use the guide to find commands, expected results, and the checks that cover each workflow.

```sh
fabric guide
fabric guide local --table
fabric guide custody --json
fabric guide fleet --json
fabric guide query --json
fabric guide receipts --json
fabric guide settings --json
```

The guide uses an embedded catalog and runs without a daemon, network, or data
directory. Each scenario names declared executable checks. Proof entries identify
the Lean theorem, whether it uses extracted Rust or an independent model, the
proved property, the assumptions, and the command to reproduce the check.

A successful `cargo xtask candidate verify CAPTURE` retains `guide-evidence.json`
in the candidate evidence directory beside the capture. Inspect the recorded
outcomes with an absolute or relative path to that file.

```sh
fabric guide fleet --evidence /data/verification/evidence-CANDIDATE/guide-evidence.json --json
```

The report contains the recorded revision, catalog and coverage digests,
checker identities, artifact references, and named outcomes. A build with a
known source commit can set `FABRIC_BUILD_REVISION` to its full lowercase commit
ID at compile time. The CLI then compares the record with that build revision
and with its own embedded catalog digest. Builds without a source commit report
an unknown revision match. A supplied evidence file is a record for inspection,
and its contents require independent verification against the candidate
artifacts.

The [guide catalog](../verification/guide.json) is also the source for the wiki
topic index. To preview the index from a workspace matching a reviewed commit,
run `cargo xtask guide wiki REVISION`. The rendered links pin each document and
proof reference to that revision.

## Approximate percentiles in local queries

Use `fabric query` to estimate p50, p90, p95, or p99 for a numeric field in
the selected domain. Both `p95:duration_ms` and `p95(duration_ms)` produce
the field `p95_duration_ms`.

```sh
fabric query traces --since 1h --group-by service \
  --agg 'count,p50:duration_ms,p95(duration_ms),p99:duration_ms' --json
```

The engine uses t-digest and returns floating-point estimates. Large integer
inputs may be rounded when converted to floating point. Null inputs are ignored.
A percentile is null when its group has no non-null inputs, or when the target
field in that group contains NaN, infinity, or a floating-point value with
magnitude above `1e284`. Other aggregates in the same query use their usual inputs.

Each matching query row contributes one value. Historical metrics may have
been retained as five-minute bucket means. Each visible bucket mean contributes
one value alongside any matching raw rows. Means and counts cannot recover
the original percentiles of the raw observations. Query the underlying rows
together for a combined percentile, since averaging separate percentile values
cannot recover it.
