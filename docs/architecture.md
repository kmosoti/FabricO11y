# FabricO11y

> [!NOTE]
> This is the original architecture proposal and research agenda. Its code sketches, guarantees, and numerical examples are not claims about implemented behavior or measured results. See [current project state](CURRENT.md) and [system architecture](architecture/system.md) for the repository today.

## First-Principles Architecture, Fundamental Units, and Ablation Plan

**Project:** FabricO11y

**Type:** Personal Rust-first full-stack observability application

**Purpose:** Build an observability system from explicit, testable primitives rather than beginning with a preselected vendor stack.

**Method:** Establish a boring reference implementation, then use ablation, property testing, model checking, fuzzing, and benchmarking to justify each mechanism that survives into the architecture.

---

# 0. Project Thesis

FabricO11y should not begin with:

- OpenTelemetry
- Kafka
- Arrow
- Parquet
- SQLite
- ClickHouse
- Splunk
- Elasticsearch/OpenSearch
- a particular queue
- a particular UI framework
- a particular storage engine

Those are candidate implementations.

The system should begin with the semantics that must exist regardless of implementation.

At the lowest useful level, an observability system records and preserves **claims about system state over time**.

The irreducible semantic core is:

1. **Identity** — which thing is this?
2. **Time** — when did it happen, and when was it observed?
3. **Value** — what was observed?
4. **Context** — what dimensions or environment give the value meaning?
5. **Provenance** — where did the information come from and what happened to it?
6. **State transition** — what changed and what transitions are legal?
7. **Ownership** — which component is currently responsible for preserving the observation?
8. **Failure** — how is rejection, uncertainty, corruption, loss, or overload represented?

Everything else should be derived from these.

```text
Identity
   +
Time
   +
Value
   +
Context
   +
Provenance
   +
State
   +
Ownership
   +
Failure
   │
   ▼
Observation
   │
   ▼
Telemetry Event
   │
   ▼
Batch / Stream
   │
   ▼
Durable Ownership
   │
   ▼
Transformation
   │
   ▼
Storage / Index
   │
   ▼
Query
   │
   ▼
Evidence about a system
```

The central engineering rule of FabricO11y is:

> Every important abstraction must have a contract, explicit failure modes, measurable cost, and at least one way to falsify its correctness claims.

---

# 1. Design Principles

## 1.1 Semantics before mechanisms

The domain model should not know that Arrow, Parquet, SQLite, OTLP, HTTP, Kafka, ClickHouse, or Splunk exist.

Those technologies may implement contracts, but they must not define the contracts.

## 1.2 Make invalid states difficult or impossible to represent

Use:

- Rust newtypes
- enums
- algebraic data types
- ownership
- typestate
- exhaustive matching

before reaching for runtime validation.

## 1.3 Explicit uncertainty

Distributed systems produce outcomes such as:

> "The request may have succeeded, but I did not receive confirmation."

FabricO11y must represent that state directly rather than incorrectly collapsing it into success or failure.

## 1.4 No silent loss

An accepted event must always have a known owner or an explicit terminal outcome.

## 1.5 Mechanism and policy are separate

Example:

```text
Policy:
    preserve critical security telemetry under overload

Mechanism:
    bounded priority queue + disk spill
```

The policy must not depend on one queue implementation.

## 1.6 Evidence-driven architecture

Nothing becomes "fundamental" because it is fashionable.

Arrow, Parquet, SQLite, async runtimes, custom WALs, columnar layouts, sketch algorithms, and indexing structures must earn their place through measured trade-offs.

## 1.7 Optimize vectors, not scores

Do not collapse system quality into one number.

Track at least:

```text
throughput
latency
CPU
memory
allocations
disk bytes
network bytes
write amplification
read amplification
duplicates
loss
recovery time
query cost
```

---

# 2. The Fundamental Semantic Units

These are the smallest domain concepts FabricO11y should treat as stable.

---

## 2.1 Identity

Identity answers:

> Which entity is this?

Use separate Rust newtypes.

```rust
pub struct EventId(pub u128);
pub struct SourceId(pub u128);
pub struct ResourceId(pub u128);
pub struct TenantId(pub u128);
pub struct SchemaId(pub u128);
pub struct StreamId(pub u128);
pub struct BatchId(pub u128);
pub struct SegmentId(pub u128);
pub struct PartitionId(pub u32);
pub struct PolicyId(pub u128);
pub struct TraceId(pub u128);
pub struct SpanId(pub u64);
```

Do not use a generic `String` everywhere.

This prevents entire classes of accidental substitutions.

### Identity-generation mechanisms are replaceable

Candidates:

- UUID
- ULID
- content-derived hash
- monotonic local sequence
- composite structured IDs

The semantic contract is stable while generation is ablatable.

---

## 2.2 Time

Observability requires multiple clocks.

At minimum:

```rust
pub struct EventTime(pub i64);
pub struct ObservedTime(pub i64);
pub struct IngestTime(pub i64);
pub struct CommitTime(pub i64);
pub struct IndexedTime(pub i64);
pub struct DurationNs(pub u64);
```

These are intentionally distinct.

Example:

```text
12:00:00  event occurred
12:00:02  agent observed it
12:00:04  gateway ingested it
12:00:06  durable commit completed
12:00:09  query index exposed it
```

Derived values:

```text
collection_delay = observed_time - event_time
ingest_delay     = ingest_time   - observed_time
commit_delay     = commit_time   - ingest_time
index_delay      = indexed_time  - commit_time
end_to_end       = indexed_time  - event_time
```

Do not silently repurpose one timestamp field at different pipeline stages.

---

## 2.3 Scalar Value

The smallest value algebra can begin as:

```rust
pub enum Scalar {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    String(String),
    Bytes(Vec<u8>),
}
```

Possible later additions:

```text
decimal
timestamp
duration
array
map
histogram
structured record
```

The project should not use a universal `serde_json::Value` as its semantic model unless experiments justify losing stronger type information.

---

## 2.4 Attribute

An attribute is:

\[
Attribute = Key \times Value
\]

```rust
pub struct AttributeKey(pub String);

pub struct Attribute {
    pub key: AttributeKey,
    pub value: Scalar,
}
```

The semantic concept does not determine physical representation.

Candidate representations:

```text
Vec<Attribute>
HashMap<Key, Value>
BTreeMap<Key, Value>
sorted Vec
interned keys
dictionary IDs
Arrow arrays
```

