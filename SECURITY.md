# Security Policy & Trust Boundaries

FabricO11y is designed as a single-user / host-local observability custody plane with strict privilege separation between untrusted telemetry inputs, authenticated loopback custody RPCs, and local filesystem/engine storage.

## Supported Versions

Security fixes land on `main` and are released on the current `v0.1.x` alpha line (`0.1.1-alpha.1`).

## Reporting a Vulnerability

Do not open public issues with exploit details for unpatched vulnerabilities. Report issues privately via GitHub Security Advisories on `kmosoti/FabricO11y`.

## Trust Boundaries & Mitigations (`v0.1.1-alpha.1`)

1. **Loopback Canonical RPC Mutual Authentication (`fabric`, `fabric-client`, `fabric-server`, `spindle`)**
   - `fabric-server` canonical TCP uses a 4-frame mutual HMAC-SHA256 challenge-response handshake (`ClientHello` -> `ServerChallenge` -> `ClientProof` -> authenticated `Request` with `request_mac`).
   - Raw bearer credential bytes are never transmitted on TCP; clients verify the server's sorted challenge proof over fresh 256-bit client/server nonces before proving possession or transmitting telemetry/requests.
   - Pre-authentication sockets are isolated in a bounded non-blocking `rustix::event::poll` gate with a `250ms` handshake deadline so unauthenticated Slowloris peers cannot exhaust worker threads.

2. **Authenticated Storage Engine Transport (`fabric-server`)**
   - When `engine_endpoint` (GreptimeDB loopback `/v1/sql`) is configured, `Server::open` and `Greptime::open_with_auth` fail closed unless `engine_auth_token_file` is supplied and attaches `Authorization: Bearer <token>`, or `allow_unauthenticated_loopback_engine = true` is explicitly opted into for local unauthenticated dev instances.

3. **TOCTOU-Safe Credential & State I/O (`fabric`, `fabric-server`, `fabric-store`, `fabric-query`)**
   - Credential and engine token files are opened once with `O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`, validated on the opened file descriptor (`fstat`: regular file, UID match, `0o600` private mode for custody services, bounded `<= 256`/`4096` bytes), and read from that same descriptor.
   - Custody live state (`write_live_state_atomic`), SQLite/OLAP directories, and projection manifests anchor parent directories via `OwnedFd` / `openat(..., O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)`, verify `(dev, ino)` across staging and target directories, publish atomically with `renameat`, and `fsync` directory descriptors.

4. **Strict Custody & Local Socket Permissions (`fabric`, `fabric-collect`, `fabric-query`, `fabric-server`, `fabric-store`)**
   - Custody directories enforce `0o700` and custody databases, locks, state files, Parquet segments, and local Unix domain sockets (`daemon.sock`, `ingest.sock`, `spindle.sock`) enforce `0o600` via explicit `fchmod` / permissions regardless of ambient `umask`.
   - Local UDS RPC and OTLP/ingest listeners verify `SO_PEERCRED` peer UID equality on `accept()`.

5. **Fair-Share Per-Origin Storage Quotas (`fabric-server`, `fabric-query`)**
   - Enrolled nodes omitted from `node_quota_bytes` receive a bounded per-origin fair-share ceiling in both canonical storage (`Ledger`) and analytical Parquet storage (`Projection`), preventing any single origin from starving peers.

6. **Untrusted Telemetry Isolation (`fabric`, `fabric-mcp`, `fabric-cli`, `fabric-query`, `fabric-collect`)**
   - MCP tools (`fabric-mcp`) enclose canonical telemetry payloads inside explicit metadata framing (`"untrusted_telemetry": true`, `"trust_boundary": "untrusted_observed_data"`) and escape C0/DEL/ESC/C1/bidi controls and LLM/prompt delimiter sequences (`<|`, `</`, `<<<`, `>>>`, ```` ``` ````, `[INST]`, `<system>`, etc.).
   - Human/table CLI renderers and error printers sanitize terminal control sequences (C0, DEL, ESC/CSI/OSC, C1, bidi) via `fabric_core::sanitize_terminal_text` before column truncation.
   - `FsChurnTracker` (`fabric-collect`) evicts disappeared paths each scan cycle, caps retained tracking entries (`MAX_TRACKED_PATHS = 4096`), and drops cached path state under memory pressure.

7. **Release Provenance & Supply Chain (`release.yml`, `install-toolchain.sh`)**
   - CI/release toolchains verify pinned versioned `rustup-init` binary SHA-256 digests instead of piping unverified shell scripts.
   - Tagged releases verify protected-`main` ancestry (`git merge-base --is-ancestor "${GITHUB_SHA}" refs/remotes/origin/main`) fail-closed before packaging, attestation, or release publication, with all GitHub Actions pinned to 40-character commit SHAs.
