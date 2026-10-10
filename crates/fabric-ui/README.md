# Fabric operator console

Rust/Leptos CSR composition root with pure native-testable presentation models.
The current app is an explicit deterministic demonstration: Overview, Explore,
Trace detail, Pipeline, bounded manual Live Tail and Settings. Production access
stays locked until the server implements the registered identity/session APIs.
It never asks for the existing administrator bearer token.

The [algorithm model](../../docs/architecture/console-algorithms.md) defines
request epochs, bounded UTF-8 tail storage, monotonic polling/backoff and exact-time
chart envelopes. The [art-direction flow](../../docs/architecture/console-art-direction.md)
connects tasks and evidence to layout and evaluation; it uses primary HCI research,
WCAG and established design-system guidance. The
[release plan](../../docs/milestones/operator-console.md) owns remaining integrated
workflows and browser/security/accessibility acceptance.

## Build and check

Run from the repository root, using the mounted data drive and resource launcher.
Rust 1.99.0, the `wasm32-unknown-unknown` target, Leptos 0.8.22 and Trunk 0.21.14
are the initial pinned toolchain. `tools/ui/build.py` expects Trunk and its
download/checksum receipt in the data drive's `tools/ui` directory, uses the
data-drive Cargo/Rustup caches, and rasterizes the code-native icon with ImageMagick.
It refuses existing output directories and records asset/source hashes and CSP.

```sh
export CARGO_HOME=/run/media/kmosoti/data/FabricO11y/toolchain-cache/cargo
export RUSTUP_HOME=/run/media/kmosoti/data/FabricO11y/toolchain-cache/rustup
python3 -B tools/resource_group.py -- cargo +1.99.0 test --locked -p fabric-ui
python3 -B tools/resource_group.py -- python3 -B tools/ui/build.py --out /run/media/kmosoti/data/FabricO11y/results/my-console-build
python3 -B tools/resource_group.py -- python3 -B tools/ui/browser_check.py --build /run/media/kmosoti/data/FabricO11y/results/my-console-build --out /run/media/kmosoti/data/FabricO11y/results/my-console-browser-check
```

The Firefox harness uses a pinned geckodriver under the same tool directory.
It starts a loopback-only static fixture server, exercises the actual WASM,
stores screenshots/receipts and removes its browser profile and processes.
Browser acceptance here is finite desktop/narrow-layout evidence, not Android,
iOS, installed-app, real passkey or general release qualification.

Serve the generated `dist` at `/console/` with its recorded response headers.
Serve WASM as `application/wasm`; API/auth routes are never SPA fallbacks.
Production requires trusted HTTPS; loopback is for development. The generated
service worker caches only the explicit public shell asset list. No telemetry,
credentials, API responses or offline actions are stored. Reopening resets the
demonstration. This crate does not yet add asset routes to the production server
or change Linux packages.

The full workspace dependency-policy check currently fails. The stable Leptos
tree includes the unmaintained `paste` macro crate and dependencies whose
licenses are outside the existing allowlist; there are separate existing server
failures. The build is experimental until those are resolved through maintained
dependencies and an explicit license-policy decision. No advisory was ignored.

The public shell is limited to 8 MiB per build. Worker lifecycle keeps an active
and a waiting version (a third can exist during installation); failed fresh
installs remove their cache. These are logical asset bounds, not browser-wide
disk or memory limits. Cache lifecycle across real upgrades still belongs to U4.