These belong in representation ablations.

---

## 2.5 Resource

A resource is the entity telemetry describes.

Examples:

```text
host
process
service
container
virtual machine
database
Splunk indexer
search head
network interface
AI worker
GPU
queue
```

```rust
pub struct Resource {
    pub id: ResourceId,
    pub kind: ResourceKind,
    pub attributes: Attributes,
}
```

A resource is not necessarily the same thing as the source.

---

## 2.6 Source

A source answers:

> Who observed or emitted this record?

```rust
pub struct Source {
    pub id: SourceId,
    pub kind: SourceKind,
}
```

Examples:

```text
application
kernel
collector
agent
gateway
network sensor
database
synthetic generator
```

Source begins the provenance chain.

---

## 2.7 Observation

Observation is the first truly observability-specific primitive.

```rust
pub struct Observation {
    pub id: EventId,
    pub event_time: EventTime,
    pub observed_time: ObservedTime,

    pub source: SourceId,
    pub resource: ResourceId,

    pub value: Scalar,
    pub attributes: Attributes,
}
```

Semantically:

> Source S observed value V about resource R at time T under context C.

This concept exists before logs, metrics, traces, or storage backends.

---

# 3. Signal Types Are Derived Semantics

Signals specialize observations.

```rust
pub enum SignalKind {
    Log,
    Metric,
    Span,
    Profile,
    Event,
}
```

---

## 3.1 Log

A log is primarily a discrete statement.

```rust
pub struct LogRecord {
    pub body: Scalar,
    pub severity: Option<Severity>,
    pub attributes: Attributes,
}
```

---

## 3.2 Metric

A metric has aggregation semantics.

```rust
pub enum MetricValue {
    Gauge(f64),
    Counter(u64),
    Histogram(Histogram),
}
```

Important distinctions:

```text
gauge vs counter
monotonic vs non-monotonic
delta vs cumulative
histogram buckets
units
dimensions
```

A metric is not merely a log record containing a number.

---

## 3.3 Span

A span represents an interval of execution.

```rust
pub struct Span {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,

    pub start: EventTime,
    pub end: EventTime,

    pub attributes: Attributes,
}
```

Derived invariant:

\[
duration = end - start
\]

Trace topology is a graph built from span relations.

---

## 3.4 Profile Sample

```rust
pub struct ProfileSample {
    pub stack: StackId,
    pub weight: u64,
    pub period_ns: u64,
}
```

Profiles are intentionally modeled separately from ordinary metrics.

---

# 4. Canonical Event Envelope

FabricO11y should have a canonical semantic event envelope.

```rust
pub struct Event {
    pub id: EventId,

    pub event_time: EventTime,
    pub observed_time: ObservedTime,

    pub tenant: TenantId,
    pub source: SourceId,
    pub resource: ResourceId,

    pub schema: SchemaId,
    pub signal: SignalKind,

    pub attributes: Attributes,
    pub payload: Payload,
}
```

The envelope is semantic.

It may physically become:

```text
Rust struct
Arrow record batch
Protobuf message
Parquet row
SQLite row
binary WAL record
```

but those are representations.

---

# 5. Schema

Without schema:

```text
latency = 42
```

has no stable meaning.

It could mean:

```text
42 ns
42 ms
42 s
42 arbitrary units
```

Minimum schema:

```rust
pub struct Schema {
    pub id: SchemaId,
    pub version: SchemaVersion,
    pub fields: Vec<Field>,
}
```

Each field should eventually capture:

```text
name
type
nullability
unit
semantic meaning
expected cardinality
classification
```

Useful classification:

```rust
pub enum FieldClass {
    Identity,
    Dimension,
    Measure,
    Payload,
    Secret,
    Personal,
    Internal,
}
```

---

# 6. Provenance

A production observability system should be able to answer:

> Where did this record come from, and what changed it?

Minimal form:

```rust
pub struct Provenance {
    pub source: SourceId,
    pub schema: SchemaId,
    pub transforms: Vec<TransformId>,
}
```

This may be too expensive per event.

Therefore a later representation might use:

```text
event.provenance_id
```

with deduplicated provenance graphs stored separately.

That is an ablation question.

---

# 7. State and Transition

FabricO11y is a distributed state-transition system.

General model:

\[
S_{t+1} = \delta(S_t, E_t)
\]

A naïve runtime representation might be:

```rust
pub enum DeliveryState {
    Observed,
    Buffered,
    Sent,
    Durable,
    Transformed,
    Indexed,
    Dropped,
}
```

But important transitions should often use typestate:

```rust
pub struct Observed;
pub struct Buffered;
pub struct Durable;
pub struct Indexed;

pub struct Telemetry<S> {
    pub event: Event,
    pub state: S,
}
```

Then:

```rust
fn buffer(
    event: Telemetry<Observed>,
) -> Result<Telemetry<Buffered>, BufferError>;

fn commit(
    event: Telemetry<Buffered>,
) -> Result<Telemetry<Durable>, CommitError>;

fn index(
    event: Telemetry<Durable>,
) -> Result<Telemetry<Indexed>, IndexError>;
```

The compiler now enforces part of:

\[
Indexed(e) \Rightarrow Durable(e)
\]

---

# 8. Failure and Uncertainty

A distributed operation may be:

```text
accepted
rejected
unknown
```

Do not model this as merely `Ok` or `Err`.

```rust
pub enum DeliveryResult {
    Accepted(CommitToken),
    Rejected(RejectReason),
    Unknown(UnknownReason),
}
```

`Unknown` means:

> FabricO11y cannot determine whether downstream accepted ownership.

This matters for retries and deduplication.

---

# 9. Drop

Drop must be explicit.

```rust
pub enum DropReason {
    Policy,
    InvalidSchema,
    Capacity,
    Cardinality,
    Security,
    Corruption,
    Retention,
    Unsupported,
}
```

Core invariant:

\[
Accepted(e)
\Rightarrow
Owned(e)
\lor
ExplicitlyDropped(e)
\]

No black-hole state is permitted.

---

# 10. Ordering

There is no universal ordering in a distributed observability system.

Possible orderings:

```text
producer sequence
stream sequence
partition offset
event-time order
ingest order
commit order
causal order
```

Use explicit types:

```rust
pub struct Sequence(pub u64);
pub struct Offset(pub u64);
```

Never assume:

```text
event time == ingest order == commit order
```

