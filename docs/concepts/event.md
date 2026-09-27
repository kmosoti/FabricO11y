# Event

An event records a claim: a source observed something about a resource at a time, with context. Source and resource have distinct meanings even when they refer to the same real-world entity.

The [library](../../src/lib.rs) expresses that claim with an `EventId`, a tenant, source and resource IDs, event and observed times, a vector of attributes, and a payload. The payload selects a Log body or a Gauge with a name, value, and unit. Attributes use the smaller `Scalar` value algebra.

## Identity, time, and ownership

Separate Rust newtypes prevent accidental substitution of IDs or of the two timestamp roles. They do not certify that a number is unique, that a timestamp came from a clock, or that an observation happened after an event. All fields are public and values are supplied by the caller.

In the [current execution path](../architecture/system.md), the generator transfers each event to `main` through `Iterator::next`. A successful push moves it into the [buffer](buffer.md); a full push returns it to `main`. Draining moves it into an owned batch, where formatting borrows it before it is dropped. The event owns its strings and attribute vector. This Rust ownership is about memory and lifetimes; durable responsibility for preserving telemetry has not been implemented.

## Contract still to develop

Attribute key uniqueness, accepted units, finite numeric values, schema, and ID generation have no validation contract yet. A rendered debug string is for inspection and is not a storage or interchange encoding. No delivery state is attached to the event.

See the [glossary](../glossary.md), [current assumptions](../CURRENT.md), and [domain boundary decision](../decisions/ADR-0001-keep-domain-independent.md).
