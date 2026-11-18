# FabricO11y settings

Use `fabric settings` to inspect resolved collector settings as TOML without
starting services or creating telemetry storage. Use `--json` for the
versioned CLI envelope. The report identifies command-line values, environment
values, profile values, automatic settings, defaults, and missing inputs. Discovered service
settings show credential file references and do not open credential files.

```sh
fabric settings
fabric --config /data/fabrico11y/fabric.toml settings --json
fabric --config /data/fabrico11y/fabric.toml daemon --interval-ms 5000
```

## Collector profile

Copy the [collector example](../config/fabric.toml) to
`$XDG_CONFIG_HOME/fabrico11y/fabric.toml` or
`$HOME/.config/fabrico11y/fabric.toml` when XDG_CONFIG_HOME is unset.
An explicit `--config` takes priority over `FABRIC_CONFIG_FILE` and discovery.
If an explicitly selected file is missing or invalid, the command fails.

TOML documents start with `version = 1`. Collector fields belong to
`[daemon]` and are optional. Values resolve independently, with an explicit
flag ahead of its environment variable, then the profile, then the fallback.

| Field | Flag | Environment | Fallback |
| --- | --- | --- | --- |
| `data_dir` | `--data-dir` | `FABRIC_DATA_DIR` | `/data/fabrico11y` |
| `tmp_dir` | `--tmp-dir` | `FABRIC_TMP_DIR` | Valid `TMPDIR`, then `/data/tmp` |
| `spindle_socket` | `--spindle-socket` | `SPINDLE_SOCKET` | `$XDG_RUNTIME_DIR/spindle/spindle.sock` when available |
| `interval_ms` | `--interval-ms` | | 2,000 milliseconds |
| `max_ticks` | `--max-ticks` | | Run until shutdown |
| `ring_budget_bytes` | `--ring-budget-bytes` | | [Automatic collector budget](collection.md#elastic-collector-memory) |
| `max_disk_bytes` | `--max-disk-bytes` | | 4,294,967,296 bytes |

The resident collector enforces a minimum interval of 250 milliseconds.
The report's `ring_budget` table gives the automatically calculated byte
budget and the effective cgroup limit used as input when available.
An explicit hot ring budget is a byte count from 4,096 through 33,554,432.
The disk budget is a positive byte count within the TOML signed integer
range. Use absolute data and socket paths outside `/tmp` and `/dev/shm`.
Paths are literal and receive no environment or tilde expansion. The
`--spindle-only` mode requires an explicit Spindle socket through a flag,
environment variable, or profile.

## Spindle and receiver settings

Spindle and Fabric Server accept either a versioned TOML document selected by
its `.toml` filename or a legacy JSON file. Explicit launch paths select what
the service reads. The installer continues to create and use JSON.

```sh
spindle serve /data/fabrico11y/spindle.toml
fabric-server serve /data/fabrico11y/server.toml
```

Use the [standalone Spindle example](../config/spindle.toml) on a disconnected
host, or the [forwarding example](../config/spindle-fleet.toml) through an
authenticated tunnel to the [receiver example](../config/server.toml).
Replace paths, origin, and authority with those of the deployment. Keep
credential contents in private credential files and configure their paths.

For custody and projection-readiness discovery, the CLI looks for
`spindle.toml` and `server.toml` in the user configuration directory,
then falls back to the corresponding JSON file if the TOML file is absent.
Explicit endpoint and credential switches keep priority. Use the service
configuration actually passed by your units so discovery reports those values.
Each input file is capped at 64 KiB and must resolve to a regular file.