---

# 11. Batch

A batch is a performance mechanism.

```rust
pub struct Batch<T> {
    pub id: BatchId,
    pub items: Vec<T>,
}
```

Batch policies may constrain:

```text
event count
bytes
time
destination
schema
tenant
```

Ablations:

```text
1
16
64
256
1024
4096
```

and:

```text
count-based
byte-based
time-based
hybrid
```

---

# 12. Buffer

A buffer decouples rates and temporarily owns data.

```rust
pub trait Buffer<T> {
    type Error;

    fn push(&mut self, item: T) -> Result<(), Self::Error>;

    fn take(
        &mut self,
        limit: usize,
    ) -> Result<Vec<T>, Self::Error>;

    fn ack(
        &mut self,
        ids: &[EventId],
    ) -> Result<(), Self::Error>;
}
```

Candidate implementations:

```text
VecDeque
bounded ring
crossbeam queue
mmap-backed queue
SQLite spool
custom disk spool
```

Same contract, different economics.

---

# 13. Backpressure

Backpressure is a response to finite capacity.

```rust
pub enum Backpressure {
    Block,
    Defer,
    Spill,
    Sample,
    Reject,
}
```

Required system properties:

```text
bounded memory
explicit overload behavior
bounded queues
priority policy
observable drops
```

---

# 14. Admission

Admission answers:

> Should this observation enter this constrained subsystem now?

```rust
pub enum Admission {
    Accept,
    Defer,
    Reject(RejectReason),
}
```

Inputs may include:

```text
tenant quota
memory pressure
queue depth
cardinality budget
downstream health
signal priority
```

The admission policy must remain separate from the queue implementation.

---

# 15. Ownership

Ownership is one of the central abstractions in FabricO11y.

At every durable boundary, there must be an answer to:

> Which component is responsible for not losing event E?

Example:

```text
Agent buffer owns E
        │
        │ send
        ▼
Gateway receives E
        │
        │ commit
        ▼
Durable log owns E
        │
        │ acknowledge
        ▼
Agent may delete E
```

Primary invariant:

\[
AckUpstream(e)
\Rightarrow
DurableDownstream(e)
\]

A successful network write is not sufficient.

---

# 16. Commit

A commit means the downstream durability contract has been satisfied.

```rust
pub struct CommitToken {
    pub stream: StreamId,
    pub partition: PartitionId,
    pub offset: Offset,
}
```

Different persistence mechanisms may produce different commit tokens.

The semantic meaning must remain stable.

---

# 17. Stream

A stream is a logical ordered collection of events.

```rust
pub struct Stream {
    pub id: StreamId,
    pub schema: SchemaId,
    pub policy: PolicyId,
}
```

A stream does not imply Kafka.

---

# 18. Partition

Partitioning creates independently ordered stream subsets.

```rust
pub struct Partition {
    pub id: PartitionId,
}
```

General partition function:

\[
partition(key) \rightarrow PartitionId
\]

Potential keys:

```text
tenant
resource
service
trace
stream
hash composite
```

Partitioning changes:

```text
parallelism
ordering
skew
locality
failure isolation
replay behavior
```

Therefore partition strategy is an experiment.

---

# 19. Write-Ahead Log

A write-ahead log is one mechanism for durable ownership.

Fundamental WAL concepts:

```text
record
offset
segment
checksum
commit point
checkpoint
```

```rust
pub struct WalRecord {
    pub offset: Offset,
    pub event_id: EventId,
    pub payload: Bytes,
    pub checksum: Checksum,
}
```

Potential implementations:

```text
simple append file
segmented WAL
SQLite-backed ledger
embedded log engine
external broker
```

---

# 20. Segment

A segment is a bounded physical storage unit.

```rust
pub struct Segment {
    pub id: SegmentId,
    pub start: Offset,
    pub end: Offset,
}
```

Segments enable:

```text
rotation
recovery
checksum validation
retention
compaction
parallel readers
```

---

# 21. Routing

Routing is a deterministic policy decision.

\[
Route(Event, PolicyVersion) \rightarrow Destination
\]

```rust
pub fn route(
    event: &Event,
    policy: &RoutingPolicy,
) -> Result<Route, RoutingError>;
```

Expected property:

```text
same event + same policy version
→ same route
```

unless randomness is an explicit policy feature.

---

# 22. Destination

Destination is semantic rather than vendor-specific.

```rust
pub enum DestinationKind {
    HotIndex,
    WarmAnalytical,
    Archive,
    DeadLetter,
}
```

Implementations may later map these to:

```text
FabricO11y native index
ClickHouse
Splunk
OpenSearch
Parquet/object storage
```

---

# 23. Transform

A transform must be explicit.

```rust
pub trait Transform {
    fn apply(&self, event: Event) -> TransformResult;
}
```

```rust
pub enum TransformResult {
    Continue(Event),
    Drop {
        event: Event,
        reason: DropReason,
    },
}
```

Useful transform classes:

```text
validate
normalize
redact
enrich
project
sample
aggregate
compress
encode
```

A transform should be independently testable.

---

# 24. Cardinality

Cardinality is a first-class resource.

\[
C(X)=|\{x:x\in X\}|
\]

Examples:

```text
unique host IDs
unique service names
unique users
unique trace IDs
unique label sets
unique time-series keys
```

```rust
pub struct CardinalityBudget {
    pub max_unique: u64,
    pub max_dimensions: u32,
}
```

Estimator candidates:

```text
HashSet ground truth
HyperLogLog
Bloom filter
Count-Min Sketch
sampling
```

Estimator mechanism and enforcement policy must remain separate.

---

# 25. Physical Representation

FabricO11y should deliberately distinguish:

```text
semantic object
```

from:

```text
physical layout
```

## 25.1 Row-oriented baseline

```text
Event 1: [time, source, service, value]
Event 2: [time, source, service, value]
Event 3: [time, source, service, value]
```

Good for:

- constructing individual events
- heterogeneous data
- per-event control logic

## 25.2 Column-oriented layout

```text
time:    [t1, t2, t3]
source:  [s1, s2, s3]
service: [a,  b,  c ]
value:   [v1, v2, v3]
```

Good for:

- filtering
- projection
- aggregation
- compression
- vectorization

The row-to-column transition point is a core FabricO11y research question.

---

# 26. Apache Arrow: Candidate In-Memory Columnar Representation

Arrow is not fundamental.

