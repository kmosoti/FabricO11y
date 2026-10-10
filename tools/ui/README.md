# Console browser checks

Run these tools from the repository root through the resource launcher. Build
artifacts, browser tools, profiles and server fixtures stay on the mounted data
drive. Run one browser fixture at a time; coordinate Cargo commands with other
workspace checks.

```sh
python3 tools/resource_group.py -- python3 -B tools/ui/install_browser.py
python3 tools/resource_group.py -- python3 -B tools/ui/build.py \
  --out /run/media/kmosoti/data/FabricO11y/results/console-build
python3 tools/resource_group.py -- python3 -B tools/ui/live_browser_check.py \
  --build /run/media/kmosoti/data/FabricO11y/results/console-build \
  --server /run/media/kmosoti/data/FabricO11y/cargo/debug/fabric-server \
  --spindle /run/media/kmosoti/data/FabricO11y/cargo/debug/fabric-node \
  --out /run/media/kmosoti/data/FabricO11y/results/console-browser
```

Output directories must be fresh. Configure `CARGO_HOME` and `RUSTUP_HOME` to
the data-drive toolchain cache before building. Build the native binaries from
the same candidate revision and retain their build receipts. Before launch the
fixture copies both executables into its bounded owned scratch and requires
their hashes to match the initial receipt. Restarts and recovery use those
immutable copies; a concurrent build cannot replace the running candidate.
The fixture uses
the production TLS `serve` command, its supervised dedicated Spindle and a second
enrolled native Spindle. Packaged assets come from `stage_console.py`, which checks
the frozen build receipt against source and asset hashes.

For a final package run, point `--server` and `--spindle` at its extracted
binaries and add `--packaged-console` with its extracted console directory.
The harness requires the exact asset inventory and bytes, plus matching header
and manifest metadata, against the frozen build before serving those package
bytes. The receipt retains this cross-check and binary identities.

The default browser is Chrome for Testing 155.0.8059.39 with its matching driver.
The installer verifies the reviewed fixed archive sizes and SHA-256 hashes before
extracting them, and records their official download URLs. Those pins do not
establish an independent upstream signature. The isolated Chrome profile trusts only the
fixture leaf certificate's exact SPKI. No browser sandbox exemption is supplied.

