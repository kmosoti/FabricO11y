# D-0192 · Headless defaults and an optional frontend

The headless deployment and typed Rust client remain supported. This decision
supersedes the product-wide UI prohibition in [D-0002](D-0002-cli-pivot-and-converged-telemetry-engine.md).
[D-0190](D-0190-zero-ui-fabric-client.md) continues to govern the headless client.

The frontend architecture keeps a target-independent `fabric-app-core` for
shared state, reducers, and transformations. It cannot depend on DOM, GPU,
OS, transport, or async runtime APIs. `fabric-ui-dom` owns Leptos components and
accessible DOM interactions. `fabric-ui-gpu` owns wgpu rendering. Thin web and
Tauri desktop roots wire platform, rendering and typed transport. Frontends
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
