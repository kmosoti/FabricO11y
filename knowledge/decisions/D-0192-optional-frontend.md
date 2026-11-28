# D-0192 · Headless defaults and an optional frontend

The headless deployment and typed Rust client remain supported. This decision
supersedes the product-wide UI prohibition in [D-0002](D-0002-cli-pivot-and-converged-telemetry-engine.md).
[D-0190](D-0190-zero-ui-fabric-client.md) continues to govern the headless client.

The frontend architecture keeps a target-independent `fabric-app-core` for
shared state, reducers, and transformations. It cannot depend on DOM, GPU,
OS, transport, or async runtime APIs. `fabric-ui-dom` owns Leptos components and
accessible DOM interactions. A GPU renderer and a thin Tauri desktop root are
proposed, not qualified implementations. Frontends
reuse backend authorization and data contracts. Package installation is
explicit and starts no collectors or listeners by default.

The repository pins Rust 1.100.0. Before adoption, qualify the exact Leptos,
wgpu and Tauri versions with that toolchain, the supported WebView engines, and
native and wasm targets. Run identical reducer fixtures on native and wasm.
Prove the headless default dependency graph excludes frontend renderers and
repeat existing headless and custody qualifications. The frontend crate and
artifact qualifications are separate from this boundary decision.

[Tauri's frontend configuration](https://v2.tauri.app/start/frontend/)
documents the static asset and WebView integration boundary.

## Qualification profile addendum (2026-11-27)

## Build and authority boundary

The proposed frontend uses a separate Cargo workspace and lockfile, excluded
from the root workspace. Its web, desktop and GPU profiles must be explicitly
selected and packaged when qualified; the backend, CLI, MCP and typed client remain headless
by default. Root `--workspace` builds, checks and packages must continue to
operate without frontend renderers. `default-members` alone does not provide
that isolation. Bind both lockfiles, source revisions, enabled features and
targets to the artifact qualification. The existing architecture check examines
the locked normal and build dependency graph of the declared headless roots;
it does not establish frontend package behavior or all target closures.

The shared core uses versioned operation DTOs and a transport-free port from
the authorized backend contracts. Browser and desktop adapters do not grant
permissions or infer trusted identity from caller-supplied fields. They rely on
backend authorization and adapter-established identity. `fabric-client`'s
native `cli` feature is not a browser transport. The shared frontend core and
its real reducer belong to the frontend implementation; artifact profiles and
release qualification are separate work. This decision itself does not create
or qualify either.

## Implementation status

The optional workspace contains the Leptos 0.8.23 browser source and shared
reducer; a separate native workspace contains a private Unix query gateway.
A develop preflight can archive the static browser build without wasm-opt. It
is not a release package, desktop profile, authenticated proxy or runtime
qualification. The browser smoke pins Trunk 0.22.1 and wasm-bindgen 0.2.130;
other framework and platform profiles remain proposals until qualified.

## Qualification before adoption

Product code and tools use the repository's Rust 1.100.0 pin. Proposed framework
versions to evaluate are Leptos 0.8.23, wgpu 30.0.2 and Tauri 2.12.7; declared
upstream minimum versions are candidate-selection evidence, not integrated
qualification. Lock and build the exact selected crates, features and target
profiles with that pin. For a Trunk 0.22.2 browser build, bind a matching
wasm-bindgen crate and CLI explicitly; verify the actual selected versions and
preprovisioned tool checksums. Qualify Tauri's WebKitGTK and native build
requirements on each supported package target, including any Nix CLI override.
See the upstream [Leptos manifest](https://docs.rs/crate/leptos/0.8.23/source/Cargo.toml),
[wgpu manifest](https://github.com/gfx-rs/wgpu/blob/v30.0.2/wgpu/Cargo.toml),
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) and
[Trunk configuration](https://github.com/trunk-rs/trunk/blob/v0.22.2/guide/src/configuration/index.md).

Before claiming a profile works:

- Check the locked native and wasm normal and build dependency closures for
  the frontend and repeat the headless graph, package, CLI and custody checks.
- Run the same fixture against the actual shared reducer and a real typed
  operation DTO on native and wasm; a standalone mock does not establish parity.
- Exercise packaged browser and desktop behavior on supported targets, including
  authorization, startup without collectors or listeners, accessible interaction,
  renderer fallback and actual rendering. Do not infer WebGPU support in a
  WebView from upstream package versions; verify the selected rendering path.
- Qualify artifact profiles and release evidence independently from this policy
  and from the shared core implementation. Record exact source, locks, targets,
  features and test results. Unexecuted qualifications remain open.
