# E1R run 01 evidence

- [Run and interpretation](../../coverage-e1-run-01.md)
- [Command, environment and source hashes](metadata.json)
- [Raw test/compiler output and starting tracked diff](raw-output.json) (lossless JSON strings)
- [Per-corpus counters and totals](summary.json)
- [Protocol frozen before candidates](protocol.txt)
- [Independent oracle hashes](oracle-freeze.json)
- [Candidate A](a.json), [candidate B](b.json), [selection](decision.json)
- [Root-binding mutation](mutation-root-binding.json) and [builder-validation mutation](mutation-builder-validation.json)

These are logical correctness checks, not timing samples. The tracked starting
diff is preserved separately; new source files were untracked during the run and
are identified by hashes and retained in the associated repository change.

Review records: [GPT](review-gpt.txt), [Claude](review-claude.json), and [parent probe reruns](review-probe-reruns.json).
The standalone [GPT probe source](review-gpt-probe.rs.txt) and [Claude probe source](review-claude-probes.rs.txt)
are archived as text. To reproduce a probe, create a scratch Cargo package depending on
`storage-probe` at this repository's `tools/storage-probe` path, copy the GPT source to
`src/main.rs` or the Claude source to `tests/probes.rs`, then run `cargo run --offline` or
`cargo test --offline` with that scratch manifest. The original exact commands and exits are preserved.

[Final verification](verification.json) confirms all 19 measured source hashes and four frozen oracle/protocol hashes, and recomputes the summary from the raw output. [Artifact hashes](artifacts.json) cover the preserved files.