`--browser firefox` uses installed Firefox 157.0, the recorded geckodriver 0.37.1
at the data-drive tools path, and `--certutil PATH` (default: the installed tool,
otherwise the owned extracted copy). The local extracted copy resides at
`tools/ui/nss-tools-3.129.0-1.fc44/usr/bin/certutil` under the same data root.
The matching NSS tool was obtained from Fedora's
[official Koji archive](https://kojipkgs.fedoraproject.org/packages/nss/3.129.0/1.fc44/x86_64/nss-tools-3.129.0-1.fc44.x86_64.rpm).
The harness creates an owned NSS SQL database and trusts its fixture CA only in
that temporary profile, with `acceptInsecureCerts=false`. It does not install
system trust or packages. Firefox's
[virtual authenticator interface](https://firefox-source-docs.mozilla.org/python/marionette_driver.html#marionette-driver-webauthn-module)
and Chrome's WebAuthn protocol emulate separate authenticators with resident-key
capability and user verification. Firefox uses CTAP 2.1; Chrome uses CTAP 2.
Adding another passkey uses another
authenticator because `excludeCredentials` should reject the original one.
Geckodriver 0.37.1's [field naming defect](https://github.com/mozilla/geckodriver/issues/2239)
prevents its ordinary virtual-authenticator endpoint from preserving the requested
verification flags. The Firefox fixture enables privileged driver automation,
invokes Gecko's own Marionette WebAuthn module with the correct options, then
restores content context. The real application still calls `navigator.credentials`
and receives the same server validation. Receipts label this workaround; they do
not establish interoperability of the affected driver endpoint.
The Firefox fixture explicitly enables the software manager and disables the
physical USB manager, recording actual preference readback. Pinned Firefox 157
[dispatch](https://github.com/mozilla-firefox/firefox/blob/FIREFOX_157_0_RELEASE/dom/webauthn/authrs_bridge/src/lib.rs#L939)
otherwise selects USB before the software manager containing the virtual device.
Earlier deadline failures with zero credentials remain failed.

The corrected transport selected real owner registration and enrollment in the
bounded `console-live-firefox-usb-selection-04` diagnostic. Fresh login later
failed with CTAP 2.0. The observation-only `console-live-firefox-allowlist-05`
confirmed two allowed server keys and one matching active virtual credential,
with the correct RP and required UV. The pinned
[test token](https://github.com/mozilla-firefox/firefox/blob/FIREFOX_157_0_RELEASE/dom/webauthn/authrs_bridge/src/test_token.rs#L418)
omits the credential ID after filtering to one key, whereas its
[bridge](https://github.com/mozilla-firefox/firefox/blob/FIREFOX_157_0_RELEASE/dom/webauthn/authrs_bridge/src/lib.rs#L1139)
restores it only for an originally single-key request. The otherwise unchanged
CTAP 2.1 control `console-live-firefox-ctap21-selection-06` completed 127 finite
assertions on the exact UI09 package: exit 0, 29.548 seconds, 808.7 MiB peak,
no swap and owned-fixture cleanup. These concurrent causal diagnostics do not
fill standalone Firefox acceptance or establish a physical-authenticator defect.

The prospective Firefox fixture is registered separately in
[console protocol revision 3](../../docs/experiments/formal/console-access-protocol.md).
Actual resident-credential coverage remains unestablished: the observed
production enrollment requested `residentKey=discouraged`, and the stored
Firefox credential was nonresident despite the enabled capability. Keep required
UV and the original acceptance assertions; the shell-only check cannot establish
live Firefox passkey acceptance.

The harness verifies a stricter combined 4,000,000,000-byte memory cap (rounded
down to a kernel page), zero swap, two CPU equivalents and 512 tasks before
launching fixtures. Its fixture storage ceiling is 512 MiB. The primary native
producer has a 32 MiB Spool allowance so the finite cursor-expiry wait does not
exhaust its telemetry buffer; the scoped-query witness first checks that this
producer remains alive and ACKs the new independent observations. The launcher retains
the finite outer deadline; browser waits also have a bounded overall budget.
Receipts include the effective cgroup limits, process and cgroup samples, source
and binary hashes, checks, screenshots, failures and cleanup. Cleanup stops owned
process groups and removes the profile, keys, tokens and state. It preserves
evidence without archiving browser cookies or credentials.

Run the independent cache negative control with a fresh output path and
`--inject-api-cache-defect`. It changes only the owned staged worker and matching
staged asset hash. The required failure is `worker caches only public shell
paths`; an earlier setup failure is inconclusive for that property. The canonical
build and registered acceptance expectations stay unchanged.

`--inject-rejection-defect error` replaces one witnessed permission rejection's
error text; `status` replaces its status with 429. Both must fail `first visitor
cannot claim owner without protected bootstrap`. They verify that an unrelated
failure cannot satisfy a permission assertion. Normal fixture API calls are
paced below the server's verification budget; capacity failures are never
accepted as permission evidence.

Use `--pressure --poll-seconds 120` for the finite timeout and polling probes.
Two valid partial authenticated query bodies occupy the actual query slots;
the fixture requires query admission rejection, unauthorized credential rejection,
the 15-second body deadline, and subsequent recovery. The polling probe appends
independent log events for two minutes and records visible rows, UTF-8 bytes and
resource samples. These are bounded observations, not a general capacity benchmark.

The ordinary run also compares recorded trace geometry and integer timestamps
with an independent OTLP producer, uses two distinct native sources to check
scope isolation, and opens a real background tab to verify hidden-tail polling
stops and resumes. Held real query responses exercise transport cancellation,
immediate local logout and serialized server logout. A narrowly invited human
checks the granted row/window caps and local rejection before a network request.

`--expired-cursor-probe` retains a real continuation for at least 902 seconds,
then requires the exact expiry rejection and an adjacent authorized positive
request. It grants this finite fixture a 20-minute internal deadline, within
the resource launcher's 30-minute limit. This option must use matching final
binary and console asset identities; an idle wait does not establish throughput
or capacity.

`--recovery` stops the owned server, runs its actual offline owner recovery
command, verifies prior workload rejection, and enrolls the same owner through
real WebAuthn. `--shell-lifecycle` exercises failed worker installation, a waiting
candidate with an open tab, activation after actually closing the controlled tab,
and rollback. Navigating away and immediately back is not used as proof that
the controlled client closed. Its staged variants keep
the same application assets and change cache identity; they do not establish an
application schema migration. The same option installs, opens and uninstalls a
desktop PWA in the owned profile and XDG directories. Chrome uses a private
debugging pipe because Chromium restricts the PWA automation domain to that
trusted fixture transport. This does not change application authentication or
certificate trust.
The fixture sets the browser's actual standalone preference through
[PWA.changeAppUserSettings](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/json/browser_protocol.json)
before launching, then independently checks `display-mode: standalone` and the
locked public shell in that app window. The preference is controlled by the
browser and is distinct from the manifest's requested display mode.

Each receipt establishes only its recorded assertions. The broader
[console acceptance protocol](../../docs/experiments/formal/console-access-protocol.md)
contains additional cells. A desktop tab with virtual passkeys does not establish
physical or synced passkeys, mobile behavior, other installed-PWA platforms,
operating system trust deployment or release qualification.
