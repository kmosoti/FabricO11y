# Delivery ownership model

This is the Stage 4 target protocol for a durable receiver and sender handoff. The default [Rust demo](../../src/main.rs) still prints volatile batches; the optional [local log](../../src/log.rs) added in Stage 5 implements a receiver-side commit. No network acknowledgement or independent sender implements the full model. The [Stage 5 implementation record](../../docs/experiments/formal/delivery-rust-stage5.md) maps the current code to its actions.

## One event, four facts

- `upstream`: the sender still keeps a retryable copy.
- `volatile`: the receiver has an in-memory copy that disappears on its crash.
- `durable`: the receiver has completed a commit that survives its crash.
- `acked`: the sender has received an acknowledgement and may discard its copy.

The [TLA+ module](DeliveryOwnership.tla) allows receive, durable commit, acknowledgement, sender forget, and receiver crash steps. `Receive` can happen again after a crash or a lost acknowledgement. The `durable` set is monotonic in this model; an already committed event may be received again, and committing it again has the same set effect. This assumes stable distinct event identities and a future deduplication rule. The current `EventId` does not guarantee global uniqueness. Only the receiver can crash in this model; the upstream copy is assumed available until `Forget`.

`AckSafety` says every acknowledged event has a durable owner. `RetainedOrDurable` says every modeled event still has an upstream copy or a durable receiver copy. Volatile memory never satisfies that second property because a crash can erase it. `EarlyAckSpec` adds an intentionally unsafe action that acknowledges after buffering alone. The [formal experiment](../../docs/experiments/formal/delivery-ownership.md) records TLC's results and limits.

## Run TLC

Use Java 11 or newer and the stable [TLA+ tools release v1.7.1](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.1). Download its `tla2tools.jar`; its SHA-256 for the release used here is `d532ba31aafe17afba1130f92410d9257454ff7393d1eb2fe032f0c07f352da5`. For example, from the repository root:

```sh
mkdir -p "$HOME/.cache/fabric_o11y/tla"
curl -fL -o "$HOME/.cache/fabric_o11y/tla/tla2tools-v1.7.1.jar" https://github.com/tlaplus/tlaplus/releases/download/v1.7.1/tla2tools.jar
printf '%s  %s\n' d532ba31aafe17afba1130f92410d9257454ff7393d1eb2fe032f0c07f352da5 "$HOME/.cache/fabric_o11y/tla/tla2tools-v1.7.1.jar" | sha256sum --check
```

If `java -version` fails on Linux `x86_64`, this verified [Temurin JRE](https://adoptium.net/installation/ci-scripts) can live in the same user cache without changing the Rust package or system Java installation:

```sh
curl -fL -o "$HOME/.cache/fabric_o11y/tla/temurin-jre.tar.gz" 'https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jre_x64_linux_hotspot_21.0.12.1_1.tar.gz'
printf '%s  %s\n' 2413149700df0f7d440500a84a8f764c535f21e5a5e87d38328b64eec2c5b500 "$HOME/.cache/fabric_o11y/tla/temurin-jre.tar.gz" | sha256sum --check
tar -xzf "$HOME/.cache/fabric_o11y/tla/temurin-jre.tar.gz" -C "$HOME/.cache/fabric_o11y/tla"
```

From the repository root, run all three checks with one command. Set `JAVA_BIN` only if `java` is not on your `PATH`:

```sh
TLA_JAR="$HOME/.cache/fabric_o11y/tla/tla2tools-v1.7.1.jar" bash formal/delivery/check.sh
```

If using that cached JRE, add `JAVA_BIN="$HOME/.cache/fabric_o11y/tla/jdk-21.0.12.1+1-jre/bin/java"` before `bash` in the same command.

The [checker](check.sh) requires the safe configuration to finish with no invariant violations and all 64 expected reachable states. The two early-ack configurations must exit with TLC status `12`, report their expected invariants, and include the expected counterexample steps. It uses a temporary TLC metadata directory and removes it afterward. To inspect the full trace yourself, run TLC directly from `formal/delivery` with one of the three `.cfg` files. `-deadlock` permits completed states without requiring a liveness property. A successful finite-state run checks only the two-event abstraction and the transitions written here. It does not test fsync, filesystem recovery, network delivery, or the Rust executable.
