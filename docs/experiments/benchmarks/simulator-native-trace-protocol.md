# Initial-thread native publication observation

Prospective registration; source/native commands remain unrun. This supplements
[native witness controls](simulator-native-witness-protocol.md), whose eight
finite diagnostics do not certify atomic publication.

Trace only the helper's original ELF thread, without `-f`. The pinned helper
calls `BeginWitness::publish` synchronously before its first thread spawn;
offers originate only in spawned workers. Raw trace order must establish a
complete stage write, 0400 mode, file fsync, no-replace hard link, owned stage
unlink and directory fsync before any clone/clone3 entry. An earlier clone,
unknown/missing/failed operation, partial write, unexpected destination or
remaining second link refuses certification. Buffer-flushed events alone do
not establish offer order. No wall timestamp orders concurrent workers.

The live controller holds and hashes the helper ELF descriptor, discovers the
single actual child of its owned tracer, binds loaded ELF device/inode/hash,
PID/start/boot and pidfd, and retains the tracer separately. A diagnostic-only
100 ms first-execve exit delay gives this binding a finite bootstrap window;
it precedes the helper's native begin clocks and is not an elapsed workload
shortcut. The signed prepared strace binary/version/manuals and exact options
must be frozen before execution. Unsupported options or missing live binding
are inconclusive, never success. No preloading, host clock change, privilege or
product/helper byte change is permitted.

Raw single-thread trace bytes and offsets, actual target identity, actual target
exit and separate tracer exit are retained. A complete positive join also
requires the unchanged live/final begin descriptor observation and native TLS
diagnostic controller. The offline join receives independently pinned trace and
observer-receipt hashes, actual expected process/helper/begin hashes, and reads
the raw trace again. It cannot infer its own expected hashes from its receipt.

Register literal source controls for complete order and rejection of absent or
reordered fsync/link/unlink, short write, missing exclusive/no-follow open, wrong
mode/destination/inode and clone-before-publication. Register a real positive
trace first. Freeze its exact syscall ordinals/path/FD evidence before injecting
write, file-fsync, hard-link and unlink EIO separately. A negative is credited
only if the actual raw injected operation is the intended witness operation,
the actual helper refuses success and no worker clone/offer precedes refusal.
Unrelated startup errors and a wrapper's generic refusal are inconclusive.
Existing destination and precreated stage/symlink controls must preserve exact
victim bytes; a transient two-link begin after injected unlink failure is not a
completed publication.

Every native control has a fresh DATA result directory, full 1 GiB reservation,
namespace census, default 16/20 GiB/no-swap/1800-second containment and bounded
controller deadline. Tool preparation remains separate and must complete first.
Retain all failed traces and exact raw fixture receipts. Confirm actual helper,
tracer, TLS sink and outer containment stopped before the next cell. These are
finite publication controls, not Fabric durability, S1, arbitrary crash recovery
or product acceptance. Native execution requires its own pinned command preflight.
