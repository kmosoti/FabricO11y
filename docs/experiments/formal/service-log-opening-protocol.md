# Registered service-compatible log opening

Revision 1, registered 2026-10-12 before the repair checks. This amends the
mechanism in section 7 of the historical
[security change specification](../security-hardening/SECURITY-CHANGE-SPEC.md).
Its refusal policy and F01–F10 acceptance requirements remain in force.

## Origin and invariant

Frozen `ca84d9823b5e3f8f39f40647cedffef0acff3aac` failed Debian lifecycle
`rel-deb-life-01`: the dedicated Spindle's log preflight returned `ENOSYS`, then
the server exited. The [retained installation failure](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-installation-release-run-02)
records the package hashes, commands, receipts and minimized guest controls.
With systemd 257, `RestrictSUIDSGID=yes` denies `openat2` with `ENOSYS`;
changing only that directive to `no` allowed the same syscall on the same file.
The directive remains required by the product contract.

All three entrypoints (backlog, data reader and companion preflight) must use
one checked regular-file descriptor. Refuse every symlink, including parent
and magic links, before reading its target; preserve cursor and gap semantics.
Never canonicalize, check a path then reopen it, use a leaf-only policy, or
retry a denied secure operation with an ordinary opener.

## Amended mechanism and assumptions

Use `openat` for a descriptor-relative component walk anchored at `/`.
An owned root descriptor and each intermediate descriptor use `O_PATH`,
`O_CLOEXEC` and `O_NOFOLLOW`. Inspect the descriptor itself: a symlink returns
`ELOOP`, another nondirectory returns `ENOTDIR`. Each next lookup uses that
checked directory descriptor and one basename, never a reconstructed path.
Open the leaf read-only with `O_CLOEXEC | O_NONBLOCK | O_NOFOLLOW`, then check
that its descriptor is regular before reading. All syscall/metadata errors
propagate; this is the primary mechanism, not an `openat2` fallback.

Require absolute, NUL-free paths of at most 4096 bytes without `..` components.
Reject trailing directory syntax (`/` or `/.`) rather than normalize a
non-file selection into a readable file. At most two descriptors coexist during
the walk; Rust `File` ownership closes them on success and all errors.
A directory renamed after it is opened remains pinned for the next lookup.
Regular-file rotation stays supported. Trusted source configuration, Unix
permissions and a trusted OS remain assumptions: this policy does not
authenticate content, reject hard links, confine mount changes or establish
a directory sandbox.

## Registered checks

1. Run a regression in a separate test process with a seccomp filter returning
   `ENOSYS` for `openat2`. Its retained old `openat2` call must actually fail
   with that errno (negative control). The production opener, backlog and
   data reader must still return the exact approved bytes/cursor; leaf and
   parent symlinks and `/proc` aliases must still fail. The old implementation
   must fail this regression before the replacement is accepted.
2. Keep all existing refusal, nonblocking FIFO, descriptor replacement and
   cursor assertions. Inject `ENOSYS`, `EPERM`, `EINVAL` and `ELOOP` at root,
   intermediate and leaf operations: propagate the exact error and stop
   without a fallback. Reject invalid and trailing-directory paths before
   any syscall. Pin an intermediate directory, replace its pathname with a
   symlink to a sentinel, and require the original bytes through the pinned
   descriptor; a subsequent fresh open must reject the replacement.
3. Run focused security checks, the fast registry and documentation checks
   under the resource launcher on the mounted data drive. Retain command,
   revision/tree, exit, cgroup/storage observations and cleanup receipts.
   Default 30-minute/20 GiB/no-swap limits apply; no uncapped fallback.

Native filter injection is a regression, not installed-system acceptance.
After owner merge, freeze and rebuild a new exact candidate and repeat #41's
Debian/Fedora cells with unchanged service hardening. Historical package and
extended evidence cannot qualify the changed implementation. No result or
release authorization is recorded by this protocol.