It is a candidate implementation of:

```text
Batch<Event> → ColumnarBatch
```

Ablation family:

```text
R0  Vec<Event>
R1  manual struct-of-arrays
R2  Arrow RecordBatch
R3  Arrow + dictionary encoding
R4  Arrow at different batch sizes
```

Measure:

```text
construction latency
conversion cost
memory/event
allocations
filter throughput
projection throughput
aggregation throughput
serialization
high-cardinality behavior
sparse-field overhead
```

Potential outcome:

```text
ingest in row form
normalize in row form
batch
convert to Arrow
perform analytical transforms
```

But FabricO11y must discover this experimentally rather than assume it.

---

# 27. Parquet: Candidate Analytical Persistence Format

Parquet is a candidate implementation of:

```text
ColumnarBatch → DurableAnalyticalFile
```

Relevant physical units:

```text
file
row group
column chunk
page
encoding
compression
statistics
```

Ablations:

```text
row-group size
page size
compression codec
compression level
dictionary encoding
sort order
partition layout
statistics
```

Measure:

```text
write throughput
compression ratio
read amplification
projection performance
predicate pruning
replay speed
schema evolution cost
```

Parquet is especially promising for:

```text
archive
offline replay
benchmark corpora
historical analytics
```

but it must earn those roles.

---

# 28. SQLite: Candidate Embedded Transactional Substrate

SQLite may implement:

```text
checkpoint store
configuration state
schema registry
delivery ledger
deduplication metadata
experiment metadata
small local spool
```

Ablation:

```text
D0 memory only
D1 custom append log
D2 SQLite rollback journal
D3 SQLite WAL
```

Measure:

```text
write latency
batch throughput
reader concurrency
writer contention
recovery
disk amplification
database growth
checkpoint behavior
```

SQLite should not become the architecture merely because it is convenient.

---

# 29. Serialization

Serialization is a boundary mechanism.

Required semantic property:

\[
decode(encode(x)) = x
\]

modulo explicitly permitted normalization.

Candidates:

```text
Protobuf
Arrow IPC
custom binary
MessagePack
JSON
```

Measure:

```text
encoded size
encode throughput
decode throughput
allocations
schema evolution
malformed input behavior
interoperability
```

---

# 30. Transport

Transport moves candidate ownership between processes.

```rust
pub trait Transport {
    async fn send(
        &self,
        batch: Batch<Event>,
    ) -> DeliveryResult;
}
```

Candidate mechanisms:

```text
HTTP
gRPC
OTLP
QUIC
Unix domain socket
shared memory
broker protocol
```

Transport and durability are deliberately separate concepts.

---

# 31. Data Plane

The FabricO11y data plane should decompose into independently testable stages.

```text
Telemetry Source
      │
      ▼
┌───────────────┐
│ 1. Decode     │ bytes → observation
└───────┬───────┘
        ▼
┌───────────────┐
│ 2. Normalize  │ observation → canonical event
└───────┬───────┘
        ▼
┌───────────────┐
│ 3. Buffer     │ bounded local ownership
└───────┬───────┘
        ▼
┌───────────────┐
│ 4. Batch      │ amortization
└───────┬───────┘
        ▼
┌───────────────┐
│ 5. Transport  │ move toward next owner
└───────┬───────┘
        ▼
┌───────────────┐
│ 6. Admission  │ accept/defer/reject
└───────┬───────┘
        ▼
┌───────────────┐
│ 7. Commit     │ durable ownership
└───────┬───────┘
        ▼
┌───────────────┐
│ 8. Transform  │ validate/redact/enrich
└───────┬───────┘
        ▼
┌───────────────┐
│ 9. Cardinality│ enforce resource budgets
└───────┬───────┘
        ▼
┌───────────────┐
│10. Route      │ destination selection
└───────┬───────┘
        ▼
┌───────────────┐
│11. Index      │ query-optimized representation
└───────┬───────┘
        ▼
┌───────────────┐
│12. Query      │ retrieve evidence
└───────────────┘
```

Each box should expose:

1. contract
2. invariants
3. failure modes
4. complexity model
5. benchmark
6. verification method

---

# 32. Collector Agent

The agent should be intentionally narrow.

Responsibilities:

```text
observe
decode
minimal normalize
assign identity
buffer
batch
forward
```

It should not decide global topology or global retention.

Primary objective:

> Remain bounded and predictable when everything downstream is unhealthy.

---

# 33. Aggregator / Gateway

Gateway responsibilities:

```text
fan-in
authentication
schema validation
admission
batch consolidation
routing
regional policy
backpressure
```

Avoid turning the gateway into an unbounded all-purpose processor.

---

# 34. Durable Stream Layer

Contract:

```text
durability
ordered partitions
replay
consumer offsets
retention
```

Possible implementations:

```text
FabricO11y native log
embedded log
Kafka-family system
NATS JetStream
other broker
```

The first version should begin with the smallest local mechanism capable of testing the contract.

---

# 35. Processing Layer

Build processing as composition:

```text
validate
   ↓
redact
   ↓
canonicalize
   ↓
enrich
   ↓
sample
   ↓
derive
```

Do not build a monolithic processor.

---

# 36. Indexing

Indexing means:

> Convert durable telemetry into structures optimized for a specific query workload.

It does not mean:

> Send everything to one product.

Fundamental indexing concepts include:

```text
row/document
field/column
key
term
posting/reference
segment
partition/shard
statistics
```

FabricO11y should eventually be capable of evaluating multiple index designs.

---

# 37. Storage Tiers

A plausible semantic model:

```rust
pub enum StorageClass {
    Hot,
    Warm,
    Cold,
    Archive,
}
```

Possible mapping:

```text
Hot
    native query/index structures

Warm
    analytical columnar storage

Cold
    compressed retained datasets

Archive
    immutable Parquet/object-store style files
```

Do not bind these classes to products in the domain crate.

---

# 38. Query Is Fundamental

A query is a request over observable state.

Canonical workload classes:

```text
Q1  point lookup
Q2  bounded time-range scan
Q3  filter by dimensions
Q4  aggregate
Q5  group-by
Q6  top-k
Q7  trace reconstruction
Q8  correlation/join
Q9  anomaly window
Q10 full forensic replay
```

Indexing should be evaluated against these explicit query classes.

---

# 39. Query Workload

