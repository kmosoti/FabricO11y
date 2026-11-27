# Offline CI qualification and retained hosted evidence

The [paired native trial](https://github.com/kmosoti/FabricO11y/actions/runs/44415669269)
used the native OCI digest `sha256:170d813a79e72c8a7f0c74bcc8e969b3efa14983c75498dde5f3bbaa5cbb89f0`
and source `0d14f2c517ed1a0a49d2445be77cda9c9e3bf796`. It completed three
cold pairs. The Fedora median was 49m35.42s and the candidate median was
33m02.81s including anonymous image transfer. The first complete warm pair
took 12m28.37s and 11m49.02s respectively. Both sides restored their exact
archives. All three archives totaled 8,033,033,894 bytes. The run was canceled
during the second warm pair, and only its three trial caches were reclaimed.
The warm sample count is insufficient for adoption under the recorded
threshold.

The qualification fixture can exercise the same plan and evidence rules
without downloading gigabytes, provisioning nested daemons, or rerunning
verified product workloads. Recorded image transfers, cache transport, and
compiler wall times remain properties of real hosted runs. The existing
protected development jobs still run the product and verification checks.
An explicit live dispatch measures an image under consideration for adoption.

The [Node 20 retirement notice](https://github.blog/changelog/2026-09-23-node-20-is-no-longer-available-in-github-actions/)
describes why a Node 20 action manifest can be flagged even when the hosted
runner executes the action on Node 24.
