# Offline feature and verification guide

Use the guide to find commands, expected results, and the checks that cover each workflow.

```sh
fabric guide
fabric guide local --table
fabric guide custody --json
fabric guide fleet --json
fabric guide query --json
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