```rust
pub struct QueryWorkload {
    pub id: WorkloadId,
    pub class: QueryClass,
    pub time_window: DurationNs,
    pub selectivity: Selectivity,
}
```

Track:

```text
latency
CPU
memory
bytes scanned
cache sensitivity
```

Do not benchmark one generic "search."

---

# 40. FabricO11y API Layer

The API layer exists to expose domain operations, not storage implementation details.

Candidate boundaries:

```text
ingest API
query API
topology API
schema API
health API
experiment API
```

Possible Rust implementation:

```text
axum / tower
```

but the domain contracts should not depend on Axum.

---

# 41. FabricO11y UI

The UI is a consumer of the control/query APIs.

Useful surfaces:

```text
live pipeline topology
throughput and latency
buffer pressure
drop accounting
schema/cardinality explorer
query explorer
trace view
storage/index view
experiment comparison
fault/recovery timeline
```

The UI should make the system itself observable.

A Rust full-stack implementation can later use a Rust web framework such as Leptos, but the UI framework is not foundational.

---

# 42. Control Plane

The control plane decides what the data plane should be.

```text
Discover
   ↓
Observed Topology
   ↓
Desired Policy
   ↓
Constraints
   ↓
Plan
   ↓
Compile
   ↓
Validate
   ↓
Deploy
   ↓
Reconcile
```

---

# 43. Topology

Topology is a graph:

\[
G=(V,E)
\]

Nodes might include:

```text
agents
gateways
durable stores
processors
indexes
archives
```

Edges represent contracts.

Each edge should describe:

```text
protocol
capacity
ownership semantics
ACK semantics
retry semantics
ordering
security boundary
failure behavior
```

Most distributed failures occur in the contracts between components, so edges deserve first-class modeling.

---

# 44. Policy

Policy states what should happen.

Examples:

```text
security events must never be sampled

telemetry from tenant A must never route to tenant B

local buffers may use at most N bytes

high-cardinality fields may be stripped from metric dimensions

archive retention is N days
```

Policies should be represented separately from implementations.

---

# 45. Constraints and Z3

Use an SMT solver only where the problem genuinely looks like:

\[
\exists x : constraints(x)
\]

Examples:

```text
stream placement
partition assignment
capacity planning
replica placement
tenant isolation
retention placement
```

Possible constraints:

\[
load(node) \le capacity(node)
\]

\[
replicas(stream) \ge R
\]

\[
zone(replica_a) \ne zone(replica_b)
\]

\[
classified(stream) \Rightarrow approved(destination)
\]

Z3 should remain in a solver crate, not in the domain model.

---

# 46. Plan

A plan is an inspectable proposed configuration.

```rust
pub struct Plan {
    pub routes: Vec<RouteAssignment>,
    pub placements: Vec<Placement>,
    pub policies: Vec<CompiledPolicy>,
}
```

A plan should be testable and reviewable before it mutates runtime state.

---

# 47. Compiler

The compiler maps verified semantic configuration into implementation-specific configuration.

Possible outputs:

```text
FabricO11y agent config
gateway config
SQLite schema
Arrow schemas
Parquet layout
routing tables
external backend configuration
```

The compiler boundary prevents implementation details from contaminating core semantics.

---

# 48. Reconciliation

Reconciliation computes:

\[
desired - observed \rightarrow actions
\]

```rust
pub fn reconcile(
    observed: &ObservedState,
    desired: &DesiredState,
) -> Plan;
```

Prefer minimal corrective actions.

---

# 49. Invariants

Core FabricO11y invariants should begin with:

## S1. No silent loss

\[
Accepted(e)
\Rightarrow
Owned(e)
\lor
ExplicitlyDropped(e)
\]

## S2. Durable acknowledgement

\[
Ack(e)
\Rightarrow
Committed(e)
\]

## S3. Tenant isolation

\[
tenant(event)=tenant(destination)
\]

## S4. Indexed implies durable

\[
Indexed(e)
\Rightarrow
Durable(e)
\]

## S5. Bounded buffer

\[
buffer\_bytes \le configured\_capacity
\]

## S6. Deterministic routing

\[
route(e,p)=route(e,p)
\]

for a fixed event and policy version.

## S7. Schema correctness

Accepted canonical events must satisfy their declared schema.

## S8. Explicit transformation

A field cannot change semantic meaning without an explicit transform/schema transition.

## S9. Explicit drops

Every terminally discarded event carries a reason.

## S10. Identity preservation

Transforms that do not intentionally derive new events preserve the event identity.

---

# 50. Safety and Liveness

## Safety

Something bad never happens.

Examples:

```text
no cross-tenant routing
no acknowledged-but-undurable event
no silent loss
no unsupported canonical schema
no capacity bound silently exceeded
```

## Liveness

Something good eventually happens.

Examples:

```text
accepted event eventually commits or terminates explicitly
durable backlog drains after downstream recovery
valid configuration eventually converges
healthy consumers eventually receive eligible records
```

A system can be perfectly safe by doing nothing forever.

Therefore both classes matter.

---

# 51. Verification Stack

Different tools answer different questions.

```text
Rust compiler
    ↓
invalid states / ownership / exhaustive matching

unit tests
    ↓
known examples

proptest
    ↓
generated semantic properties

cargo-fuzz
    ↓
hostile and malformed bytes

Kani
    ↓
bounded proof/counterexample over Rust code

Loom
    ↓
concurrency interleavings

Z3
    ↓
constraint satisfiability and optimization

Miri
    ↓
undefined-behavior and unsafe-code checking

Criterion
    ↓
performance evidence
```

Do not use every tool everywhere.

---

# 52. Verification by Component

| Component | Primary verification |
|---|---|
| Domain types | compiler, unit tests |
| Decoder | fuzzing, proptest |
| Normalizer | proptest, Kani |
| Buffer | proptest, Loom, fault injection |
| Batcher | proptest, Criterion |
| Transport | state-machine tests, integration |
| Admission | proptest, Kani, Z3 if needed |
| Routing | proptest, Kani |
| WAL | Loom, fault injection, Kani for local properties |
| Transform | proptest, Kani |
| Cardinality | statistical tests, proptest, Criterion |
| Placement | Z3, generated instances |
| Index writer | integration, fault injection |
| Query/index | benchmark corpus |
| Reconciler | property tests, model checking |

---

# 53. Counterexamples Are Artifacts

A counterexample should be saved rather than discarded.

Example:

