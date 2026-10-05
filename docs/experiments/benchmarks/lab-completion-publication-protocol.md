# Query during an in-progress build

Registered before execution as Q1 transition evidence in the
[completion campaign](lab-completion-protocol.md). The owned eight-group fixture
has log, metric, span and explicit-gap payloads and a retained pre-build ledger.
Pause an actual child builder with SIGSTOP at its manifest-sync call before
publication, using the scoped syscall interposer. Require an injection trace,
observed stopped process and no published Segment. Drain complete log, selective
log, span and rate answers under Scan and Walk against the unchanged independent
oracle while the build is stopped. Resume the same child, require successful
publication, terminate its idle post-publication helper, and repeat exact chains.

This barrier establishes that queries can read pending journals during an
unfinished build and across publication. It does not measure natural overlap
frequency, query latency during CPU contention, or ACK-to-queryable timing. The
live native and lifecycle experiments own those measurements. Loss, duplicated
rows and incomplete chains remain oracle failures. Preserve failure state and
terminate the owned process tree through the outer cgroup.
