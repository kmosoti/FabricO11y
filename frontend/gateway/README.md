# Optional private query adapter

This native process serves one scoped query provision on a private Unix socket
for at most 15 minutes. It requires a real, owned mode-0700 parent directory,
creates a mode-0600 socket, and accepts only the process's own UID according
to kernel peer credentials. This is a same-user trust boundary: every process
with that UID is trusted. It is not a browser authentication mechanism.

An independently managed same-origin HTTPS proxy must authenticate one scoped
holder, strip untrusted identity headers, prevent caching, and forward only to
this socket. Configure the exact external Host and Origin. The proxy must not
expose the socket to untrusted local users or add broad CORS. The gateway accepts
only POST `/api/v1/query`; it has four connection slots, closes each connection
after one request, and applies a 15-second connection deadline. Request scope
must be a subset of the independently configured authority and origins. The
canonical credential remains in its owner-provisioned file and is checked by
the authenticated canonical client. Backend errors are not returned verbatim.

Arguments, in order: socket path, loopback canonical endpoint, canonical
credential file, expected authority, external Host including any port, matching
HTTPS Origin, then one or more source origins. The socket parent must exist.
A service manager should restart the process only after reauthorizing the holder;
expiry is process lifetime, not browser-session revocation.

Build and test the optional gateway with the Rust toolchain pinned by the
repository:

```sh
cargo build --locked --manifest-path frontend/gateway/Cargo.toml
cargo test --locked --manifest-path frontend/gateway/Cargo.toml
```

The output binary is `fabric-ui-gateway`. The default native packages do not
build, install, or start this adapter.