```text
1. agent buffers E
2. agent sends E
3. gateway commits E
4. ACK is lost
5. agent retries E
6. index writes E twice
```

That trace should become:

```text
tests/regressions/duplicate-after-lost-ack.*
```

The system should accumulate a corpus of discovered failures.

---

# 54. Fault

Faults must be modeled as inputs, not surprises.

Canonical fault classes:

```text
process crash
disk full
disk slow
partial write
network partition
timeout
duplicate delivery
out-of-order delivery
stale configuration
corrupt record
schema mismatch
clock skew
slow consumer
consumer death
oversized payload
cardinality explosion
memory pressure
```

---

# 55. Workload

A workload is a reproducible generated input distribution.

```rust
pub enum Workload {
    Steady,
    Burst,
    SkewedTenant,
    HighCardinality,
    LargePayload,
    TinyPayload,
    SlowConsumer,
    DuplicateStorm,
    InvalidSchema,
    BackendFailure,
}
```

Every workload should accept a deterministic seed.

```rust
pub struct WorkloadConfig {
    pub seed: u64,
    pub events: u64,
}
```

---

# 56. Measurement

Every experiment should preserve raw measurements.

Minimum result vector:

```rust
pub struct ExperimentResult {
    pub accepted: u64,
    pub committed: u64,
    pub indexed: u64,
    pub dropped: u64,
    pub duplicates: u64,

    pub throughput_eps: f64,

    pub p50_latency_ns: u64,
    pub p95_latency_ns: u64,
    pub p99_latency_ns: u64,

    pub cpu_time_ns: u64,
    pub peak_memory_bytes: u64,
    pub allocations: u64,

    pub bytes_written: u64,
    pub bytes_read: u64,

    pub recovery_time_ns: Option<u64>,
}
```

Do not produce a single "FabricO11y score."

---

# 57. Experiment

An experiment should be reproducible configuration + output.

```rust
pub struct Experiment {
    pub id: ExperimentId,
    pub workload: WorkloadConfig,
    pub pipeline: PipelineConfig,
    pub fault_schedule: FaultSchedule,
}
```

Possible manifest:

```toml
[experiment]
id = "buffer-ring-001"
hypothesis = "a bounded ring buffer reduces allocation pressure under steady ingestion"

[baseline]
buffer = "vec_deque"

[variant]
buffer = "ring"

[workload]
name = "steady-small-events"
seed = 12345
events = 10_000_000
```

---

# 58. Ablation Philosophy

An ablation asks:

> What happens when we remove or replace one mechanism while preserving as much else as possible?

FabricO11y should use three major forms.

## Removal ablation

```text
full pipeline
vs
pipeline - feature
```

## Replacement ablation

```text
same contract
implementation A
vs
implementation B
```

## Parameter ablation

```text
same implementation
parameter x = 16 / 64 / 256 / 1024
```

---

# 59. Ablation Round 0: Reference System

Build the intentionally boring reference:

```text
Synthetic Source
      │
      ▼
Rust Event
      │
      ▼
VecDeque Buffer
      │
      ▼
Vec<Event> Batch
      │
      ▼
Simple Append WAL
      │
      ▼
Null Sink
```

No Arrow.

No Parquet.

No SQLite.

No OTLP.

No async complexity unless required.

No external backend.

The point is to establish ground truth.

---

# 60. Ablation Round 1: Representation

Compare:

```text
Vec<Event>
manual struct-of-arrays
Arrow RecordBatch
```

Questions:

```text
Where should row → column conversion occur?
At what batch size does Arrow earn its conversion cost?
How does sparse heterogeneous telemetry behave?
How expensive is dictionary encoding under varying cardinality?
```

---

# 61. Ablation Round 2: Buffering

Compare:

```text
VecDeque
bounded ring
crossbeam queue
SQLite-backed spool
custom disk spool
```

Workloads:

```text
steady load
burst
slow consumer
producer saturation
restart
```

---

# 62. Ablation Round 3: Batching

Compare:

```text
1
16
64
256
1024
4096
```

and:

```text
count-based
byte-based
time-based
hybrid
```

Look for the latency-throughput knee.

---

# 63. Ablation Round 4: Persistence

Compare:

```text
no persistence
custom append-only file
segmented WAL
SQLite WAL
```

Measure:

```text
throughput
p99 latency
recovery time
write amplification
data retained after kill -9
```

Kill the process at deterministic points.

---

# 64. Ablation Round 5: Analytical Persistence

Compare:

```text
raw length-delimited records
Arrow IPC
Parquet
```

Evaluate:

```text
archive size
write speed
replay speed
projection
filtering
schema evolution
interoperability
```

---

# 65. Ablation Round 6: Ownership and Copying

Compare payload representations:

```text
Vec<u8>
Box<[u8]>
Bytes
Arc<[u8]>
borrowed slices where viable
```

Do not assume zero-copy automatically wins.

Measure:

```text
reference-count overhead
cache behavior
allocation count
copy volume
memory fragmentation
```

---

# 66. Ablation Round 7: Concurrency

Start:

```text
single-threaded
```

Then compare:

```text
pipeline threads
Tokio tasks
sharded workers
work stealing
```

Test:

```text
1 producer / 1 consumer
N producers / 1 consumer
N producers / N consumers
slow sink
tenant skew
bursts
```

Only keep concurrency that buys enough to justify its correctness cost.

---

# 67. Ablation Round 8: Routing

Compare:

```text
match
linear rule vector
HashMap
BTreeMap
compiled decision tree
radix/trie if applicable
```

Workloads:

```text
10 rules
1,000 rules
100,000 rules

uniform
Zipfian
tenant-skewed
```

---

# 68. Ablation Round 9: Cardinality

Ground truth:

```text
HashSet
```

Compare:

```text
HyperLogLog
Bloom filter
Count-Min Sketch
sampling
```

Measure both systems and statistical cost:

\[
relative\ error =
\frac{|estimate-truth|}{truth}
\]

Trade-off space:

```text
memory
CPU
error
```

---

# 69. Ablation Round 10: Formal Methods

Formal methods must earn their own complexity.

Compare defect discovery from:

```text
compiler only
+ unit tests
+ proptest
+ fuzzing
+ Kani
+ Loom
+ Z3
```

Measure:

```text
unique defects found
bug class
runtime
maintenance effort
specification effort
regression value
```

Expected result is likely not "use all tools everywhere."

Expected result is a verification map.

