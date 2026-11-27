# D-0176 · Qualify Fedora tool images before adoption

The existing proof tools are provisioned per job or restored from a large
archive. The [cold PR](https://github.com/kmosoti/FabricO11y/actions/runs/43920326727)
spent 15m41s installing native tools. One [warm native job](https://github.com/kmosoti/FabricO11y/actions/runs/43928173267/job/133522501855)
spent 3m21s restoring caches and 1m32s in installers. These are observed
baseline runs under different cache and event conditions.

Build a common Fedora layer and a native extension during a trusted tool
publication run. Use the same bootstrap scripts as local development to
retain an independent way to reproduce their inputs. Verify runtime tool
identities and a small Charon, Aeneas, and Lean check without network access
before publication.

Prefer this approach to broader proof-tool caches because immutable images
can be shared across branch cache scopes. A hosted runner still has to pull
image layers. Adopt digest-locked images only after measuring pulls and
end-to-end runs against equivalent cold and warm baselines.

Use a dedicated same-repository trial branch per job profile. The workflow can
run before the experiment reaches the default branch. Seed one immutable cache
per variant, run the other independent cold samples without a cache, and
require every warm sample to restore its variant's exact seed. This reduces
cache writes and separates runner variation from variation in cache contents.
Repository cache pressure can invalidate a sample and evict development caches.
On 2026-11-18 the repository had 8.78 GB cached, including 5.02 GB of native
proof tools. Qualification cache seeds share the default 10 GB repository
allowance. Run profiles sequentially and reclaim only each completed trial's
cache keys after recording the cache sizes. See the
[GitHub Actions limits](https://docs.github.com/en/actions/reference/limits).

See [CI image qualification](../../docs/ci-images.md).