---

# 70. Ablation Round 11: Query and Index Structures

Begin with a deliberately simple reference index.

Then explore by query class.

Possible structures:

```text
HashMap point index
BTreeMap ordered index
inverted index
columnar scan
time-partitioned segments
bitmap indexes
Bloom filters
zone maps
posting lists
```

Evaluate against the fixed query corpus.

---

# 71. Query Corpus

Maintain stable benchmark queries.

Example:

```text
Q001 errors for service X in last 15m
Q002 telemetry for host X over 1h
Q003 p99 latency grouped by service
Q004 reconstruct trace ID T
Q005 top error-producing services
Q006 all events correlated with deployment D
Q007 cardinality by attribute key
Q008 cold historical scan
Q009 high-selectivity point lookup
Q010 broad forensic replay
```

Every schema or index change runs against this corpus.

---

# 72. Minimal Vertical Slice

The first useful FabricO11y executable should be intentionally small.

```text
Synthetic Generator
      │
      ▼
Canonical Event
      │
      ▼
Bounded Buffer
      │
      ▼
Batcher
      │
      ▼
Append WAL
      │
      ▼
Null Sink
```

Required capabilities:

```text
deterministically generate 10M events
zero unexplained loss
restart and replay
measure latency and throughput
record experiment results
run property tests
```

This is Milestone 0.1.

---

# 73. Milestone 0.2

Add:

```text
schema validation
routing
backpressure
explicit drop accounting
```

Still no real external backend.

---

# 74. Milestone 0.3

Run the first substrate ablation matrix:

```text
Vec<Event> vs Arrow

custom WAL vs SQLite

raw archive vs Arrow IPC vs Parquet
```

Only then choose initial physical representations.

---

# 75. Milestone 0.4

Add real collection:

```text
OTLP receiver
```

but map OTLP immediately into the FabricO11y domain model.

OTLP must remain an adapter.

---

# 76. Milestone 0.5

Add a native queryable store or one external analytical backend.

Do not add three at once.

The point is to test the index/query contract.

---

# 77. Milestone 0.6

Add the full-stack application shell:

```text
Rust API
Rust UI
topology view
live pipeline metrics
query workbench
experiment browser
```

FabricO11y should be able to observe its own pipeline.

---

# 78. Proposed Rust Workspace

```text
fabric-o11y/
│
├── Cargo.toml
├── README.md
├── SPEC.md
│
├── crates/
│   ├── domain/
│   │   ├── identity/
│   │   ├── time/
│   │   ├── value/
│   │   ├── schema/
│   │   ├── event/
│   │   └── error/
│   │
│   ├── ingest/
│   │   ├── synthetic/
│   │   ├── otlp/
│   │   ├── syslog/
│   │   └── file/
│   │
│   ├── normalize/
│   ├── buffer/
│   ├── batch/
│   ├── transport/
│   ├── admission/
│   ├── routing/
│   ├── wal/
│   ├── transform/
│   ├── cardinality/
│   ├── representation/
│   │   ├── row/
│   │   └── arrow/
│   │
│   ├── storage/
│   │   ├── sqlite/
│   │   ├── parquet/
│   │   └── archive/
│   │
│   ├── index/
│   ├── query/
│   ├── topology/
│   ├── policy/
│   ├── solver/
│   ├── planner/
│   ├── reconcile/
│   │
│   ├── api/
│   ├── ui/
│   │
│   ├── agent/
│   ├── gateway/
│   ├── node/
│   │
│   ├── experiments/
│   └── verification/
│
├── benches/
│   ├── ingest/
│   ├── buffer/
│   ├── representation/
│   ├── persistence/
│   ├── routing/
│   ├── cardinality/
│   └── query/
│
├── fuzz/
│
├── proofs/
│   ├── kani/
│   ├── loom/
│   └── z3/
│
├── workloads/
│   ├── steady/
│   ├── burst/
│   ├── skew/
│   ├── cardinality/
│   ├── corruption/
│   └── failures/
│
├── experiments/
│   ├── manifests/
│   └── results/
│
└── tests/
    ├── integration/
    ├── replay/
    ├── crash/
    ├── interoperability/
    └── regression/
```

This is a target structure, not a requirement to create all crates immediately.

Start with fewer crates and split only once contracts become real.

---

# 79. Dependency Direction

Desired direction:

```text
                   domain
                     ▲
         ┌───────────┼───────────┐
         │           │           │
      ingest       buffer      routing
         │           │           │
         └───────────┼───────────┘
                     ▲
                application
             /       |        \
          agent    gateway    node
                     ▲
                     │
                  adapters
```

Important constraints:

```text
domain does not depend on Tokio
domain does not depend on Arrow
domain does not depend on SQLite
domain does not depend on Parquet
domain does not depend on OTLP
domain does not depend on Z3
domain does not depend on web frameworks
```

---

# 80. Candidate Rust Libraries

These are implementation candidates, not semantic dependencies.

## Runtime / async

```text
tokio
```

Only once concurrency experiments justify it.

## HTTP / API

```text
axum
tower
```

## Serialization

```text
serde
prost
```

## Columnar representation

```text
arrow-rs
```

## Parquet

```text
parquet / arrow-rs ecosystem
```

## SQLite

Possible choices:

```text
rusqlite
```

or another appropriately scoped SQLite wrapper.

## Property testing

```text
proptest
```

## Fuzzing

```text
cargo-fuzz
libFuzzer integration
```

## Bounded model checking

```text
kani
```

## Concurrency exploration

```text
loom
```

## SMT

```text
z3
```

## Benchmarking

```text
criterion
```

## Unsafe-code checking

```text
miri
```

Do not add these all to one crate.

---

# 81. Component Specification Template

Every non-trivial FabricO11y component should eventually document:

```text
NAME

PURPOSE
What problem does this solve?

INPUT
What does it consume?

OUTPUT
What does it produce?

CONTRACT
What does it promise?

INVARIANTS
What may never happen?

FAILURES
What explicit failure outcomes exist?

OWNERSHIP
Who owns data before and after the operation?

ORDERING
What ordering is guaranteed?

COMPLEXITY
What grows with input size?

RESOURCE MODEL
CPU / memory / disk / network behavior.

VERIFICATION
Compiler / unit / proptest / fuzz / Kani / Loom / Z3.

BENCHMARK
How do we know it improved?

ALTERNATIVES
What mechanisms compete with this one?
```

---

# 82. Experiment Result Discipline

Every optimization claim should look like:

```text
Workload:
    burst-small-events-v1

Baseline:
    VecDeque

Variant:
    bounded ring

Result:
    throughput      +21%
    p99 latency     -14%
    memory          -28%
    allocations     -73%
    recovery        unchanged

Correctness:
    no invariant regression detected

Limitations:
    measured only at <= 4 producers
```

Not:

> Ring buffers are faster.

---

# 83. Architecture Decision Rule

A mechanism should enter the default FabricO11y architecture only if:

1. it satisfies the semantic contract,
2. its failure behavior is understood,
3. its relevant invariants are tested,
4. it wins or enables an important trade-off under representative workloads,
5. its complexity cost is justified,
6. its replacement boundary remains visible.

---

# 84. What FabricO11y Should Not Become

Avoid turning the project into:

## A thin OpenTelemetry frontend

OTel can be an adapter, but FabricO11y should preserve an independent domain.

## A Kafka clone

Durable streaming is one subsystem, not the product.

## A Splunk clone

Search/indexing is one part of observability.

## An Arrow demo

Columnar representation is an implementation choice.

## A benchmark toy

Correctness, durability, provenance, and recoverability matter as much as throughput.

## A formal-methods showcase

Verification tools exist to falsify claims and improve architecture, not decorate the README.

---

# 85. Primary Research Questions

FabricO11y can pursue concrete systems questions.

## Representation

> At what point should telemetry transition from row-oriented event representation to columnar representation?

## Batching

> Where is the throughput/latency knee for different signal and payload distributions?

## Durability

> Which local persistence mechanism provides the best recovery guarantees per unit of latency and write amplification?

## Arrow

> Under which event distributions does Arrow beat ordinary Rust row structures enough to justify conversion complexity?

## Parquet

> Which layout and partition strategy minimizes archive size while preserving useful replay/query performance?

## SQLite

> Which control-plane and local-delivery workloads remain comfortably inside SQLite's concurrency model?

## Cardinality

> Which probabilistic estimator gives the best memory-error trade-off for observability dimensions?

## Concurrency

> Which pipeline stages actually benefit from concurrency after queueing and synchronization overhead are included?

## Indexing

> Which index structures dominate for specific observability query classes?

## Verification

> Which verification techniques discover unique defect classes at acceptable engineering cost?

---

# 86. Initial Success Criteria

A credible first FabricO11y research prototype should be able to demonstrate:

```text
1. deterministic workload generation

2. canonical typed telemetry events

3. bounded buffering

4. batching

5. durable local commit

6. crash/restart replay

7. explicit drop accounting

8. deterministic routing

9. invariant checking

10. reproducible benchmarks

11. saved counterexamples

12. queryable experiment results
```

Only after these exist should "full observability platform" expansion become the priority.

---

# 87. First Concrete Build Order

Recommended implementation sequence:

```text
1. domain identities + Event
2. synthetic workload generator
3. experiment result schema
4. VecDeque buffer
5. fixed-size batcher
6. null sink
7. simple append WAL
8. replay/recovery
9. invariant checker
10. Criterion harness
11. proptest properties
12. crash harness
13. representation ablation
14. SQLite ablation
15. Parquet/Arrow ablation
16. OTLP adapter
17. query/index prototype
18. API
19. UI
20. distributed gateway/agent split
```

This sequence intentionally delays network and UI complexity until the core semantics can be measured.

---

# 88. The FabricO11y Stack, From Atoms to Application

```text
LEVEL 0 — SEMANTIC ATOMS
────────────────────────
Identity
Time
Value
Context
Provenance
State
Ownership
Failure


LEVEL 1 — DOMAIN OBJECTS
────────────────────────
Resource
Source
Observation
Event
Schema
Signal
Attribute


LEVEL 2 — FLOW UNITS
────────────────────
Batch
Stream
Partition
Offset
Commit
Drop
Route


LEVEL 3 — DATA-PLANE MECHANISMS
────────────────────────────────
Decode
Normalize
Buffer
Backpressure
Admission
Transport
Durability
Transform
Cardinality
Index


LEVEL 4 — PHYSICAL REPRESENTATIONS
──────────────────────────────────
Rust rows
Arrow
binary records
SQLite
Parquet
WAL segments


LEVEL 5 — CONTROL
─────────────────
Topology
Policy
Constraint
Plan
Compile
Reconcile


LEVEL 6 — VERIFICATION
──────────────────────
Types
Properties
Fuzzing
Kani
Loom
Z3
Fault injection


LEVEL 7 — MEASUREMENT
─────────────────────
Workload
Experiment
Benchmark
Counterexample
Regression corpus


LEVEL 8 — APPLICATION
─────────────────────
Agent
Gateway
Storage node
Query engine
API
UI


LEVEL 9 — FABRICO11Y
────────────────────
A self-observing, experimentally justified,
Rust-first observability system.
```

---

# 89. The Most Important Boundary

FabricO11y's deepest architectural boundary should be:

```text
SEMANTICS
    │
    ▼
CONTRACT
    │
    ▼
MECHANISM
```

For example:

```text
Semantic:
    durable ownership

Contract:
    ACK is emitted only after durable commit

Mechanisms:
    custom WAL
    SQLite
    external broker
```

Or:

```text
Semantic:
    analytical batch

Contract:
    typed columns preserving event semantics

Mechanisms:
    manual struct-of-arrays
    Arrow RecordBatch
```

Or:

```text
Semantic:
    archive

Contract:
    durable, replayable historical telemetry

Mechanisms:
    raw records
    Arrow IPC
    Parquet
```

This boundary is what makes ablation possible.

---

# 90. Final Project Principle

FabricO11y should evolve by repeatedly asking:

```text
What is the semantic requirement?

What is the smallest mechanism that satisfies it?

What invariant expresses correctness?

What workload represents reality?

What alternative mechanism competes with it?

What does the experiment show?

What new failure mode did the mechanism introduce?

Does the mechanism still deserve to exist?
```

The architecture should therefore be **discovered**, not merely designed.

The long-term goal is not the smallest system, the fastest system, or the most formally verified system in isolation.

It is a system whose important behavior is **understood**.

That makes FabricO11y suitable both as a practical personal observability application and as a systems-engineering laboratory for studying telemetry representation, durability, indexing, query execution, concurrency, and formal verification.
